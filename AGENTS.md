# AGENTS.md

## Cursor Cloud specific instructions

Sleek is a Rust + egui desktop/mobile **freeq (IRC) chat client**. The runnable
product in this environment is the **desktop host** (`host/` → binary `sleek`);
the `android/` crate is the shared UI/logic library (also built for Android via
`cargo-apk`/Waydroid, which is not runnable here — no binder/Waydroid kernel
support).

### Toolchain lives in the Nix flake dev shell
- Nix (Determinate, multi-user, installed with `--init none`) is preinstalled in
  the VM image. There is **no systemd**, so the `nix-daemon` is not started
  automatically — the update script starts it, and `scripts/enter` will start it
  on demand too. If `nix` commands hang/fail with a daemon-socket error, start it
  with `sudo nohup /nix/var/nix/profiles/default/bin/nix-daemon >/tmp/nix-daemon.log 2>&1 &`.
- `nix` is on the PATH of login shells (Determinate profile). In a bare shell,
  source it: `. /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh`.
- All commands must run **inside the flake dev shell** so `LIBCLANG_PATH`,
  `BINDGEN_EXTRA_CLANG_ARGS` (needed by the `v4l2r` camera bindgen),
  `PKG_CONFIG_PATH`, `OPENSSL_NO_VENDOR`, and `SLEEK_LD_LIBRARY_PATH` are set.
  Use `nix develop /workspace --command <cmd>` or `./scripts/enter <cmd>`.
- The flake's `nixConfig` advertises `codegod100.cachix.org` as an
  `extra-substituter`. On multi-user Determinate Nix (`trusted-users = root`
  only), that is **not** enough: non-trusted users get
  `ignoring untrusted substituter` and rebuild toolchain deps from source.
  Bootstrap writes the cache into `/etc/nix/nix.custom.conf` as
  `extra-substituters`, `extra-trusted-substituters`, and
  `extra-trusted-public-keys`, then reloads `nix-daemon`. If those warnings
  appear, re-run `bash scripts/codespace-bootstrap.sh` (needs passwordless
  sudo). Prefer `nix run --accept-flake-config` (or plain `nix run` once the
  daemon trusts the cache); do not put the signing key in user `nix.conf`.

### Sibling path dependencies (required for working-tree builds)
`android/Cargo.toml` uses path deps `../../vidya` and `../../freeq/freeq-sdk`.
Because the repo root is `/workspace`, those resolve to **`/vidya`** and
**`/freeq`** (filesystem root). Both are pinned in **`flake.lock`**: vidya from
Radicle Garden (`rad:z2UqGTRH21s3pHnJgSuMwRaPPNNcW`) and freeq from
`github:codegod100/freeq`. Materialize siblings from the lock with:

```bash
bash scripts/sync-flake-path-deps.sh
```

(`scripts/codespace-host.sh` runs this automatically when `nix` is available.)
Do not `git pull` sibling checkouts blindly — use `nix flake update vidya` (or
`freeq`) in the sleek repo, then re-sync.

### Build / lint / test / run (desktop host)
- Build: `nix develop /workspace --command cargo build --manifest-path host/Cargo.toml`
- Lint: `nix develop /workspace --command cargo clippy --manifest-path host/Cargo.toml`
- Test: `nix develop /workspace --command cargo test --manifest-path android/Cargo.toml --lib` (unit tests live in the `sleek`/android crate)
- Run GUI: `SLEEK_CODESPACE=1 nix develop /workspace --command just host`
  - The VM has a TigerVNC X server on **`DISPLAY=:1`** (view via noVNC / the
    Desktop pane). There is **no GPU**, so software GL is required. Setting
    `SLEEK_CODESPACE=1` makes the `justfile` export `DISPLAY=:1` and
    `LIBGL_ALWAYS_SOFTWARE=1` and apply `SLEEK_LD_LIBRARY_PATH` to
    `LD_LIBRARY_PATH` automatically. `nix run .#host` also works (it sets the
    same env itself).
