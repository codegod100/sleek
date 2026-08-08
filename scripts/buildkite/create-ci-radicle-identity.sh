#!/usr/bin/env bash
# Create a *dedicated* Radicle CI identity and print upload instructions.
# Does not touch your personal ~/.radicle. Never commit the private key.
#
# Usage:
#   ./scripts/buildkite/create-ci-radicle-identity.sh
#   ./scripts/buildkite/create-ci-radicle-identity.sh --alias sleek-ci
#
# Then store the printed fields as Buildkite cluster secrets and/or:
#   OPENBAO_SECRET_PATH=secret/data/radicle ./scripts/openbao-put-key.sh RADICLE_SECRET_KEY --value "$(cat …)"
set -euo pipefail

ALIAS="sleek-ci"
OUT_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --alias) ALIAS="$2"; shift 2 ;;
    --out) OUT_DIR="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if ! command -v rad >/dev/null 2>&1; then
  echo "rad not on PATH — install with: curl -fsSL https://radicle.xyz/install | sh" >&2
  exit 1
fi

OUT_DIR="${OUT_DIR:-$(mktemp -d /tmp/sleek-ci-radicle.XXXXXX)}"
mkdir -p "$OUT_DIR"
export RAD_HOME="$OUT_DIR"

# Empty passphrase (seeder-style). Ephemeral agents use RAD_PASSPHRASE="" + MemorySigner.
printf '\n' | rad auth --alias "$ALIAS" --stdin >/dev/null

DID="$(rad self --did)"
PUB="$(cat "$RAD_HOME/keys/radicle.pub")"

umask 077
cp "$RAD_HOME/keys/radicle" "$OUT_DIR/RADICLE_SECRET_KEY.pem"
cp "$RAD_HOME/keys/radicle.pub" "$OUT_DIR/RADICLE_PUBLIC_KEY.pub"
chmod 600 "$OUT_DIR/RADICLE_SECRET_KEY.pem"

cat <<EOF
Created dedicated CI identity (not your personal node):
  alias: $ALIAS
  did:   $DID
  home:  $RAD_HOME
  key:   $OUT_DIR/RADICLE_SECRET_KEY.pem

Buildkite (Default cluster → Secrets, scope pipeline sleek-5u9xbr if possible):
  RADICLE_SECRET_KEY  = contents of RADICLE_SECRET_KEY.pem (multiline PEM)
  RADICLE_PUBLIC_KEY  = $PUB
  RAD_PASSPHRASE      = (leave unset / empty)

OpenBao (optional; bootstrap also soft-loads these when OPENBAO_TOKEN is set):
  OPENBAO_SECRET_PATH=secret/data/radicle \\
    ./scripts/openbao-put-key.sh RADICLE_SECRET_KEY < $OUT_DIR/RADICLE_SECRET_KEY.pem
  OPENBAO_SECRET_PATH=secret/data/radicle \\
    ./scripts/openbao-put-key.sh RADICLE_PUBLIC_KEY --value "$PUB"
  # If secret/radicle does not exist yet, create it first (bao kv put / UI).

After keys are in Buildkite, re-run an issue-agent build. bootstrap.sh will
install rad, materialize \$RAD_HOME, start the node, and \`rad init --existing\`
so \`git push rad HEAD:refs/patches\` works from the Garden HTTPS checkout.

Delete $OUT_DIR when secrets are stored.
EOF
