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
`POST /api/v1/Run` on BuildBuddy -> executor builds `.#android` + `.#flatpak`
-> PUTs artifacts back to the Worker -> stored in R2, served at
`https://proxy.latha.org/artifacts/<sha>/...`. See
`cloudflare/proxy-latha-org/README.md` for the full design.

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
- **Default executor disk (~22G) is too small** for a from-scratch build of
  this repo. Fixed by requesting a bigger one via `platform_properties` on
  the `Run` request body:
  ```json
  {"platform_properties": {"EstimatedFreeDiskBytes": "60GB"}}
  ```
  Confirmed this actually resizes the VM (`df -h /` inside reported a real
  63G disk with it set, vs. ~22G without). Already baked into
  `cloudflare/proxy-latha-org/worker.js`'s `triggerBuild`.
- **`extra-substituters`/`extra-trusted-public-keys` set via client-side
  `NIX_CONFIG` are silently dropped** by nix-daemon when the invoking user
  isn't a `trusted-user` (the build script runs `nix build` as an
  unprivileged user while the daemon runs as root via `sudo`). Confirmed
  the hard way: `codegod100.cachix.org` already had a needed derivation
  cached (`200` on its `.narinfo`) but the build still compiled it from
  scratch. Fix: write substituter config into `/etc/nix/nix.conf` (the
  daemon's own, inherently-trusted config) as root *before* starting the
  daemon — same pattern as `scripts/ci-nixbuild.sh`'s `setup_nixbuild()`.
  Already baked into `worker.js`.
- **BuildBuddy recycles runners/snapshots** across builds on the same
  branch for speed — meaning stale state (an old `nix-daemon` process, old
  `result-*` out-link symlinks that pin GC roots, prior disk usage) can
  carry over between otherwise-unrelated invocations. `worker.js`'s build
  script now: removes stale out-links before `nix-collect-garbage -d`,
  and restarts the daemon whenever one is already running so config edits
  always take effect.

## If a build fails

1. `bb view <id>` for the full log — look for the actual compiler/linker
   error near the end, not just `bazelExitCode: Failed`.
2. Check `df -h /` lines in the log (the build script prints checkpoints
   around GC and each `nix build` step) to rule disk in/out fast.
3. Cross-check `host` from `GetInvocation` — if two failures share a host
   within a short window, suspect executor-level contention/leftover state
   over a code bug.
