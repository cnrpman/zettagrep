#!/bin/sh
set -eu

# Usage:
#   scripts/probe-db-cache.sh PATH [--limit N] [--json]
#
# Examples:
#   scripts/probe-db-cache.sh .
#   scripts/probe-db-cache.sh . --limit 20 --json
#   scripts/probe-db-cache.sh resources/sample-vaults/ripgrep-14.1.1
#
# Notes:
#   - PATH should be an indexed root or a path inside an indexed root.
#   - --limit controls how many top files / shared chunks are shown.

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

exec cargo run --quiet -- dev probe db-cache "$@"
