#!/bin/zsh
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "$0")/.." && pwd)"

cd "$repo_root"
exec cargo run -- "$@"
