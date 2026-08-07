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
- The flake's `nixConfig` advertises the optional `codegod100.cachix.org`
  substituter. `accept-flake-config = false` is set in `/etc/nix/nix.conf` so it
  is silently ignored (no cachix needed). If you invoke nix from an interactive
  TTY (e.g. tmux) and still get a trust prompt, run nix with stdin from
  `</dev/null`.

### Sibling path dependencies (required for working-tree builds)
`android/Cargo.toml` uses path deps `../../vidya` and `../../freeq/freeq-sdk`.
Because the repo root is `/workspace`, those resolve to **`/vidya`** and
**`/freeq`** (filesystem root). Both are pinned in **`flake.lock`** as
`github:codegod100/vidya` and `github:codegod100/freeq`. Materialize siblings
from the lock with:

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
[sleek](https://buildkite.com/nandi/sleek) (GitHub `codegod100/sleek`; steps in
`.buildkite/pipeline.yml`). API token is `BUILDKITE_API_KEY` in OpenBao
(same KV). Agent skill: `.cursor/skills/configure-buildkite/`.

Cluster secrets (soft-loaded; artifacts skip if missing): `NIXBUILD_TOKEN`
and/or `OPENBAO_TOKEN`. Scope `NIXBUILD_TOKEN` to `pipeline_slug: sleek` (and
`baogui-aopjch`). Helper: `scripts/ci-nixbuild.sh`. Check clones flake.lock–
pinned `vidya`/`freeq` from GitHub (not tangled tip).

```bash
eval "$(./scripts/configure-buildkite-from-openbao.sh)"
./scripts/configure-buildkite-from-openbao.sh --check
```
