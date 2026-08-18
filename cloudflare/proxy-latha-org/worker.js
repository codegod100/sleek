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
// Tag pushes additionally build //:sleek-host (the desktop egui binary) and
// publish both it and the apk to tangled.org as sh.tangled.repo.artifact
// release records (see "tangled release publishing" below) — that's what
// codegod100/tap's Formula/sleek.rb downloads instead of building from
// source.
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
    if (request.method === "GET" && url.pathname === "/") {
      return handleIndex(env);
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

  // main pushes build just the apk; tag pushes additionally build
  // //:sleek-host and publish both as sh.tangled.repo.artifact release
  // records (see handlePublishRelease near the bottom of this file) — everything else
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

  // Respond fast (Tangled times out at 30s + retries on 5xx); resolve the
  // tag's real commit (if this is a tag push) and trigger the BuildBuddy
  // run after responding — see triggerBuildForRef()'s own comment for why
  // a tag push needs an extra resolution step sha alone doesn't cover.
  ctx.waitUntil(triggerBuildForRef(env, cloneUrl, sha, tagName));
  return new Response(`ok: build queued for ${sha}${tagName ? ` (tag ${tagName})` : ""}`, { status: 200 });
}

async function triggerBuildForRef(env, cloneUrl, sha, tagName) {
  let buildSha = sha;
  // The tag's own hash (what sh.tangled.repo.artifact's `tag` field
  // wants) — for annotated tags this *is* payload.after (see below); for
  // lightweight tags payload.after is just the commit sha, which is the
  // right value there too (no separate tag object exists). Captured here,
  // before buildSha gets overwritten with the resolved *commit*, and
  // passed straight into buildScript() as a literal — deliberately not
  // resolved via `git rev-parse refs/tags/<name>` on the trigger executor,
  // since that ref is never fetched there (BuildBuddy's checkout uses
  // commit_sha, which fetches only that one commit object, not any tag
  // refs pointing at it — confirmed live, invocation c24d0ebd: both the
  // primary and fallback rev-parse failed with "unknown revision or path
  // not in the working tree").
  const tagHash = tagName ? sha : null;
  if (tagName) {
    // For a tag push, Tangled's webhook reports `after` (sha, above) as
    // the *tag object*'s own sha, not the commit it points at — confirmed
    // against a real delivery (2026-08-18, pushing v0.1.1): `sha` here
    // was `git cat-file -t <sha>` == "tag", and BuildBuddy's commit_sha
    // needs an actual commit for its `git fetch`. Tangled's
    // git-upload-pack ref advertisement (the info/refs endpoint) doesn't
    // include the peeled (`^{}`) line either, so there's no way to
    // resolve this via the plain git protocol. resolveTagCommit() scrapes
    // the tag's own web page instead, which always links to exactly one
    // real commit.
    const resolved = await resolveTagCommit(cloneUrl, tagName);
    if (!resolved) {
      console.error(`could not resolve a commit for tag ${tagName} (tag object ${sha})`);
      return;
    }
    buildSha = resolved;
  }
  await triggerBuild(env, cloneUrl, buildSha, { tagName, tagHash });
}

