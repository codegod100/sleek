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

  if (payload.ref !== "refs/heads/main") {
    return new Response(`ok: ignored ref ${payload.ref}`, { status: 200 });
  }

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
  ctx.waitUntil(triggerBuild(env, cloneUrl, sha));
  return new Response(`ok: build queued for ${sha}`, { status: 200 });
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

function buildScript(env, sha) {
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
  return [
    "set -euo pipefail",
    "if ! command -v buck2 >/dev/null 2>&1; then",
    "  mkdir -p \"$HOME/.local/bin\"",
    "  curl -fsSL -o /tmp/buck2.zst https://github.com/facebook/buck2/releases/download/latest/buck2-x86_64-unknown-linux-gnu.zst",
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
  ].join("\n");
}

async function triggerBuild(env, cloneUrl, sha) {
  const body = {
    repo: cloneUrl,
    branch: "main",
    // No platform_properties override needed now that buildScript() runs
    // buck2 instead of Nix: the actual compile happens on BuildBuddy's own
    // RE cluster (platforms/defs.bzl's custom sleek-rbe image), so this
    // trigger executor only holds the repo checkout + buck2 client + the
    // final ~20MB apk — default disk (~22G) is plenty. (The old Nix path
    // needed EstimatedFreeDiskBytes:"60GB" here because it compiled the
    // *entire* dependency graph inside this one VM — see git history.)
    steps: [{ run: buildScript(env, sha) }],
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

async function handleDownload(request, env, url) {
  const key = url.pathname.replace(/^\/artifacts\//, "");
  const obj = await env.ARTIFACTS.get(key);
  if (!obj) return new Response("not found", { status: 404 });
  const headers = new Headers();
  obj.writeHttpMetadata(headers);
  headers.set("etag", obj.httpEtag);
  return new Response(obj.body, { headers });
}
