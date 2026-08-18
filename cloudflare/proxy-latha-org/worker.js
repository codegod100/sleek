// proxy.latha.org — Tangled webhook relay + build artifact host.
//
// Flow: push to tangled.org/nandi.uk/sleek → Tangled fires a `push` webhook
// at this Worker → verify HMAC → kick a BuildBuddy remote run (clones the
// repo, runs `buck2 build //:sleek-android-apk` — the actual compile
// happens on BuildBuddy's own RE cluster via the repo's existing
// platforms/defs.bzl setup, with real BuildBuddy action-cache reuse, not
// Nix — see git history for the abandoned flake.nix/nix-daemon path and
// the disk-exhaustion problems that motivated dropping it) → the remote
// script PUTs the finished apk back to this Worker → stored in R2 →
// served back out at a public URL:
//
//   https://proxy.latha.org/artifacts/<sha>/sleek.apk
//   https://proxy.latha.org/artifacts/latest/sleek.apk   (always newest)
//
// No npm deps — plain ES module Worker, deployable via the raw Cloudflare
// API with curl (see deploy.sh). Bindings/secrets expected:
//   env.ARTIFACTS              R2 bucket binding
//   env.TANGLED_WEBHOOK_SECRET HMAC secret configured in Tangled's
//                               Settings → Hooks for this repo
//   env.BUILDBUDDY_API_KEY     org key, https://app.buildbuddy.io/ → Settings
//   env.UPLOAD_TOKEN            bearer token the remote build script uses to
//                               PUT artifacts back here (never touches git)

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    if (request.method === "POST" && url.pathname === "/webhook") {
      return handleWebhook(request, env, ctx);
    }
    if (request.method === "PUT" && url.pathname.startsWith("/upload/")) {
      return handleUpload(request, env, url);
    }
    // Cloudflare Workers reject request bodies over ~100MB — confirmed live,
    // a single-shot PUT of the 617MB flatpak bundle got a 413. R2's native
    // multipart upload API lets the build script stream it in chunks
    // instead (each chunk is its own small request; only the final
    // completion call needs the accumulated part list, which the build
    // script itself tracks across the run — no server-side state needed).
    if (request.method === "POST" && url.pathname.startsWith("/upload-init/")) {
      return handleUploadInit(request, env, url);
    }
    if (request.method === "PUT" && url.pathname.startsWith("/upload-part/")) {
      return handleUploadPart(request, env, url);
    }
    if (request.method === "POST" && url.pathname.startsWith("/upload-complete/")) {
      return handleUploadComplete(request, env, url);
    }
    if (request.method === "GET" && url.pathname.startsWith("/artifacts/")) {
      return handleDownload(request, env, url);
    }
    if (request.method === "POST" && url.pathname.startsWith("/publish-release/")) {
      return handlePublishRelease(request, env, url);
    }
    // Maintenance escape hatch for a bad/test sh.tangled.repo.artifact
    // record — same UPLOAD_TOKEN auth as everything else, deliberately not
    // exposed any other way (no listing endpoint).
    if (request.method === "POST" && url.pathname === "/admin/delete-record") {
      return handleAdminDeleteRecord(request, env, url);
    }
    // atproto OAuth — lets nandi authorize this Worker once, via a URL, to
    // publish sh.tangled.repo.artifact release records to tangled.org
    // instead of pasting an app password. See the block near the bottom of
    // this file.
    if (request.method === "GET" && url.pathname === "/client-metadata.json") {
      return handleClientMetadata();
    }
    if (request.method === "GET" && url.pathname === "/oauth/login") {
      return handleOAuthLogin(env);
    }
    if (request.method === "GET" && url.pathname === "/oauth/callback") {
      return handleOAuthCallback(request, env, url);
    }
    return new Response("not found", { status: 404 });
  },
};

// --- webhook intake -------------------------------------------------------

