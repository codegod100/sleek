#!/usr/bin/env bash
# Deploy proxy.latha.org via the raw Cloudflare API (no wrangler/node needed).
#
# Creates/updates:
#   - R2 bucket "proxy-latha-org-artifacts" (artifact storage)
#   - Worker script "proxy-latha-org" (module worker, worker.js), with
#     bindings: ARTIFACTS (r2_bucket) + 3 secret_text bindings
#   - Custom domain route proxy.latha.org -> that worker (needs the
#     latha.org zone already on this Cloudflare account)
#
# Required env:
#   CLOUDFLARE_API_TOKEN   Workers Scripts:Edit, Workers Routes:Edit (or
#                          Account:Workers R2 Storage:Edit + Zone:Edit for
#                          DNS on latha.org)
#   CLOUDFLARE_ACCOUNT_ID
#   TANGLED_WEBHOOK_SECRET Paste the same value into Tangled's
#                          Settings -> Hooks -> Secret for this repo.
#   BUILDBUDDY_API_KEY     Org key from https://app.buildbuddy.io/ -> Settings
#   UPLOAD_TOKEN           Bearer token the remote build script uses to PUT
#                          artifacts back to this worker.
#
# Usage:
#   export CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=...
#   export TANGLED_WEBHOOK_SECRET=... BUILDBUDDY_API_KEY=... UPLOAD_TOKEN=...
#   ./deploy.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT_NAME="proxy-latha-org"
BUCKET_NAME="proxy-latha-org-artifacts"
HOSTNAME="proxy.latha.org"
ZONE_NAME="latha.org"
API="https://api.cloudflare.com/client/v4"

: "${CLOUDFLARE_API_TOKEN:?export CLOUDFLARE_API_TOKEN}"
: "${CLOUDFLARE_ACCOUNT_ID:?export CLOUDFLARE_ACCOUNT_ID}"
: "${TANGLED_WEBHOOK_SECRET:?export TANGLED_WEBHOOK_SECRET}"
: "${BUILDBUDDY_API_KEY:?export BUILDBUDDY_API_KEY}"
: "${UPLOAD_TOKEN:?export UPLOAD_TOKEN}"

auth=(-H "Authorization: Bearer $CLOUDFLARE_API_TOKEN")

echo "--- ensure R2 bucket $BUCKET_NAME" >&2
curl -fsS "${auth[@]}" -X POST \
  "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets" \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"$BUCKET_NAME\"}" \
  | jq -c '{success, errors}' || true
# (a 10004 "bucket already exists" error here is fine on re-deploy)

echo "--- upload worker script $SCRIPT_NAME" >&2
metadata="$(jq -n \
  --arg main "worker.js" \
  --arg bucket "$BUCKET_NAME" \
  --arg webhook_secret "$TANGLED_WEBHOOK_SECRET" \
  --arg bb_key "$BUILDBUDDY_API_KEY" \
  --arg upload_token "$UPLOAD_TOKEN" \
  '{
    main_module: $main,
    compatibility_date: "2024-09-23",
    bindings: [
      {type: "r2_bucket", name: "ARTIFACTS", bucket_name: $bucket},
      {type: "secret_text", name: "TANGLED_WEBHOOK_SECRET", text: $webhook_secret},
      {type: "secret_text", name: "BUILDBUDDY_API_KEY", text: $bb_key},
      {type: "secret_text", name: "UPLOAD_TOKEN", text: $upload_token}
    ]
  }')"

curl -fsS "${auth[@]}" -X PUT \
  "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/workers/scripts/$SCRIPT_NAME" \
  -F "metadata=$metadata;type=application/json" \
  -F "worker.js=@$ROOT/worker.js;type=application/javascript+module" \
  | jq -c '{success, errors}'

echo "--- look up zone id for $ZONE_NAME" >&2
zone_id="$(curl -fsS "${auth[@]}" "$API/zones?name=$ZONE_NAME" | jq -r '.result[0].id')"
if [[ -z "$zone_id" || "$zone_id" == "null" ]]; then
  echo "error: zone $ZONE_NAME not found on this Cloudflare account/token" >&2
  exit 1
fi
echo "zone_id=$zone_id" >&2

echo "--- attach custom domain $HOSTNAME" >&2
curl -fsS "${auth[@]}" -X PUT \
  "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/workers/domains" \
  -H "Content-Type: application/json" \
  -d "$(jq -n \
    --arg hostname "$HOSTNAME" \
    --arg service "$SCRIPT_NAME" \
    --arg zone_id "$zone_id" \
    '{hostname: $hostname, service: $service, environment: "production", zone_id: $zone_id}')" \
  | jq -c '{success, errors, result: {hostname: .result.hostname}}'

echo "ok: https://$HOSTNAME/webhook is live" >&2
echo "Next: Tangled repo -> Settings -> Hooks -> new webhook" >&2
echo "  Payload URL: https://$HOSTNAME/webhook" >&2
echo "  Secret: (the TANGLED_WEBHOOK_SECRET value used above)" >&2
echo "  Events: push" >&2
