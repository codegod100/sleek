// proxy.latha.org — Tangled webhook relay + build artifact host.
//
// Flow: push to tangled.org/nandi.uk/sleek → Tangled fires a `push` webhook
// at this Worker → verify HMAC → kick a BuildBuddy remote run (clones the
// repo, `nix build`s .#android + .#flatpak on BuildBuddy's RBE executors,
// which have real disk unlike Spindle's microvm — see the "No space left on
// device" failure this replaces) → the remote script PUTs finished artifacts
// back to this Worker → stored in R2 → served back out at a public URL:
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
  // Runs on a BuildBuddy remote-bazel executor (real disk, unlike Spindle's
  // microvm). Installs Nix if missing, builds both flake outputs, streams
  // the finished files back to this Worker over HTTPS (no git/SSH creds
  // needed on either side for the upload leg).
  return [
    "set -euo pipefail",
    "if ! command -v nix >/dev/null 2>&1; then",
    "  curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \\",
    "    | sh -s -- install linux --no-confirm --init none",
    "  . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh",
    "fi",
    // Cache trust MUST live in the daemon's own /etc/nix/nix.conf, not just
    // client-side NIX_CONFIG: `nix build` below runs as the unprivileged
    // build user while nix-daemon runs as root (via sudo), and Nix silently
    // drops extra-substituters/extra-trusted-public-keys supplied by a
    // non-trusted-user client — same reasoning as scripts/ci-nixbuild.sh's
    // setup_nixbuild() writing /etc/nix/sleek-nixbuild.conf as root before
    // starting the daemon. Confirmed the hard way on a real run: cachix
    // already had cargo-vendor-dir cached (200 on its .narinfo) but the
    // build still compiled it from scratch and ran the executor's disk to
    // 100% doing so — the substituter was configured but never trusted.
    "sudo mkdir -p /etc/nix",
    "printf '%s\\n' " +
      "'experimental-features = nix-command flakes' " +
      "'accept-flake-config = true' " +
      "'extra-substituters = https://codegod100.cachix.org' " +
      "'extra-trusted-public-keys = codegod100.cachix.org-1:LZFL5VrR644WUjleS3bLbVeOdzlXqzKznQWvD5MVthA=' " +
      "| sudo tee /etc/nix/sleek-cachix.conf >/dev/null",
    "grep -qxF 'include /etc/nix/sleek-cachix.conf' /etc/nix/nix.conf 2>/dev/null || " +
      "echo 'include /etc/nix/sleek-cachix.conf' | sudo tee -a /etc/nix/nix.conf >/dev/null",
    // `--init none` never starts nix-daemon as a background process — a bare
    // `nix build` afterwards fails with `opening lock file ".../big-lock":
    // Permission denied` (confirmed on a real BuildBuddy remote run). Start
    // it ourselves and wait for the socket, same pattern as
    // scripts/ci-nixbuild.sh's setup_nixbuild(). BuildBuddy recycles
    // runners/snapshots across builds on this branch, so a daemon from an
    // earlier invocation of this script may still be alive and holding the
    // *old* config in memory — restart it whenever it's already running so
    // the nix.conf edit above always actually takes effect.
    "if [ -S /nix/var/nix/daemon-socket/socket ]; then",
    "  sudo pkill -x nix-daemon || true",
    "  for _ in $(seq 1 30); do [ ! -S /nix/var/nix/daemon-socket/socket ] && break; sleep 1; done",
    "fi",
    "sudo \"$(command -v nix)\" daemon >/tmp/nix-daemon.log 2>&1 &",
    "for _ in $(seq 1 30); do [ -S /nix/var/nix/daemon-socket/socket ] && break; sleep 1; done",
    "[ -S /nix/var/nix/daemon-socket/socket ] || { echo 'nix-daemon socket missing'; cat /tmp/nix-daemon.log; exit 1; }",
    "export NIX_CONFIG=$'experimental-features = nix-command flakes\\naccept-flake-config = true'",
    "nix show-config | grep -E '^(substituters|trusted-substituters|trusted-public-keys) =' || true",
    "echo '--- disk before gc ---'; df -h / || true",
    // result-*'s out-link symlinks from a *previous* invocation in this same
    // (possibly recycled) workdir are themselves GC roots — nix-collect-
    // garbage can't reclaim anything they point at until they're gone.
    // Confirmed the cachix-trust fix above worked (cargo-vendor-dir stopped
    // being rebuilt from source) but a run still hit "No space left on
    // device" later, mid-compile — this is the next most likely reason gc
    // wasn't actually freeing prior builds' output.
    "rm -f result-android result-flatpak",
    "sudo nix-collect-garbage -d || true",
    "echo '--- disk after gc ---'; df -h / || true",
    "nix build .#android --out-link result-android -L --print-build-logs",
    "echo '--- disk after android build ---'; df -h / || true",
    "nix build .#flatpak --out-link result-flatpak -L --print-build-logs",
    `curl -fsS -X PUT "${uploadBase}/sleek.apk" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}" --data-binary @result-android/sleek.apk`,
    // The flatpak bundle runs several hundred MB (confirmed: 617MB on a real
    // build) — a single PUT through the Worker hits Cloudflare's ~100MB
    // request body cap and 413s. Stream it via R2's multipart upload API
    // instead: split into 50MB chunks, upload each part, then complete with
    // the part list this script accumulates itself (no server-side state).
    "flatpak_file=result-flatpak/uk.nandi.sleek.flatpak",
    `flatpak_key="${sha}/uk.nandi.sleek.flatpak"`,
    `init=$(curl -fsS -X POST "https://proxy.latha.org/upload-init/$flatpak_key" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}")`,
    `uploadId=$(printf '%s' "$init" | sed -n 's/.*"uploadId":"\\([^"]*\\)".*/\\1/p')`,
    '[ -n "$uploadId" ] || { echo "failed to init multipart upload: $init"; exit 1; }',
    "rm -f /tmp/flatpak-parts.json /tmp/flatpak-chunk-*",
    'split -b 50m -d -a 4 "$flatpak_file" /tmp/flatpak-chunk-',
    'printf "[" > /tmp/flatpak-parts.json',
    "n=0",
    'for chunk in /tmp/flatpak-chunk-*; do',
    "  n=$((n + 1))",
    `  resp=$(curl -fsS -X PUT "https://proxy.latha.org/upload-part/$flatpak_key?uploadId=$uploadId&partNumber=$n" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}" --data-binary @"$chunk")`,
    `  etag=$(printf '%s' "$resp" | sed -n 's/.*"etag":"\\([^"]*\\)".*/\\1/p')`,
    '  [ -n "$etag" ] || { echo "part $n upload failed: $resp"; exit 1; }',
    '  [ "$n" -gt 1 ] && printf "," >> /tmp/flatpak-parts.json',
    '  printf \'{"partNumber":%d,"etag":"%s"}\' "$n" "$etag" >> /tmp/flatpak-parts.json',
    "done",
    'printf "]" >> /tmp/flatpak-parts.json',
    `curl -fsS -X POST "https://proxy.latha.org/upload-complete/$flatpak_key?uploadId=$uploadId" -H "Authorization: Bearer ${env.UPLOAD_TOKEN}" -H "content-type: application/json" --data-binary @/tmp/flatpak-parts.json`,
  ].join("\n");
}

async function triggerBuild(env, cloneUrl, sha) {
  const body = {
    repo: cloneUrl,
    branch: "main",
    // Default executor disk (~22G) isn't enough for a from-scratch build of
    // this project (confirmed repeatedly: "No space left on device", even
    // after fixing cache trust and clearing stale GC roots — the compile
    // itself just needs more room than that). Confirmed directly that this
    // property actually resizes the runner: a manual test with it reported
    // `/dev/vda 63G` free vs. the ~22G default with it omitted.
    platform_properties: { EstimatedFreeDiskBytes: "60GB" },
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