- The `sleek` binary is large (~800 MB debug) and the `host/target` cache
  persists in the image, so incremental rebuilds are fast.

### Hello-world / smoke flow
Launch the host, click **Continue as guest** (defaults: nick `sleekXXXX`, server
`irc.freeq.at:6697` TLS), which auto-joins `#general` and `#test`; then open a
channel and send a message. Outbound network to `irc.freeq.at` works in this
environment.

### Secrets (OpenBao)
Cursor env secret `OPENBAO_TOKEN` unlocks `https://openbao.boxd.sh` (KV
`secret/data/ai-api-keys`). Bootstrap runs `scripts/configure-gh-from-openbao.sh`
when `GH_TOKEN` is present there so `gh` uses a real PAT instead of the limited
Cursor integration token (needed for Actions secrets, etc.).

```bash
# One-time, from a machine already logged into gh:
export OPENBAO_TOKEN=…   # or use Cursor env
./scripts/openbao-put-key.sh GH_TOKEN --from-gh
./scripts/configure-gh-from-openbao.sh
printf '%s' "$OPENBAO_TOKEN" | gh secret set OPENBAO_TOKEN -R codegod100/sleek
```

### Buildkite (baogui reference)
Org `nandi`, Default cluster, hosted queue `auto`. Reference pipeline:
[baogui-aopjch](https://buildkite.com/nandi/baogui-aopjch). Sleek pipeline:
[sleek-5u9xbr](https://buildkite.com/nandi/sleek-5u9xbr) (created via API; steps in
`.buildkite/pipeline.yml`).

**Clone source is Radicle Garden** (not GitHub), same shape as baogui:

- RID: [`rad:z9mjPzpVK472QXaaP1picc5U9xBR`](https://nandi.radicle.garden/rad:z9mjPzpVK472QXaaP1picc5U9xBR)
- Git URL: `https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git`
- Provider: private (no GitHub webhooks — trigger via Buildkite API / schedule /
  Radicle CI adapter)

API token is `BUILDKITE_API_KEY` in OpenBao (same KV). Agent skill:
`.cursor/skills/configure-buildkite/`. Radicle **CI** signing keys for
publishing patches live under OpenBao `secret/data/radicle` (fields
`RADICLE_SECRET_KEY`, optional `RADICLE_PUBLIC_KEY` / `RAD_PASSPHRASE`) and/or
matching Buildkite cluster secrets — loaded by
`scripts/buildkite/bootstrap.sh` for the issue→agent step. Bootstrap hydrates
`$RAD_HOME/storage` from the Garden HTTPS git URL (same clone Buildkite uses),
so agents do **not** need egress to Garden p2p `:58019`; public seeds
(`rosa`/`iris` `:8776`) are used for connect/announce. Use a dedicated CI
identity (not a personal DID); never commit key material.

Cluster secrets (soft-loaded; artifacts skip if missing): `NIXBUILD_TOKEN`
and/or `OPENBAO_TOKEN`. Issue agent also needs `CURSOR_API_KEY` +
`RADICLE_SECRET_KEY`. Scope `NIXBUILD_TOKEN` to `pipeline_slug: sleek-5u9xbr` (and
`baogui-aopjch`). Helper: `scripts/ci-nixbuild.sh`. Check materializes flake.lock–
pinned `vidya`/`freeq` via `scripts/sync-flake-path-deps.sh` and runs `cargo`
inside `nix develop` (mold + libs from the flake, not apt).

```bash
eval "$(./scripts/configure-buildkite-from-openbao.sh)"
./scripts/configure-buildkite-from-openbao.sh --check

# Trigger a build of the Radicle tip (or a patch commit once synced):
# curl -X POST -H "Authorization: Bearer $BUILDKITE_API_TOKEN" \
#   -H "Content-Type: application/json" \
#   -d '{"commit":"HEAD","branch":"main"}' \
#   https://api.buildkite.com/v2/organizations/nandi/pipelines/sleek-5u9xbr/builds
```
