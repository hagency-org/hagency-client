#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
launcher="$root/../hagency-server/scripts/local-dev.py"
if [[ ! -f "$launcher" ]]; then
    echo "Missing sibling hagency-server/scripts/local-dev.py; check out both repositories side by side." >&2
    exit 1
fi
exec python3 "$launcher" "$@"
