#!/usr/bin/env bash
# Rebuild assets/vidya-symbols.ttf from DejaVu Sans (pyftsubset).
#
# Keeps the font small while covering glyphs Ubuntu Light lacks:
#   - HIG-style UI punctuation (arrows, bullets, disclosure triangles, quotes)
#   - Common math/prose Unicode LLMs emit outside $…$ (ℝ, ⁿ, ∑, Greek, …)
# On Android / egui those otherwise render as hollow boxes (“tofu”).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/assets/vidya-symbols.ttf"

find_dejavu() {
  if [[ -n "${DEJAVU_SANS:-}" && -f "$DEJAVU_SANS" ]]; then
    echo "$DEJAVU_SANS"
    return
  fi
  local candidates=(
    /nix/store/*-dejavu-fonts-*/share/fonts/truetype/DejaVuSans.ttf
    /run/current-system/sw/share/fonts/truetype/DejaVuSans.ttf
    /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf
    /usr/share/fonts/TTF/DejaVuSans.ttf
  )
  # shellcheck disable=SC2068
  for f in ${candidates[@]}; do
    # Prefer the Book/Regular face — Condensed lacks some math glyphs.
    if [[ -f "$f" && "$(basename "$f")" == "DejaVuSans.ttf" ]]; then
      echo "$f"
      return
    fi
  done
  for f in ${candidates[@]}; do
    if [[ -f "$f" ]]; then
      echo "$f"
      return
    fi
  done
  # fc-list fallback (file path only; avoid Condensed)
  if command -v fc-list >/dev/null 2>&1; then
    fc-list -f '%{file}\n' "DejaVu Sans:style=Book" 2>/dev/null \
      | grep -E 'DejaVuSans\.ttf$' | head -1
    return
  fi
  return 1
}

DEJAVU="$(find_dejavu || true)"
if [[ -z "$DEJAVU" || ! -f "$DEJAVU" ]]; then
  echo "error: DejaVu Sans not found; set DEJAVU_SANS=/path/to/DejaVuSans.ttf" >&2
  exit 1
fi

if ! command -v pyftsubset >/dev/null 2>&1; then
  echo "error: pyftsubset not on PATH (nix-shell -p python3Packages.fonttools)" >&2
  exit 1
fi

# Keep in sync with assets/NOTICE.
# Ranges: Greek, general punctuation (incl. thin/ZWSP), letterlike, arrows,
# operators, brackets, math alphanumerics; singletons for UI shapes/checks.
UNICODES=$(
  cat <<'EOF' | tr '\n' ',' | sed 's/,$//'
U+00B0
U+00B1
U+00B2
U+00B3
U+00B7
U+00B9
U+00D7
U+00F7
U+0370-03FF
U+2000-206F
U+2070-209F
U+2100-214F
U+2190-21FF
U+2200-22FF
U+27C0-27EF
U+2980-299F
U+25A0
U+25A1
U+25B2
U+25B4
U+25B6
U+25B8
U+25BA
U+25BC
U+25BE
U+25C0
U+25C2
U+25C4
U+25CB
U+25CF
U+2713
U+2714
U+2715
U+2717
U+1D400-1D7FF
EOF
)

echo "source: $DEJAVU"
echo "output: $OUT"
pyftsubset "$DEJAVU" \
  --unicodes="$UNICODES" \
  --layout-features='' \
  --glyph-names \
  --symbol-cmap \
  --legacy-cmap \
  --notdef-glyph \
  --notdef-outline \
  --recommended-glyphs \
  --name-IDs='*' \
  --name-legacy \
  --name-languages='*' \
  --output-file="$OUT"

echo "wrote $(wc -c <"$OUT") bytes"
