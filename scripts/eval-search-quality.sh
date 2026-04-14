#!/bin/sh
set -eu

# Usage:
#   scripts/eval-search-quality.sh [--fixture FIXTURE.json] [--golden GOLDEN.json] [--vault PATH] [--update-golden] [--json]
#
# Examples:
#   scripts/eval-search-quality.sh --json
#   scripts/eval-search-quality.sh --vault resources/sample-vaults/ripgrep-14.1.1 --golden resources/search-quality/ripgrep-14.1.1.golden.json
#   scripts/eval-search-quality.sh --update-golden --vault resources/sample-vaults/ripgrep-14.1.1
#   ZG_TEST_FAKE_EMBEDDINGS=0 scripts/eval-search-quality.sh --vault /path/to/vault --json
#
# Notes:
#   - This wrapper defaults ZG_TEST_FAKE_EMBEDDINGS=1 for hermetic local evals.
#   - Set ZG_TEST_FAKE_EMBEDDINGS=0 to exercise the real embedding backend.

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

: "${ZG_TEST_FAKE_EMBEDDINGS:=1}"
export ZG_TEST_FAKE_EMBEDDINGS

exec cargo run --quiet -- dev eval search-quality "$@"
