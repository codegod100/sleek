# artifacts.latha.org

Replaces Spindle as the build path for `.#android` + `.#flatpak`: Spindle's
`microvm` runner has too little disk to compile this project from a cache
miss (confirmed live — it died ~4min in with `No space left on device` while
compiling `rustls`/`noq-proto` for the Android target after an SDK/NDK
substitution). BuildBuddy's remote-bazel executors, already proven for this
repo's Buck2/RBE build, have the disk. This Worker is the glue:

```
push to tangled.org/nandi.uk/sleek (main)
  → Tangled fires a `push` webhook  →  https://artifacts.latha.org/webhook
  → Worker verifies HMAC, triggers a BuildBuddy remote run
  → BuildBuddy executor: nix build .#android + .#flatpak, then PUTs the
    finished files back to https://artifacts.latha.org/upload/<sha>/<file>
  → Worker stores them in R2, serves them at:
      https://artifacts.latha.org/artifacts/<sha>/sleek.apk
      https://artifacts.latha.org/artifacts/<sha>/uk.nandi.sleek.flatpak
      https://artifacts.latha.org/artifacts/latest/sleek.apk         (always newest)
      https://artifacts.latha.org/artifacts/latest/uk.nandi.sleek.flatpak
```

No npm/wrangler dependency — `worker.js` is a plain ES module Worker,
deployed via the raw Cloudflare API (`deploy.sh`, just curl + jq).

## One-time setup

1. Cloudflare: an API token with Workers Scripts, R2, and Zone(DNS) edit on
   the account that owns the `latha.org` zone, plus the account ID.
2. BuildBuddy: an org API key (this repo already has one baked into
   `.buckconfig.local` for the Buck2/RBE builds — reuse it).
3. Pick a `TANGLED_WEBHOOK_SECRET` and `UPLOAD_TOKEN` (random strings —
   `openssl rand -hex 32`). These never touch git; they're pushed straight
   into the Worker as encrypted secret bindings by `deploy.sh`.

```bash
export CLOUDFLARE_API_TOKEN=...
export CLOUDFLARE_ACCOUNT_ID=...
export TANGLED_WEBHOOK_SECRET=...   # openssl rand -hex 32
export BUILDBUDDY_API_KEY=...       # from .buckconfig.local
export UPLOAD_TOKEN=...             # openssl rand -hex 32
./deploy.sh
```

4. On tangled.org: repo → **Settings → Hooks → new webhook**
   - Payload URL: `https://artifacts.latha.org/webhook`
   - Secret: the same `TANGLED_WEBHOOK_SECRET`
   - Events: `push`
   - Active: on

That's the whole trust chain — Tangled only knows the Worker's HMAC secret,
BuildBuddy only knows its own API key (held by the Worker, never the repo),
and the remote build only knows a single-purpose upload bearer token good
for PUTting artifacts back.

## Re-deploying

`./deploy.sh` is idempotent — re-run it any time `worker.js` changes or a
secret rotates (pass the new value as the same env var).

## Testing without a Cloudflare account

`test_worker.mjs` exercises `worker.js` directly under plain Node (its
`Request`/`Response`/`crypto.subtle` globals match the Workers runtime
closely enough) — no `wrangler`, no deploy, no account needed. Stubs
`fetch` (the BuildBuddy call) and `env.ARTIFACTS` (an in-memory R2 mock) so
nothing outbound happens:

```bash
node cloudflare/proxy-latha-org/test_worker.mjs
```

Covers: HMAC accept/reject, `push`-only + `refs/heads/main`-only filtering,
the exact JSON BuildBuddy expects (`repo`/`branch`/`steps[0].run`), and the
R2 upload → download → `latest/` alias roundtrip.

## Validated so far (real infrastructure, not assumptions)

- `git clone https://tangled.org/nandi.uk/sleek` — works, plain HTTPS, no
  auth (checked both locally and from an actual BuildBuddy executor's log).
- A live BuildBuddy remote run (`POST /api/v1/Run` with the key already in
  `.buckconfig.local`) cloned the repo, self-installed Nix, and ran
  `nix build .#android` **from source** (no cache hit) to a real signed
  20M `sleek.apk` in ~10.4 min — `bazelExitCode: OK`. Executor had 18G free
  disk going in, finished at 75% used (16G/22G) — the actual problem this
  replaces (Spindle's microvm ran out of disk mid-build).
- Fixed along the way: Determinate's `--init none` install never starts
  `nix-daemon`, so the first `nix build` failed with a lock-file permission
  error until the script explicitly starts the daemon and waits for the
  socket (same pattern as `scripts/ci-nixbuild.sh`'s `setup_nixbuild()`).
- `worker.js`'s own logic (HMAC, routing, R2 read/write) — unit-tested
  locally, all passing (see above).

## Deployed and live

`artifacts.latha.org` is deployed (R2 bucket + Worker script + custom domain
route all attached) and confirmed working against real HTTP traffic:
HMAC accept/reject, the BuildBuddy trigger call, and the full R2
upload → download → `latest/` alias roundtrip all verified live, not just
in `test_worker.mjs`.

## What's still unverified

Registering the actual webhook on tangled.org's side (repo → **Settings →
Hooks** → new webhook). This is confirmed UI-only — [Tangled's own
webhooks docs](https://docs.tangled.org/webhooks) describe only the web UI
flow with no CLI/API alternative, and `/settings/hooks` 307-redirects to
`/login` with no session available outside a browser — so this one step
needs a human with an authenticated browser session; nothing to script
around it. Everything downstream of it (the Worker, BuildBuddy, R2) is
proven; once the hook is registered, a normal `git push` should light up
the whole chain. Confirmed via Cloudflare's Workers Analytics GraphQL API
that a real push before registering the hook produces zero Worker
invocations from Tangled, as expected.

`.tangled/workflows/packages.yml` still exists and still builds on Spindle
in parallel for now (belt-and-suspenders) — once this path is proven live,
consider trimming Spindle back to just the tag-move/publish steps, or
dropping it and moving that publish logic into the BuildBuddy script
instead.
