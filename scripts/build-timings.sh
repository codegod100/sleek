#!/usr/bin/env bash
# Cold (+ optional incremental) host build with cargo --timings.
# Usage:
#   ./scripts/build-timings.sh            # cold rebuild + report
#   ./scripts/build-timings.sh --keep     # do not cargo clean first
#   ./scripts/build-timings.sh --incremental
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

keep=0
incremental=0
for arg in "$@"; do
  case "$arg" in
    --keep) keep=1 ;;
    --incremental) incremental=1 ;;
    -h|--help)
      sed -n '2,7p' "$0"
      exit 0
      ;;
    *)
      echo "unknown arg: $arg" >&2
      exit 2
      ;;
  esac
done

if [[ -z "${SLEEK_NIX_SHELL:-}" ]]; then
  echo "error: run inside the flake shell (nix develop / ./scripts/enter)" >&2
  exit 1
fi

jobs="${CARGO_BUILD_JOBS:-$(nproc)}"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
outdir="${SLEEK_TIMINGS_DIR:-/tmp/sleek-build-timings}"
mkdir -p "$outdir"

if [[ "$keep" -eq 0 ]]; then
  echo "cargo clean (host)" >&2
  cargo clean --manifest-path host/Cargo.toml
fi

echo "cold build start $(date -Is) jobs=$jobs" >&2
SECONDS=0
cargo build --manifest-path host/Cargo.toml --timings "-j${jobs}"
cold=$SECONDS
echo "cold build finished in ${cold}s" >&2

report="$(ls -1t host/target/cargo-timings/cargo-timing-*.html 2>/dev/null | head -1 || true)"
if [[ -n "${report:-}" ]]; then
  cp "$report" "$outdir/cold-${stamp}.html"
  cp host/target/cargo-timings/cargo-timing.html "$outdir/cold-${stamp}-latest.html"
  echo "timings: $outdir/cold-${stamp}.html" >&2
fi

if [[ -f host/target/debug/sleek ]]; then
  ls -lh host/target/debug/sleek | tee "$outdir/cold-${stamp}-binary.txt" >&2
fi

if [[ "$incremental" -eq 1 ]]; then
  touch android/src/lib.rs
  echo "incremental rebuild start $(date -Is)" >&2
  SECONDS=0
  cargo build --manifest-path host/Cargo.toml "-j${jobs}"
  echo "incremental rebuild finished in ${SECONDS}s" >&2
  echo "cold=${cold}s incremental=${SECONDS}s" | tee "$outdir/cold-${stamp}-summary.txt" >&2
else
  echo "cold=${cold}s" | tee "$outdir/cold-${stamp}-summary.txt" >&2
fi
