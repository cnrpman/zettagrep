#!/bin/sh
set -eu

# Usage:
#   scripts/bench-sample-vault.sh [--fixture FIXTURE.json] [--vault PATH] [--repeat N] [--fake-embeddings] [--out FILE] [--keep-scratch] [--json]
#
# Examples:
#   scripts/bench-sample-vault.sh --json
#   scripts/bench-sample-vault.sh --vault resources/sample-vaults/ripgrep-14.1.1 --repeat 3 --fake-embeddings --out /tmp/bench.json
#
# Notes:
#   - If --fixture is omitted, zg uses its default search-quality fixture.
#   - --fake-embeddings is useful for fast, deterministic local runs.

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

exec cargo run --quiet -- dev bench sample-vault "$@"