async function handleWebhook(request, env, ctx) {
  const rawBody = await request.text();

  // TEMPORARY debug tap (see /goal publish built artifacts on tangled
  // investigation, 2026-08-18): record every webhook delivery — headers +
  // raw body — regardless of signature/ref outcome, so a real Tangled
  // tag-push delivery can be inspected via GET /artifacts/debug/last-webhook.json
  // without needing Cloudflare API/dashboard access. Remove once the tag
  // pipeline is confirmed working end-to-end.
  ctx.waitUntil(
    env.ARTIFACTS.put(
      "debug/last-webhook.json",
      JSON.stringify({
        receivedAt: new Date().toISOString(),
        event: request.headers.get("X-Tangled-Event"),
        hasSignature: !!request.headers.get("X-Tangled-Signature-256"),
        bodyLength: rawBody.length,
        body: rawBody,
      }),
    ),
  );

  if (env.TANGLED_WEBHOOK_SECRET) {
    const sigHeader = request.headers.get("X-Tangled-Signature-256");
    const ok = await verifySignature(env.TANGLED_WEBHOOK_SECRET, rawBody, sigHeader);
    if (!ok) return new Response("bad signature", { status: 401 });
  }

  const event = request.headers.get("X-Tangled-Event") || "";
  if (event !== "push") {
    return new Response(`ok: ignored event ${event}`, { status: 200 });
  }

  let payload;
  try {
    payload = JSON.parse(rawBody);
  } catch {
    return new Response("bad json", { status: 400 });
  }

  // main pushes build the apk; tag pushes build it *and* publish it to
  // tangled.org as a sh.tangled.repo.artifact release record (see
  // handlePublishRelease near the bottom of this file) — everything else
  // (feature branches, etc.) is ignored.
  const isMain = payload.ref === "refs/heads/main";
  const tagMatch = typeof payload.ref === "string" ? payload.ref.match(/^refs\/tags\/(.+)$/) : null;
  if (!isMain && !tagMatch) {
    return new Response(`ok: ignored ref ${payload.ref}`, { status: 200 });
  }
  const tagName = tagMatch ? tagMatch[1] : null;

  const sha = payload.after;
  if (!sha) {
    return new Response("missing after", { status: 400 });
  }

  // NOT payload.repository.clone_url: confirmed on a real webhook delivery
  // that Tangled populates it as https://knot1.tangled.sh/<owner-did>/sleek,
  // which 404s ("repository not found") — the knot expects the repo's own
  // DID there, not the owner's. https://tangled.org/nandi.uk/sleek is the
  // appview host, confirmed to actually redirect+clone correctly (it 302s
  // to the right knot1.tangled.sh/<repo-did>/ URL). This Worker only ever
  // serves this one repo, so hardcode the URL that's proven to work rather
  // than trust a field Tangled itself gets wrong for this case.
  const cloneUrl = "https://tangled.org/nandi.uk/sleek";

  // Respond fast (Tangled times out at 30s + retries on 5xx); do the actual
  // BuildBuddy trigger after responding.
  ctx.waitUntil(triggerBuild(env, cloneUrl, sha, { tagName }));
  return new Response(`ok: build queued for ${sha}${tagName ? ` (tag ${tagName})` : ""}`, { status: 200 });
}

async function verifySignature(secret, rawBody, signatureHeader) {
  if (!signatureHeader || !signatureHeader.startsWith("sha256=")) return false;
  const given = signatureHeader.slice("sha256=".length);
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sigBuf = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(rawBody));
  const computed = [...new Uint8Array(sigBuf)].map((b) => b.toString(16).padStart(2, "0")).join("");
  return timingSafeEqual(computed, given);
}

