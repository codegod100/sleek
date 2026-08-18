---
name: analyze-build
description: >
  Inspect BuildBuddy remote-run builds triggered by proxy.latha.org (the
  Tangled webhook -> BuildBuddy -> R2 artifact pipeline in
  cloudflare/proxy-latha-org/). Use when asked to check build status, read
  build logs, debug a failed/stuck BuildBuddy invocation, or when
  invocationId, GetInvocation, GetLog, or "bb" CLI are mentioned.
---

# Analyze BuildBuddy builds

Pipeline: push to `tangled.org/nandi.uk/sleek` (main) -> Worker webhook ->
`POST /api/v1/Run` on BuildBuddy -> executor self-installs the buck2 client
(musl static binary, no nix/toolchain install needed) and runs
`buck2 build //:sleek-android-apk` -> the actual compile happens on
BuildBuddy's own RE cluster via the repo's `platforms/defs.bzl` custom
`sleek-rbe` image (pixi + Android NDK baked in), with real BuildBuddy
action-cache reuse -> executor PUTs `sleek.apk` back to the Worker -> stored
in R2, served at `https://proxy.latha.org/artifacts/<sha>/...`. Not Nix
anymore — see git history for the abandoned flake.nix/nix-daemon path.
See `cloudflare/proxy-latha-org/README.md` for the full design.

## Push -> watch briefly -> if it doesn't start, just push again

After a push, poll `https://proxy.latha.org/artifacts/<sha>/invocation.json`
for ~1-2 minutes (a few 5s probes). If an `invocationId` shows up but
`GetInvocation` stays `{}`/null for a couple more minutes, that's the
registration bug below — don't keep waiting on it, it's probably just
bollocked. Make a trivial retry push (`git commit --allow-empty -m retry`,
`git push tangled main`) and watch the new invocation instead. This has
consistently been faster than waiting out the bug.

## Find the invocation ID for a push

The Worker records it (no commit-sha search exists in BuildBuddy's API for
ad-hoc runs, so this is the only lookup path):

```bash
sha=$(git rev-parse HEAD)
curl -s "https://proxy.latha.org/artifacts/$sha/invocation.json"
# {"triggeredAt":"...","invocationId":"<id>"}
```

A few seconds' delay after the push is normal — the Worker's webhook handler
responds to Tangled immediately and fires the BuildBuddy trigger via
`ctx.waitUntil` in the background.

## Read build logs — prefer the `bb` CLI over raw REST

`~/bazel-5.0.436-linux-x86_64` is BuildBuddy's `bb` CLI (a bazel-shaped
binary; `bb --help` for the full command list). For reading logs it is
**much better than calling `GetLog` over REST directly** — `GetLog`'s
`nextPageToken` pagination doesn't actually work (page 2 comes back empty
even when the log is clearly longer), so raw REST only ever gets you the
first ~4-11KB. `bb view` streams the whole thing, live if the build is
still running:

```bash
export BUILDBUDDY_API_KEY=SVF7of2NDG3tjXJa00aN   # same key as .buckconfig.local
~/bazel-5.0.436-linux-x86_64 view <invocation-id>       # full log, follows if still running
~/bazel-5.0.436-linux-x86_64 view <invocation-id> | tail -50   # just the latest
```

Other useful `bb` subcommands: `bb execution` (remote execution details),
`bb ui` (interactive TUI), `bb download` (fetch build artifacts).

### `bb explain` — AI-assisted failure analysis

For a failed invocation, `bb explain` sends the build's logs/profile to
BuildBuddy's AI analyzer and returns a plain-English root-cause explanation
instead of you having to manually scroll a huge log:

```bash
export BUILDBUDDY_API_KEY=SVF7of2NDG3tjXJa00aN
~/bazel-5.0.436-linux-x86_64 explain <invocation-id>
```

Reach for this first on a failed build — it's faster than eyeballing a
multi-thousand-line Rust/Nix log, and catches root causes (e.g. "disk full
during compile" vs. a downstream symptom) that a `tail` of the log might
miss. Fall back to `bb view` for the raw log when you need the exact
error text/line it's summarizing.

## Raw REST fallback (status/exit-code checks, not full logs)

```bash
curl -s -X POST "https://app.buildbuddy.io/api/v1/GetInvocation" \
  -H "x-buildbuddy-api-key: $BUILDBUDDY_API_KEY" -H "content-type: application/json" \
  -d "{\"selector\":{\"invocationId\":\"$id\"}}" \
  | jq -c '.invocation[0] | {invocationStatus, success, bazelExitCode, durationUsec, host}'
```

`invocationStatus` goes `PARTIAL_INVOCATION_STATUS` (running) ->
`COMPLETE_INVOCATION_STATUS` (done; check `bazelExitCode`). A from-scratch
build takes ~10-15 minutes.

## Known BuildBuddy-side gotchas (confirmed, not theoretical)

- **Intermittent invocation-registration bug**: occasionally a `Run` call
  returns `HTTP 200` + a normal-looking `invocationId`, is confirmed
  genuinely scheduled (blocking `"async":false` takes a real ~2s round
  trip), but `GetInvocation` stays `{}` and `GetLog` says
  `rpc error: code = NotFound desc = invocation not found` **forever** —
  not a propagation delay, the invocation record is just never created.
  No fix found; it clears on its own on an unpredictable timescale (once
  took ~20+ min across 3 separate probes, another time cleared between two
  probes ~10 min apart). If a triggered build never registers within a
  couple minutes, fire a trivial manual probe
  (`{"steps":[{"run":"echo probe"}]}`) to check whether it's a
  pipeline-specific problem or this general flakiness before assuming a
  code bug. That build's result is unrecoverable — retry with a new commit,
  don't wait on the same invocation ID.
- **buck2's `gnu` release binary needs a newer glibc than the executor
  has**: the trigger executor image is Ubuntu 20.04 focal (glibc 2.31);
  the `buck2-x86_64-unknown-linux-gnu` "latest" release build requires
  glibc >=2.32 (up to 2.39), so it fails instantly at `buck2 --version`
  with `GLIBC_2.3x not found` and the invocation completes in ~12s with
  no artifact. Confirmed via `bb view` on a real failed invocation. Fixed
  by installing the statically-linked `buck2-x86_64-unknown-linux-musl`
  release instead (no glibc dependency at all) — baked into `worker.js`'s
  `buildScript()`. A local `buck2 build` test won't catch this if your
  dev machine's glibc happens to be newer — always verify a *fresh*
  end-to-end run, not just a local one.
- No disk-size override or Nix substituter/GC-root workarounds are needed
  anymore — those were specific to the old Nix-based pipeline (see git
  history for `flake.nix`/nix-daemon and `scripts/ci-nixbuild.sh`, since
  removed from the active path). buck2 compiles on BuildBuddy's own RE
  cluster via `platforms/defs.bzl`'s `sleek-rbe` container image; the
  trigger executor itself only holds the repo checkout + buck2 client +
  the final ~20MB apk, so default disk (~22G) is plenty.

## If a build fails

1. `bb view <id>` for the full log — look for the actual error near the
   end, not just `bazelExitCode: Failed`. A failure within ~15s of the
   invocation starting means it never got to `buck2 build` at all (glibc
   mismatch, download failure, etc.) — check the very top of the log.
2. `df -h /` lines are printed as checkpoints around the buck2 build step,
   in case disk is ever a factor again (unlikely now — see above).
3. Cross-check `host` from `GetInvocation` — if two failures share a host
   within a short window, suspect executor-level contention/leftover state
   over a code bug.
