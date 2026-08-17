# sleek RBE worker image

Extends BuildBuddy's stock `rbe-ubuntu22-04` executor image with pixi and
the Android NDK, so `cargo.bzl`'s `cargo_genrule` targets (`sleek-host`,
`sleek-android-lib` — still real `pixi run -- cargo build`, not buckified
onto the hermetic `third-party/` graph) can run on BuildBuddy's remote
workers instead of being forced local. See `platforms/defs.bzl` for where
it's wired in (`remote_execution_properties.container-image`).

## Rebuilding

Build context must be the repo root (needs `pixi.toml`/`pixi.lock`):

```sh
cd /path/to/sleek
podman build -t ghcr.io/codegod100/sleek-rbe:latest -f toolchains/rbe-image/Containerfile .
podman push ghcr.io/codegod100/sleek-rbe:latest
skopeo inspect docker://ghcr.io/codegod100/sleek-rbe:latest | grep Digest
```

Then update `platforms/defs.bzl`'s `container-image` with that digest
(`docker://ghcr.io/codegod100/sleek-rbe@sha256:...`), **not** a `:latest`
tag reference — confirmed live that BuildBuddy's RE workers can keep
serving a stale previously-pulled image for a mutable tag even well after
a fresh push completes (rebuilding with a real fix, re-pushing, and
re-running the same remote build reproduced the *exact* pre-fix failure,
while the local image was independently verified to have the fix). Pinning
by digest is the only way to be sure a rebuilt image actually takes effect
remotely.

Rebuild whenever `pixi.toml`/`pixi.lock` changes (the image pre-warms the
pixi env at build time so RE actions don't have to resolve/fetch
conda-forge packages themselves) or to bump the Android NDK version
(currently r29, pinned in the `curl` line — keep in sync with
`flake.nix`'s devShell).

## Registry visibility

The `ghcr.io/codegod100/sleek-rbe` package must stay **public** (or be
linked to this repo) — BuildBuddy's RE workers pull it anonymously, no
registry credentials are configured in `platforms/defs.bzl`. GitHub's
package visibility can't be flipped via `gh api`/REST (confirmed: no such
endpoint) — use the web UI: Package settings → Danger Zone → Change
visibility.