function timingSafeEqual(a, b) {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

// --- BuildBuddy trigger -----------------------------------------------

function buildScript(env, sha, tagName) {
  const uploadBase = `https://proxy.latha.org/upload/${sha}`;
  // Runs on a BuildBuddy remote-bazel executor. NOT Nix anymore (see git
  // history for the abandoned flake.nix/nix-daemon path) — this repo
  // already has a proven buck2 + BuildBuddy Remote Execution setup
  // (platforms/defs.bzl's custom `sleek-rbe` container image has pixi +
  // the Android NDK baked in; //:sleek-android-apk mirrors flake.nix's old
  // sleek-android derivation step for step, see cargo.bzl). That means the
  // *actual* compile happens on BuildBuddy's RE cluster using that image —
  // this trigger executor only needs the lightweight buck2 client itself,
  // not a self-installed toolchain holding gigabytes of build state (the
  // root cause of every disk-exhaustion failure the Nix path had). Real
  // local validation of this exact target: 100% BuildBuddy action-cache
  // hit, ~2s, zero local/remote compute — "standard" RE caching working
  // as designed, unlike Nix's substituter-trust footguns.
  //
  // Only //:sleek-android-apk for now — there's no buck2 target for the
  // flatpak bundle yet (that was flake.nix's sleek-flatpak derivation;
  // porting it is future work), so this pipeline currently only publishes
  // the APK.
  const steps = [
    "set -euo pipefail",
    "if ! command -v buck2 >/dev/null 2>&1; then",
    "  mkdir -p \"$HOME/.local/bin\"",
    // musl, not gnu: confirmed live that the gnu build needs glibc >=2.32
    // (up to 2.39) and the executor image is Ubuntu 20.04 focal (glibc
    // 2.31) -> "version `GLIBC_2.32' not found". musl is statically linked
    // so it has no glibc dependency at all.
    "  curl -fsSL -o /tmp/buck2.zst https://github.com/facebook/buck2/releases/download/latest/buck2-x86_64-unknown-linux-musl.zst",
    "  command -v zstd >/dev/null 2>&1 || (sudo apt-get update -y && sudo apt-get install -y zstd)",
    '  zstd -d -f /tmp/buck2.zst -o "$HOME/.local/bin/buck2"',
    '  chmod +x "$HOME/.local/bin/buck2"',
    "fi",
    'export PATH="$HOME/.local/bin:$PATH"',
    // Not committed (.buckconfig.local is git-ignored) — the checked-in
    // .buckconfig instead reads $BUILDBUDDY_API_KEY straight from the
    // environment for [buck2_re_client]'s http_headers.
    `export BUILDBUDDY_API_KEY="${env.BUILDBUDDY_API_KEY}"`,
    "buck2 --version",
    "echo '--- disk before build ---'; df -h / || true",
    "buck2 build --show-output //:sleek-android-apk 2>&1 | tee /tmp/buck2-build.log",
    "echo '--- disk after build ---'; df -h / || true",
    "apk_path=$(grep '^root//:sleek-android-apk ' /tmp/buck2-build.log | awk '{print $2}')",
    '[ -n "$apk_path" ] && [ -f "$apk_path" ] || { echo "buck2 build did not produce //:sleek-android-apk output"; exit 1; }',
    `curl -fsS -X PUT "${uploadBase}/sleek.apk" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}" --data-binary @"$apk_path"`,
  ];
  if (tagName) {
    // Ask the Worker to publish this apk as a sh.tangled.repo.artifact
    // release record (see handlePublishRelease). Best-effort — the apk is
    // already safely uploaded above by this point, so a publish failure
    // here (e.g. OAuth was never completed via /oauth/login) shouldn't
    // fail the whole build. Check /artifacts/releases/<tag>.json after for
    // the actual outcome. `^{tag}` dereferences an annotated tag to its
    // tag object hash (what sh.tangled.repo.artifact's `tag` field wants);
    // falls back to the ref hash directly for lightweight tags, which have
    // no separate tag object.
    steps.push(
      `tag_hash=$(git rev-parse "refs/tags/${tagName}^{tag}" 2>/dev/null || git rev-parse "refs/tags/${tagName}")`,
      `curl -fsS -X POST "https://proxy.latha.org/publish-release/${encodeURIComponent(tagName)}" ` +
        `-H "Authorization: Bearer ${env.UPLOAD_TOKEN}" -H "content-type: application/json" ` +
        `-d "{\\"sha\\":\\"${sha}\\",\\"tagHash\\":\\"$tag_hash\\"}" ` +
        `|| echo "release publish failed (apk is still uploaded at ${uploadBase}/sleek.apk)"`,
    );
  }
  return steps.join("\n");
}

async function triggerBuild(env, cloneUrl, sha, { tagName } = {}) {
  const body = {
    repo: cloneUrl,
    // commit_sha pins the exact checkout regardless of ref type — required
    // for tag pushes. Confirmed live (debugging the sif-egl-fix invocation)
    // that BuildBuddy's hosted-runner repo setup does an unconditional
    // `git checkout -B <ref> origin/<ref>` after a shallow
    // `git fetch --depth=1 origin <ref>`. That works for a branch (the
    // fetch creates `refs/remotes/origin/<ref>`), but a tag ref only
    // populates FETCH_HEAD — `origin/<tag>` never exists, so the checkout
    // fails with "fatal: 'origin/<ref>' is not a commit" and the run never
    // gets past setup. commit_sha (api/v1/service.proto's RunRequest field
    // 3, independent of `branch`) sidesteps ref-type resolution entirely —
    // we always have the real sha here regardless of tag vs. branch push.
    commit_sha: sha,
    // branch is *also* sent, but only for main-branch pushes, purely as a
    // snapshot-affinity hint (BuildBuddy prefers reusing a runner snapshot
    // from a matching branch to warm-start git/bazel state) — skip it for
    // tag pushes so there's no `branch` value that could reintroduce the
    // same ref-resolution path this is fixing.
    ...(tagName ? {} : { branch: "main" }),
    // No platform_properties override needed now that buildScript() runs
    // buck2 instead of Nix: the actual compile happens on BuildBuddy's own
    // RE cluster (platforms/defs.bzl's custom sleek-rbe image), so this
    // trigger executor only holds the repo checkout + buck2 client + the
    // final ~20MB apk — default disk (~22G) is plenty. (The old Nix path
    // needed EstimatedFreeDiskBytes:"60GB" here because it compiled the
    // *entire* dependency graph inside this one VM — see git history.)
    steps: [{ run: buildScript(env, sha, tagName) }],
  };
  const resp = await fetch("https://app.buildbuddy.io/api/v1/Run", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-buildbuddy-api-key": env.BUILDBUDDY_API_KEY,
    },
    body: JSON.stringify(body),
  });
  const respText = await resp.text();
  if (!resp.ok) {
    console.log("buildbuddy trigger failed", resp.status, respText);
    await env.ARTIFACTS.put(
      `${sha}/invocation.json`,
      JSON.stringify({ triggeredAt: new Date().toISOString(), triggerFailed: true, status: resp.status, body: respText }),
    );
    return;
  }
  // Record the invocation ID immediately (not just once the build finishes)
  // so a run can be looked up via BuildBuddy's GetInvocation/GetLog APIs
  // (they need an invocationId; there's no commit-sha lookup for runs
  // triggered this way) without waiting on the build itself.
  let invocationId = null;
  try {
    invocationId = JSON.parse(respText).invocationId || null;
  } catch {
    // leave null; still record that a trigger happened
  }
  await env.ARTIFACTS.put(
    `${sha}/invocation.json`,
    JSON.stringify({ triggeredAt: new Date().toISOString(), invocationId }),
  );
}