async function resolveTagCommit(cloneUrl, tagName) {
  const res = await fetch(`${cloneUrl}/tags/${encodeURIComponent(tagName)}`);
  if (!res.ok) return null;
  const html = await res.text();
  const shas = [...new Set([...html.matchAll(/\/commit\/([0-9a-f]{40})/g)].map((m) => m[1]))];
  // Only trust this if the page links to exactly one commit — anything
  // else (0, or >1 from some future page layout change) means this
  // scrape can't be trusted to have found the right one.
  return shas.length === 1 ? shas[0] : null;
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

function buildScript(env, sha, tagName, tagHash) {
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
  // //:sleek-android-apk builds (and, on main, publishes to `latest/`)
  // unconditionally. //:sleek-host — the desktop egui binary, used by the
  // codegod100/tap Homebrew formula (see Formula/sleek.rb's history and
  // https://github.com/codegod100/homebrew-tap) — only needs building on a
  // tag push: nobody installs an unpinned/unreleased build via brew, and a
  // stable download URL requires a real tag's hash anyway (see
  // publishStep()'s comment below). Skipping it on main pushes also keeps
  // ordinary main-push builds as fast as they were before this existed.
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
    // Confirmed live (invocation 58c8218e, the first tag-push build to
    // actually reach this step): a stale `buckd` daemon left running from
    // a previous run on a reused trigger-executor host still points at
    // that earlier run's buck-out/v2 — but this script's git checkout
    // step above always starts with `git clean -x -d --force`, which
    // deletes buck-out/ outright. The daemon never sees that deletion, so
    // the next `buck2 build` fails immediately with "Error validating
    // working directory ... buck-out/v2: ENOENT" before running anything.
    // `buck2 killall` tears down any daemon for this project root so the
    // next buck2 command starts a fresh one against the actually-current
    // (just-cleaned) working directory. Harmless/no-op if there's no
    // stale daemon (e.g. first run on a fresh executor).
    "buck2 killall || true",
    "buck2 --version",
    "echo '--- disk before build ---'; df -h / || true",
    "buck2 build --show-output //:sleek-android-apk 2>&1 | tee /tmp/buck2-build.log",
    "echo '--- disk after build ---'; df -h / || true",
    "apk_path=$(grep '^root//:sleek-android-apk ' /tmp/buck2-build.log | awk '{print $2}')",
    '[ -n "$apk_path" ] && [ -f "$apk_path" ] || { echo "buck2 build did not produce //:sleek-android-apk output"; exit 1; }',
    `curl -fsS -X PUT "${uploadBase}/sleek.apk" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}" --data-binary @"$apk_path"`,
  ];
  // Ask the Worker to publish `filename` (already uploaded to
  // `${uploadBase}/${filename}` by this point) as a sh.tangled.repo.artifact
  // release record (see handlePublishRelease). Best-effort — the file is
  // already safely in R2 by the time this runs, so a publish failure here
  // (e.g. OAuth was never completed via /oauth/login) shouldn't fail the
  // whole build; check /artifacts/releases/<tag>/<filename>.json after for
  // the actual outcome. tagHash (the tag object's own hash — what
  // sh.tangled.repo.artifact's `tag` field wants) is passed in as a
  // literal, computed by the caller from the webhook payload directly — NOT
  // via `git rev-parse refs/tags/<name>` here, which fails on this
  // executor: BuildBuddy's checkout only fetches the single commit_sha
  // object, never the tag ref itself (confirmed live, invocation c24d0ebd:
  // "fatal: ambiguous argument 'refs/tags/v0.1.3': unknown revision or path
  // not in the working tree").
  const publishStep = (filename) =>
    `curl -fsS -X POST "https://proxy.latha.org/publish-release/${encodeURIComponent(tagName)}" ` +
    `-H "Authorization: Bearer ${env.UPLOAD_TOKEN}" -H "content-type: application/json" ` +
    `-d "{\\"sha\\":\\"${sha}\\",\\"tagHash\\":\\"${tagHash}\\",\\"filename\\":\\"${filename}\\"}" ` +
    `|| echo "release publish failed (${filename} is still uploaded at ${uploadBase}/${filename})"`;
  if (tagName) {
    steps.push(publishStep("sleek.apk"));
    // The desktop host binary — same repo checkout, same buck2/BuildBuddy
    // setup already exported above, just a second target. Named
    // sleek-x86_64-linux (not bare "sleek") so it's self-describing once
    // it's sitting in a directory listing / download link on its own,
    // divorced from the repo/formula context that names it "sleek".
    steps.push(
      "buck2 build --show-output //:sleek-host 2>&1 | tee /tmp/buck2-build-host.log",
      "host_path=$(grep '^root//:sleek-host ' /tmp/buck2-build-host.log | awk '{print $2}')",
      '[ -n "$host_path" ] && [ -f "$host_path" ] || { echo "buck2 build did not produce //:sleek-host output"; exit 1; }',
      `curl -fsS -X PUT "${uploadBase}/sleek-x86_64-linux" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}" --data-binary @"$host_path"`,
      publishStep("sleek-x86_64-linux"),
    );
  }
  return steps.join("\n");
}

