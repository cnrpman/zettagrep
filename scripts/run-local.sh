#!/bin/zsh
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
resource_dir="$repo_root/resources/models/bge-small-en-v1.5"
dev_stage_dir="$repo_root/target/share/zg/models"

if [[ ! -d "$resource_dir" ]]; then
  echo "error: bundled model not prepared: $resource_dir" >&2
  echo "hint: run scripts/prepare-model.sh <model-source-dir>" >&2
  exit 1
fi

rm -rf "$dev_stage_dir"
mkdir -p "$dev_stage_dir"
cp -R "$resource_dir"/. "$dev_stage_dir"/

cd "$repo_root"
exec cargo run -- "$@"