// --- artifact storage (R2) -------------------------------------------------

async function handleUpload(request, env, url) {
  if (!checkUploadAuth(request, env)) return new Response("unauthorized", { status: 401 });
  const key = url.pathname.replace(/^\/upload\//, "");
  if (!key) return new Response("missing key", { status: 400 });

  await env.ARTIFACTS.put(key, request.body);
  await mirrorToLatest(env, key); // stable "latest/<filename>" alias

  return new Response(`ok: stored ${key}`, { status: 200 });
}

function checkUploadAuth(request, env) {
  const auth = request.headers.get("Authorization") || "";
  return env.UPLOAD_TOKEN && auth === `Bearer ${env.UPLOAD_TOKEN}`;
}

async function mirrorToLatest(env, key) {
  const filename = key.split("/").pop();
  const stored = await env.ARTIFACTS.get(key);
  if (stored) await env.ARTIFACTS.put(`latest/${filename}`, stored.body);
}

async function handleUploadInit(request, env, url) {
  if (!checkUploadAuth(request, env)) return new Response("unauthorized", { status: 401 });
  const key = url.pathname.replace(/^\/upload-init\//, "");
  if (!key) return new Response("missing key", { status: 400 });
  const mpu = await env.ARTIFACTS.createMultipartUpload(key);
  return new Response(JSON.stringify({ uploadId: mpu.uploadId, key: mpu.key }), {
    headers: { "content-type": "application/json" },
  });
}

async function handleUploadPart(request, env, url) {
  if (!checkUploadAuth(request, env)) return new Response("unauthorized", { status: 401 });
  const key = url.pathname.replace(/^\/upload-part\//, "");
  const uploadId = url.searchParams.get("uploadId");
  const partNumber = Number(url.searchParams.get("partNumber"));
  if (!key || !uploadId || !partNumber) return new Response("missing key/uploadId/partNumber", { status: 400 });
  const mpu = env.ARTIFACTS.resumeMultipartUpload(key, uploadId);
  const part = await mpu.uploadPart(partNumber, request.body);
  return new Response(JSON.stringify({ partNumber: part.partNumber, etag: part.etag }), {
    headers: { "content-type": "application/json" },
  });
}

async function handleUploadComplete(request, env, url) {
  if (!checkUploadAuth(request, env)) return new Response("unauthorized", { status: 401 });
  const key = url.pathname.replace(/^\/upload-complete\//, "");
  const uploadId = url.searchParams.get("uploadId");
  if (!key || !uploadId) return new Response("missing key/uploadId", { status: 400 });
  let parts;
  try {
    parts = JSON.parse(await request.text());
  } catch {
    return new Response("bad json parts list", { status: 400 });
  }
  const mpu = env.ARTIFACTS.resumeMultipartUpload(key, uploadId);
  await mpu.complete(parts);
  await mirrorToLatest(env, key);
  return new Response(`ok: completed multipart upload for ${key}`, { status: 200 });
}

// --- tangled release publishing (sh.tangled.repo.artifact) ---------------

// The repo's own auto-assigned DID (from the `tangled` git remote,
// git@tangled.org:did:plc:eimwo4adqwppiiweleayixez) — NOT nandi's personal
// DID (that's ATPROTO_DID below, used for the OAuth session/identity). This
// one goes in the artifact record's `repo` field: which git repo the
// release belongs to.
const TANGLED_REPO_DID = "did:plc:eimwo4adqwppiiweleayixez";

function b64Standard(bytesLike) {
  let bin = "";
  for (const b of new Uint8Array(bytesLike)) bin += String.fromCharCode(b);
  return btoa(bin);
}

// uploadBlob + createRecord against nandi's own PDS, authenticated with the
// stored OAuth session. Throws on any failure — caller decides what to do
// with that (the apk itself is already safely in R2 by the time this runs).
async function publishTangledArtifact(session, { apkBytes, filename, tagHashHex }) {
  const uploadResp = await dpopFetch(`${session.pds}/xrpc/com.atproto.repo.uploadBlob`, {
    method: "POST",
    headers: { "content-type": "application/vnd.android.package-archive" },
    body: apkBytes,
    dpopKeys: session.dpopKeys,
    accessToken: session.accessToken,
  });
  if (!uploadResp.ok) throw new Error(`uploadBlob failed: ${uploadResp.status} ${await uploadResp.text()}`);
  const { blob } = await uploadResp.json();

  const record = {
    repo: TANGLED_REPO_DID,
    tag: b64Standard(new TextEncoder().encode(tagHashHex)),
    name: filename,
    artifact: blob,
    createdAt: new Date().toISOString(),
  };
  const createResp = await dpopFetch(`${session.pds}/xrpc/com.atproto.repo.createRecord`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ repo: session.sub, collection: "sh.tangled.repo.artifact", record }),
    dpopKeys: session.dpopKeys,
    accessToken: session.accessToken,
  });
  if (!createResp.ok) throw new Error(`createRecord failed: ${createResp.status} ${await createResp.text()}`);
  return createResp.json();
}

// Called by buildScript() after a tag-triggered build's apk is already
// uploaded (see /publish-release/<tag> route). Not part of the atproto
// OAuth block below, but depends on it (getAtprotoSession, dpopFetch).
async function handlePublishRelease(request, env, url) {
  if (!checkUploadAuth(request, env)) return new Response("unauthorized", { status: 401 });
  const tagName = decodeURIComponent(url.pathname.replace(/^\/publish-release\//, ""));
  if (!tagName) return new Response("missing tag", { status: 400 });

  let body;
  try {
    body = JSON.parse(await request.text());
  } catch {
    return new Response("bad json", { status: 400 });
  }
  const { sha, tagHash } = body;
  if (!sha || !tagHash) return new Response("missing sha/tagHash", { status: 400 });

  const apkObj = await env.ARTIFACTS.get(`${sha}/sleek.apk`);
  if (!apkObj) return new Response(`no artifact stored for ${sha}/sleek.apk`, { status: 404 });
  const apkBytes = await new Response(apkObj.body).arrayBuffer();

  const session = await getAtprotoSession(env);
  if (!session) {
    return new Response(
      "not authorized to publish — visit https://proxy.latha.org/oauth/login once, then retry",
      { status: 401 },
    );
  }

  try {
    const result = await publishTangledArtifact(session, { apkBytes, filename: "sleek.apk", tagHashHex: tagHash });
    await env.ARTIFACTS.put(`releases/${tagName}.json`, JSON.stringify({
      tagName, sha, tagHash, publishedAt: new Date().toISOString(), record: result,
    }));
    return new Response(JSON.stringify(result), { headers: { "content-type": "application/json" } });
  } catch (e) {
    await env.ARTIFACTS.put(`releases/${tagName}.json`, JSON.stringify({
      tagName, sha, tagHash, failedAt: new Date().toISOString(), error: e.message,
    }));
    return new Response(`publish failed: ${e.message}`, { status: 502 });
  }
}

async function handleAdminDeleteRecord(request, env, url) {
  if (!checkUploadAuth(request, env)) return new Response("unauthorized", { status: 401 });
  const rkey = url.searchParams.get("rkey");
  if (!rkey) return new Response("missing ?rkey=", { status: 400 });
  const session = await getAtprotoSession(env);
  if (!session) return new Response("no atproto session", { status: 401 });
  const resp = await dpopFetch(`${session.pds}/xrpc/com.atproto.repo.deleteRecord`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ repo: session.sub, collection: "sh.tangled.repo.artifact", rkey }),
    dpopKeys: session.dpopKeys,
    accessToken: session.accessToken,
  });
  return new Response(await resp.text(), { status: resp.status, headers: { "content-type": "application/json" } });
}

