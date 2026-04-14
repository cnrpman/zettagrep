#!/bin/sh
set -eu

# Usage:
#   scripts/ensure-sample-vault.sh [--manifest MANIFEST.json] [--force] [--json]
#
# Examples:
#   scripts/ensure-sample-vault.sh
#   scripts/ensure-sample-vault.sh --manifest resources/sample-vaults/ripgrep-14.1.1.json --json
#   scripts/ensure-sample-vault.sh --force
#
# Notes:
#   - Without --manifest, zg uses its default sample-vault manifest.
#   - --force re-clones when the checkout exists but is at the wrong commit.

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

exec cargo run --quiet -- dev sample-vault ensure "$@"
