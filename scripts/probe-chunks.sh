#!/bin/sh
set -eu

# Usage:
#   scripts/probe-chunks.sh [--json] PATH
#
# Examples:
#   scripts/probe-chunks.sh docs/r0_product_philosophy.md
#   scripts/probe-chunks.sh --json src/index/files.rs
#
# Notes:
#   - PATH can be a document or code file that zg knows how to chunk.

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

exec cargo run --quiet -- dev probe chunks "$@"