async function handleDownload(request, env, url) {
  const key = url.pathname.replace(/^\/artifacts\//, "");
  // oauth/ holds the atproto session (refresh token + DPoP private key) in
  // this same R2 bucket — never let it be fetched through the public
  // artifact route.
  if (key.startsWith("oauth/")) return new Response("not found", { status: 404 });
  const obj = await env.ARTIFACTS.get(key);
  if (!obj) return new Response("not found", { status: 404 });
  const headers = new Headers();
  obj.writeHttpMetadata(headers);
  headers.set("etag", obj.httpEtag);
  return new Response(obj.body, { headers });
}

// --- atproto OAuth ---------------------------------------------------------
//
// Lets nandi authorize this Worker once, by visiting a URL and clicking
// approve, instead of generating/pasting an app password. Standard atproto
// OAuth: no client_secret — client_id is a URL to a hosted metadata document
// (this Worker serves it). Pushed Authorization Request (PAR) + PKCE +
// DPoP-bound tokens are all mandatory parts of the protocol, not optional
// extras. The resulting session (refresh token + the DPoP keypair it's
// bound to) is stashed in the same R2 bucket as build artifacts, under an
// `oauth/` prefix that handleDownload refuses to ever serve publicly.
//
// End goal this unlocks: a tag-triggered build step can later use
// getAtprotoSession() to call com.atproto.repo.uploadBlob +
// sh.tangled.repo.artifact createRecord and post release artifacts
// straight to the tangled.org repo page. That publish step itself isn't
// wired up yet — this is just the auth plumbing.

const ATPROTO_CLIENT_ID = "https://proxy.latha.org/client-metadata.json";
const ATPROTO_REDIRECT_URI = "https://proxy.latha.org/oauth/callback";
const ATPROTO_SCOPE = "atproto transition:generic";
// nandi's personal DID (handle nandi.uk resolves here; DIDs are the stable
// identifier, handles can change). Confirmed live: resolveHandle(nandi.uk)
// -> this DID -> plc.directory doc has alsoKnownAs: ["at://nandi.uk"] and a
// real Bluesky-hosted PDS. (Not the tangled `git@tangled.org:did:plc:...`
// remote DID — that one's the *repo's* auto-assigned DID, empty
// alsoKnownAs, served by the knot itself, not nandi's identity.)
const ATPROTO_DID = "did:plc:ngokl2gnmpbvuvrfckja3g7p";

function b64url(bytesLike) {
  let bin = "";
  for (const b of new Uint8Array(bytesLike)) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function b64urlFromString(str) {
  return b64url(new TextEncoder().encode(str));
}
function randomB64url(byteLen) {
  const arr = new Uint8Array(byteLen);
  crypto.getRandomValues(arr);
  return b64url(arr);
}
async function sha256(input) {
  return crypto.subtle.digest("SHA-256", typeof input === "string" ? new TextEncoder().encode(input) : input);
}

async function generateDpopKeypair() {
  const kp = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const privateJwk = await crypto.subtle.exportKey("jwk", kp.privateKey);
  const publicJwk = await crypto.subtle.exportKey("jwk", kp.publicKey);
  delete publicJwk.d;
  return { privateJwk, publicJwk };
}

// One DPoP proof JWT, signed fresh per request (each needs its own `jti`).
// `nonce` is the server-issued DPoP-Nonce from a prior response, once we
// have one. `accessToken`, when present, adds the `ath` claim required on
// resource-server requests (not needed for PAR/token-endpoint calls).
async function signDpopProof(privateJwk, publicJwk, { htm, htu, nonce, accessToken }) {
  const key = await crypto.subtle.importKey(
    "jwk", privateJwk, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"],
  );
  const header = {
    typ: "dpop+jwt",
    alg: "ES256",
    jwk: { kty: publicJwk.kty, crv: publicJwk.crv, x: publicJwk.x, y: publicJwk.y },
  };
  const payload = { jti: randomB64url(16), htm, htu, iat: Math.floor(Date.now() / 1000) };
  if (nonce) payload.nonce = nonce;
  if (accessToken) payload.ath = b64url(await sha256(accessToken));
  const signingInput = `${b64urlFromString(JSON.stringify(header))}.${b64urlFromString(JSON.stringify(payload))}`;
  // WebCrypto's ECDSA/P-256 signature output is already the raw r||s format
  // JOSE/ES256 wants — no DER re-encoding needed.
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(signingInput));
  return `${signingInput}.${b64url(sig)}`;
}

// POSTs with a DPoP proof, handling the mandatory nonce dance every atproto
// auth/resource server does: first attempt commonly 400s with
// {"error":"use_dpop_nonce"} plus a DPoP-Nonce response header; retry once
// with that nonce baked into the proof.
async function dpopFetch(url, { method = "POST", body, headers = {}, dpopKeys, nonce, accessToken } = {}) {
  const attempt = async (n) => {
    const proof = await signDpopProof(dpopKeys.privateJwk, dpopKeys.publicJwk, { htm: method, htu: url, nonce: n, accessToken });
    const h = { ...headers, DPoP: proof };
    if (accessToken) h["Authorization"] = `DPoP ${accessToken}`;
    return fetch(url, { method, headers: h, body });
  };
  let resp = await attempt(nonce);
  // The auth/token endpoints signal a required nonce with 400 + a JSON
  // {"error":"use_dpop_nonce"} body (confirmed live against PAR/token).
  // Resource-server endpoints (uploadBlob, createRecord) instead use 401 +
  // a WWW-Authenticate header (confirmed live against uploadBlob — it does
  // NOT reuse the 400+JSON shape). Check both; either way a DPoP-Nonce
  // response header carries the value to retry with.
  if (resp.status === 400 || resp.status === 401) {
    let isNonceError = (resp.headers.get("WWW-Authenticate") || "").includes("use_dpop_nonce");
    if (!isNonceError) {
      try {
        const errBody = await resp.clone().json();
        isNonceError = errBody.error === "use_dpop_nonce";
      } catch { /* not json, not a nonce error */ }
    }
    if (isNonceError && resp.headers.get("DPoP-Nonce")) {
      resp = await attempt(resp.headers.get("DPoP-Nonce"));
    }
  }
  return resp;
}

function handleClientMetadata() {
  return new Response(JSON.stringify({
    client_id: ATPROTO_CLIENT_ID,
    client_name: "sleek build artifact publisher",
    client_uri: "https://proxy.latha.org/",
    redirect_uris: [ATPROTO_REDIRECT_URI],
    scope: ATPROTO_SCOPE,
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
    token_endpoint_auth_method: "none",
    application_type: "web",
    dpop_bound_access_tokens: true,
  }), { headers: { "content-type": "application/json" } });
}

// PDS lookup is dynamic (in case of migration) even though the DID is
// hardcoded; the Bluesky-hosted PDS here delegates OAuth to a separate
// entryway/authorization server, discovered via the protected-resource
// metadata rather than assumed to be the PDS itself.
async function resolvePdsAndAuthServer() {
  const didDocResp = await fetch(`https://plc.directory/${ATPROTO_DID}`);
  if (!didDocResp.ok) throw new Error(`plc.directory lookup failed: ${didDocResp.status}`);
  const didDoc = await didDocResp.json();
  const pds = didDoc.service.find((s) => s.type === "AtprotoPersonalDataServer")?.serviceEndpoint;
  if (!pds) throw new Error("no AtprotoPersonalDataServer service in DID doc");

  const resourceMetaResp = await fetch(`${pds}/.well-known/oauth-protected-resource`);
  if (!resourceMetaResp.ok) throw new Error(`oauth-protected-resource lookup failed: ${resourceMetaResp.status}`);
  const resourceMeta = await resourceMetaResp.json();
  const issuer = resourceMeta.authorization_servers?.[0];
  if (!issuer) throw new Error("no authorization_servers in protected-resource metadata");

  const authServerMetaResp = await fetch(`${issuer}/.well-known/oauth-authorization-server`);
  if (!authServerMetaResp.ok) throw new Error(`oauth-authorization-server lookup failed: ${authServerMetaResp.status}`);
  const authServerMeta = await authServerMetaResp.json();
  return { pds, issuer, authServerMeta };
}

async function handleOAuthLogin(env) {
  try {
    const { pds, issuer, authServerMeta } = await resolvePdsAndAuthServer();
    const dpopKeys = await generateDpopKeypair();
    const verifier = randomB64url(32);
    const challenge = b64url(await sha256(verifier));
    const state = randomB64url(16);

    const parBody = new URLSearchParams({
      client_id: ATPROTO_CLIENT_ID,
      redirect_uri: ATPROTO_REDIRECT_URI,
      response_type: "code",
      scope: ATPROTO_SCOPE,
      state,
      code_challenge: challenge,
      code_challenge_method: "S256",
      login_hint: ATPROTO_DID,
    });

    const parResp = await dpopFetch(authServerMeta.pushed_authorization_request_endpoint, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: parBody.toString(),
      dpopKeys,
    });
    if (!parResp.ok) {
      const t = await parResp.text();
      return new Response(`PAR request failed: ${parResp.status} ${t}`, { status: 502 });
    }
    const { request_uri } = await parResp.json();

    await env.ARTIFACTS.put(`oauth/flow/${state}.json`, JSON.stringify({
      verifier, dpopKeys, pds, issuer, authServerMeta, createdAt: new Date().toISOString(),
    }));

    const authUrl = new URL(authServerMeta.authorization_endpoint);
    authUrl.searchParams.set("client_id", ATPROTO_CLIENT_ID);
    authUrl.searchParams.set("request_uri", request_uri);
    return Response.redirect(authUrl.toString(), 302);
  } catch (e) {
    return new Response(`oauth login setup failed: ${e.message}`, { status: 500 });
  }
}