async function triggerBuild(env, cloneUrl, sha, { tagName, tagHash } = {}) {
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
    steps: [{ run: buildScript(env, sha, tagName, tagHash) }],
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
// one goes in the artifact record's `repoDid` field: which git repo the
// release belongs to. NOT the record's `repo` field — despite the name,
// that one is typed as an at-uri (a pointer to a sh.tangled.repo.repo
// *record*, which we don't have), not a bare DID; repoDid is the plain-DID
// field. Confirmed against the actual lexicon (tangled.org/tangled.org/core,
// lexicons/sh/tangled/repo/artifact.json, cross-checked via the generated
// @atcute/tangled npm package's artifact.d.ts) after v0.1.4's published
// record never showed up in the tag page's Artifacts list — turned out we'd
// been sending this as `repo` (wrong field, wrong format: bare DID isn't a
// valid at-uri) and never setting `repoDid` at all.
const TANGLED_REPO_DID = "did:plc:eimwo4adqwppiiweleayixez";

function b64Standard(bytesLike) {
  let bin = "";
  for (const b of new Uint8Array(bytesLike)) bin += String.fromCharCode(b);
  return btoa(bin);
}

// The lexicon's `tag` field is `bytes` constrained to exactly 20 bytes —
// the raw binary SHA-1 digest of the git tag object, not its 40-character
// hex string. hexToBytes() does the hex-pair -> raw-byte conversion;
// atprotoBytes() wraps those raw bytes in the `{"$bytes": "<base64>"}` JSON
// form the atproto data model uses to represent a `bytes`-typed field (a
// bare base64 *string* value, which is what this code sent before, decodes
// as a `string`-typed field instead — silently the wrong CBOR type, not
// just the wrong length). Both bugs (wrong type, wrong length: 40 raw
// UTF-8 bytes of the hex text vs. the required 20) are why v0.1.4's
// published record didn't validate as a real artifact and never appeared
// in the tag page's Artifacts list, even though createRecord itself
// returned 200 (the PDS doesn't fetch+validate against a repo's own
// externally-hosted lexicon before accepting a write).
function hexToBytes(hex) {
  if (hex.length % 2 !== 0) throw new Error(`hexToBytes: odd-length hex string ${JSON.stringify(hex)}`);
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

function atprotoBytes(rawBytes) {
  return { $bytes: b64Standard(rawBytes) };
}

// uploadBlob + createRecord against nandi's own PDS, authenticated with the
// stored OAuth session. Throws on any failure — caller decides what to do
// with that (the apk itself is already safely in R2 by the time this runs).
function contentTypeForArtifact(filename) {
  // Only the two filenames buildScript() ever actually produces need real
  // entries — application/octet-stream (a generic "just bytes,
  // browser/client should offer Save As" type) is a fine fallback for
  // anything else published this way in the future.
  if (filename.endsWith(".apk")) return "application/vnd.android.package-archive";
  return "application/octet-stream";
}

async function publishTangledArtifact(session, { bytes, filename, tagHashHex }) {
  const uploadResp = await dpopFetch(`${session.pds}/xrpc/com.atproto.repo.uploadBlob`, {
    method: "POST",
    headers: { "content-type": contentTypeForArtifact(filename) },
    body: bytes,
    dpopKeys: session.dpopKeys,
    accessToken: session.accessToken,
  });
  if (!uploadResp.ok) throw new Error(`uploadBlob failed: ${uploadResp.status} ${await uploadResp.text()}`);
  const { blob } = await uploadResp.json();

  const record = {
    repoDid: TANGLED_REPO_DID,
    tag: atprotoBytes(hexToBytes(tagHashHex)),
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
  // filename defaults to sleek.apk for backward compatibility with the
  // original single-artifact shape of this endpoint — buildScript() now
  // always sends it explicitly (both for the apk and for sleek-x86_64-linux,
  // the desktop host binary).
  const { sha, tagHash, filename = "sleek.apk" } = body;
  if (!sha || !tagHash) return new Response("missing sha/tagHash", { status: 400 });

  const obj = await env.ARTIFACTS.get(`${sha}/${filename}`);
  if (!obj) return new Response(`no artifact stored for ${sha}/${filename}`, { status: 404 });
  const bytes = await new Response(obj.body).arrayBuffer();

  const session = await getAtprotoSession(env);
  if (!session) {
    return new Response(
      "not authorized to publish — visit https://proxy.latha.org/oauth/login once, then retry",
      { status: 401 },
    );
  }

  // Keyed by filename, not just tagName — a tag push now publishes two
  // artifacts (sleek.apk and sleek-x86_64-linux), and a single
  // `releases/<tag>.json` would have the second call's result silently
  // clobber the first's.
  const recordKey = `releases/${tagName}/${filename}.json`;
  try {
    const result = await publishTangledArtifact(session, { bytes, filename, tagHashHex: tagHash });
    await env.ARTIFACTS.put(recordKey, JSON.stringify({
      tagName, sha, tagHash, filename, publishedAt: new Date().toISOString(), record: result,
    }));
    return new Response(JSON.stringify(result), { headers: { "content-type": "application/json" } });
  } catch (e) {
    await env.ARTIFACTS.put(recordKey, JSON.stringify({
      tagName, sha, tagHash, filename, failedAt: new Date().toISOString(), error: e.message,
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

// --- index page -------------------------------------------------------------

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// GET / — a plain landing page linking the artifacts this Worker actually
// hosts, so proxy.latha.org isn't just a bare API with no browsable entry
// point. "latest/*" is listed dynamically (R2 list) since it changes on
// every main-branch build; the flatpak repo + its .flatpakref are close to
// static (manually republished, see sleek-tangled-release-publishing notes)
// so their presence is just probed with head() rather than listed.
async function handleIndex(env) {
  const latest = await env.ARTIFACTS.list({ prefix: "latest/" });
  const latestFiles = latest.objects.map((o) => o.key.slice("latest/".length)).sort();

  const [flatpakrefObj, repoConfigObj] = await Promise.all([
    env.ARTIFACTS.head("uk.nandi.sleek.flatpakref"),
    env.ARTIFACTS.head("repo/config"),
  ]);

  const latestItems = latestFiles.length
    ? latestFiles.map((f) => `<li><a href="/artifacts/latest/${encodeURIComponent(f)}">${escapeHtml(f)}</a></li>`).join("\n      ")
    : "<li><em>none built yet</em></li>";

  const flatpakSection = flatpakrefObj && repoConfigObj
    ? `<section>
      <h2>Flatpak (single-command install)</h2>
      <pre>flatpak install --user https://proxy.latha.org/artifacts/uk.nandi.sleek.flatpakref</pre>
      <ul>
        <li><a href="/artifacts/uk.nandi.sleek.flatpakref">uk.nandi.sleek.flatpakref</a></li>
        <li><a href="/artifacts/repo/config">repo/config</a> (OSTree repo root)</li>
      </ul>
    </section>`
    : "";

  const html = `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>proxy.latha.org — sleek build artifacts</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  body { font-family: system-ui, sans-serif; max-width: 40rem; margin: 2rem auto; padding: 0 1rem; line-height: 1.5; }
  h1 { font-size: 1.4rem; }
  h2 { font-size: 1.1rem; margin-top: 2rem; }
  pre { background: #f0f0f0; padding: 0.6rem 0.8rem; overflow-x: auto; border-radius: 4px; }
  code { background: #f0f0f0; padding: 0.1rem 0.3rem; border-radius: 3px; }
  ul { padding-left: 1.3rem; }
  a { color: #0554b3; }
  footer { margin-top: 3rem; font-size: 0.85rem; color: #666; }
</style>
</head>
<body>
<h1>proxy.latha.org</h1>
<p>Build artifact host for <a href="https://tangled.org/nandi.uk/sleek">nandi.uk/sleek</a>.</p>

${flatpakSection}

<section>
  <h2>Latest main-branch builds</h2>
  <ul>
      ${latestItems}
  </ul>
</section>

<section>
  <h2>Tagged releases</h2>
  <p>Version tags publish <code>sh.tangled.repo.artifact</code> records, browsable under
     <strong>Artifacts</strong> on each tag's page, e.g.
     <a href="https://tangled.org/nandi.uk/sleek/tags/v0.1.5">tags/v0.1.5</a>.</p>
</section>

<footer>Source: <a href="https://tangled.org/nandi.uk/sleek/blob/main/cloudflare/proxy-latha-org">cloudflare/proxy-latha-org</a></footer>
</body>
</html>`;

  return new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } });
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
