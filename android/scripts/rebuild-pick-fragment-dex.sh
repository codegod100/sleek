#!/usr/bin/env bash
# Compatibility wrapper — rebuilds pick_fragment.dex + sleek_activity.dex.
exec "$(cd "$(dirname "$0")" && pwd)/rebuild-java-dex.sh"
