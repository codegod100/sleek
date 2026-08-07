# baogui Buildkite patterns

Reference implementation: `codegod100/baogui` → `.buildkite/pipeline.yml`,
pipeline slug `baogui-aopjch`.

## Step layout

```text
check (cargo clippy + test)
  ├─ flatpak (depends_on check) → artifact org.openbao.baogui.flatpak
  └─ apk (depends_on check)     → artifact baogui.apk (skip if no nixbuild creds)
```

## Check step essentials

- Clone sibling `vidya` next to the checkout if missing (`dirname $PWD/vidya`).
- Install rustup toolchain **1.85** + clippy when absent.
- Install egui system libs via apt when available.
- `cargo clippy -- -D warnings` then `cargo test`.

## Flatpak step essentials

- Hosted agents: run `flatpak-builder` inside privileged
  `ghcr.io/flathub-infra/flatpak-github-actions:freedesktop-25.08`.
- Mount repo and sibling vidya so Cargo path deps resolve (`../../vidya`).
- `chown` artifacts back to the agent user before `buildkite-agent artifact upload`.

## APK / nixbuild step essentials

1. Soft-load `NIXBUILD_TOKEN` / `OPENBAO_TOKEN` via `buildkite-agent secret get`.
2. If only `OPENBAO_TOKEN`: `eval "$(./scripts/fetch-openbao-env.sh --export --keys NIXBUILD_TOKEN)"`.
3. If still empty: warn, print Secrets UI URL, **exit 0** (skip).
4. Export `NIXBUILDNET_TOKEN=$NIXBUILD_TOKEN`.
5. Install Determinate Nix `--init none` if needed; write SSH config for
   `eu.nixbuild.net` authtoken; put builders + trusted keys under `/etc/nix`
   **before** starting nix-daemon.
6. `nix build --store ssh-ng://nixbuild ".#android"`, then
   `nix copy --from ssh-ng://nixbuild` and upload the APK.

## Dollar escaping

In `.buildkite/pipeline.yml` command blocks, shell variables must use `$$`
so Buildkite leaves them for the agent shell:

```yaml
command: |
  set -euo pipefail
  vidya_dir="$$(dirname "$$PWD")/vidya"
  if [[ -z "$${NIXBUILD_TOKEN:-}" ]]; then
    echo "skip"
    exit 0
  fi
```

## What not to do

- Do not publish `target/release/<app>` ELF (Wayland/glibc failures on plain Linux).
- Do not put raw tokens in pipeline YAML or commit them.
- Do not use YAML `secrets:` for optional keys — soft get + skip instead.
- Do not restart/kill nix-daemon with patterns that SIGTERM the Buildkite job.
