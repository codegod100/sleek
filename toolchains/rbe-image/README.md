# sleek RBE worker image

Extends BuildBuddy's stock `rbe-ubuntu22-04` executor image with pixi and
the Android NDK, so `cargo.bzl`'s `cargo_genrule` targets (`sleek-host`,
`sleek-android-lib` — still real `pixi run -- cargo build`, not buckified
onto the hermetic `third-party/` graph) can run on BuildBuddy's remote
workers instead of being forced local. See `platforms/defs.bzl` for where
it's wired in (`remote_execution_properties.container-image`).

## Rebuilding

Preferred: `.github/workflows/rbe-image.yml` (manual `workflow_dispatch`)
— builds and pushes via GitHub's own runners, which have far better
upload bandwidth to ghcr.io than most dev machines (a local `podman push`
of this image can take the better part of an hour on a constrained link;
GitHub's runners do it in under a minute). Trigger with:

```sh
gh workflow run rbe-image.yml --ref <branch-with-the-workflow-file-and-Containerfile>
```

Note: `workflow_dispatch` workflows must exist on the repo's *default*
branch to be dispatchable at all, even against another ref — if this
workflow file itself isn't on `main` yet, land it there first (its own
small PR/commit is enough; the Containerfile/pixi changes it builds from
can stay on a feature branch and be targeted via `--ref`).

The job's last step prints the digest to update `platforms/defs.bzl`
with — but see the digest note below, since docker/build-push-action
produces an image *index*, not a plain manifest.

Local alternative, build context must be the repo root (needs
`pixi.toml`/`pixi.lock`):

```sh
cd /path/to/sleek
podman build -t ghcr.io/codegod100/sleek-rbe:latest -f toolchains/rbe-image/Containerfile .
podman push ghcr.io/codegod100/sleek-rbe:latest
skopeo inspect docker://ghcr.io/codegod100/sleek-rbe:latest | grep Digest
```

Either way, update `platforms/defs.bzl`'s `container-image` with the new
digest (`docker://ghcr.io/codegod100/sleek-rbe@sha256:...`), **not** a
`:latest` tag reference — confirmed live that BuildBuddy's RE workers can
keep serving a stale previously-pulled image for a mutable tag even well
after a fresh push completes (rebuilding with a real fix, re-pushing, and
re-running the same remote build reproduced the *exact* pre-fix failure,
while the local image was independently verified to have the fix). Pinning
by digest is the only way to be sure a rebuilt image actually takes effect
remotely.

**Digest gotcha**: a GitHub Actions build (docker/build-push-action) comes
out as an OCI image *index* — the real linux/amd64 manifest plus an
auto-attached SLSA provenance/attestation manifest (platform
"unknown/unknown") — not a plain single-platform manifest. Pointing
`platforms/defs.bzl` at the top-level index digest is ambiguous for RE's
single-platform pull; use the amd64 sub-manifest digest instead:

```sh
skopeo inspect --raw docker://ghcr.io/codegod100/sleek-rbe:latest
# → take the digest of the manifests[] entry with platform amd64/linux,
#   not the "unknown/unknown" one.
```

A plain local `podman build` + `push` doesn't produce an index (no
provenance attachment), so `skopeo inspect` (without `--raw`) already
prints the right single digest to use directly in that case.

Rebuild whenever `pixi.toml`/`pixi.lock` changes (the image pre-warms the
pixi env at build time so RE actions don't have to resolve/fetch
conda-forge packages themselves), to bump the Android NDK version
(currently r29, pinned in the `curl` line — keep in sync with
`flake.nix`'s devShell), the pinned `linux-libc-dev` version (currently
noble 6.8.0-139.139 — bump if v4l2r ever needs newer V4L2 kernel structs
than that ships), or the bundletool version (currently 1.17.2 — bump when
google/bundletool cuts a new release; check
https://github.com/google/bundletool/releases).

## Registry visibility

The `ghcr.io/codegod100/sleek-rbe` package must stay **public** (or be
linked to this repo) — BuildBuddy's RE workers pull it anonymously, no
registry credentials are configured in `platforms/defs.bzl`. GitHub's
package visibility can't be flipped via `gh api`/REST (confirmed: no such
endpoint) — use the web UI: Package settings → Danger Zone → Change
visibility.
