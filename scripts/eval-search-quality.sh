#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

: "${ZG_TEST_FAKE_EMBEDDINGS:=1}"
export ZG_TEST_FAKE_EMBEDDINGS

exec cargo run --quiet -- dev eval search-quality "$@"
