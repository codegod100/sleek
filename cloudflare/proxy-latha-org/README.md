# proxy.latha.org

Replaces Spindle as the build path for `.#android` + `.#flatpak`: Spindle's
`microvm` runner has too little disk to compile this project from a cache
miss (confirmed live — it died ~4min in with `No space left on device` while
compiling `rustls`/`noq-proto` for the Android target after an SDK/NDK
substitution). BuildBuddy's remote-bazel executors, already proven for this
repo's Buck2/RBE build, have the disk. This Worker is the glue:

```
push to tangled.org/nandi.uk/sleek (main)
  → Tangled fires a `push` webhook  →  https://proxy.latha.org/webhook
  → Worker verifies HMAC, triggers a BuildBuddy remote run
  → BuildBuddy executor: nix build .#android + .#flatpak, then PUTs the
    finished files back to https://proxy.latha.org/upload/<sha>/<file>
  → Worker stores them in R2, serves them at:
      https://proxy.latha.org/artifacts/<sha>/sleek.apk
      https://proxy.latha.org/artifacts/<sha>/uk.nandi.sleek.flatpak
      https://proxy.latha.org/artifacts/latest/sleek.apk         (always newest)
      https://proxy.latha.org/artifacts/latest/uk.nandi.sleek.flatpak
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
   - Payload URL: `https://proxy.latha.org/webhook`
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

## Open items / assumptions to validate on the first real run

- BuildBuddy's `repo` field is documented only with `git@github.com:...`
  examples; this sends Tangled's `repository.clone_url` (plain HTTPS)
  instead. Should work for a generic git clone, but is unconfirmed against
  BuildBuddy's remote-bazel runner for a non-GitHub host — watch the first
  triggered run's BuildBuddy invocation page for a clone failure.
- The remote script self-installs Nix (Determinate installer) since
  BuildBuddy's stock remote-bazel image isn't expected to have it — this
  needs root/sudo on the executor; unconfirmed until tested.
- `.tangled/workflows/packages.yml` still exists and still builds on Spindle
  in parallel for now (belt-and-suspenders) — once this path is proven,
  consider trimming Spindle back to just the tag-move/publish steps, or
  dropping it and moving publish logic into the BuildBuddy script instead.