async function handleOAuthCallback(request, env, url) {
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  const err = url.searchParams.get("error");
  if (err) return new Response(`oauth error: ${err} ${url.searchParams.get("error_description") || ""}`, { status: 400 });
  if (!code || !state) return new Response("missing code/state", { status: 400 });

  const flowObj = await env.ARTIFACTS.get(`oauth/flow/${state}.json`);
  if (!flowObj) return new Response("unknown or expired oauth state — try /oauth/login again", { status: 400 });
  const flow = JSON.parse(await new Response(flowObj.body).text());

  try {
    const tokenBody = new URLSearchParams({
      grant_type: "authorization_code",
      code,
      redirect_uri: ATPROTO_REDIRECT_URI,
      client_id: ATPROTO_CLIENT_ID,
      code_verifier: flow.verifier,
    });
    const tokenResp = await dpopFetch(flow.authServerMeta.token_endpoint, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: tokenBody.toString(),
      dpopKeys: flow.dpopKeys,
    });
    if (!tokenResp.ok) {
      const t = await tokenResp.text();
      return new Response(`token exchange failed: ${tokenResp.status} ${t}`, { status: 502 });
    }
    const tokens = await tokenResp.json();

    await env.ARTIFACTS.put("oauth/session.json", JSON.stringify({
      accessToken: tokens.access_token,
      refreshToken: tokens.refresh_token,
      expiresAt: Date.now() + (tokens.expires_in || 3600) * 1000,
      dpopKeys: flow.dpopKeys,
      pds: flow.pds,
      issuer: flow.issuer,
      tokenEndpoint: flow.authServerMeta.token_endpoint,
      sub: tokens.sub || ATPROTO_DID,
      updatedAt: new Date().toISOString(),
    }));
    await env.ARTIFACTS.delete(`oauth/flow/${state}.json`);

    return new Response(
      "Authorized. sleek's build publisher is now connected to your atproto account — you can close this tab.",
      { headers: { "content-type": "text/plain" } },
    );
  } catch (e) {
    return new Response(`oauth callback failed: ${e.message}`, { status: 500 });
  }
}

// For later use by a tag-triggered publish step: returns a valid (silently
// refreshed if needed) access token plus the DPoP key material to sign PDS
// requests with (uploadBlob / createRecord for sh.tangled.repo.artifact).
// Returns null if never authorized or the refresh token itself is dead —
// caller should fall back to pointing at /oauth/login again in that case.
async function getAtprotoSession(env) {
  const obj = await env.ARTIFACTS.get("oauth/session.json");
  if (!obj) return null;
  let session = JSON.parse(await new Response(obj.body).text());
  if (Date.now() < session.expiresAt - 60_000) return session;

  const resp = await dpopFetch(session.tokenEndpoint, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "refresh_token",
      refresh_token: session.refreshToken,
      client_id: ATPROTO_CLIENT_ID,
    }).toString(),
    dpopKeys: session.dpopKeys,
  });
  if (!resp.ok) return null;
  const tokens = await resp.json();
  session = {
    ...session,
    accessToken: tokens.access_token,
    refreshToken: tokens.refresh_token || session.refreshToken,
    expiresAt: Date.now() + (tokens.expires_in || 3600) * 1000,
    updatedAt: new Date().toISOString(),
  };
  await env.ARTIFACTS.put("oauth/session.json", JSON.stringify(session));
  return session;
}
