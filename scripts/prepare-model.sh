#!/bin/zsh
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/prepare-model.sh <model-source-dir>" >&2
  exit 1
fi

src_dir="$1"
repo_root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
resource_dir="$repo_root/resources/models/bge-small-en-v1.5"
dev_stage_dir="$repo_root/target/share/zg/models"

if [[ ! -d "$src_dir" ]]; then
  echo "error: source directory not found: $src_dir" >&2
  exit 1
fi

mkdir -p "$resource_dir"

required=(
  "tokenizer.json"
  "config.json"
  "special_tokens_map.json"
  "tokenizer_config.json"
)

onnx_found=0
for onnx_name in model_optimized.onnx model.onnx; do
  if [[ -f "$src_dir/$onnx_name" ]]; then
    cp "$src_dir/$onnx_name" "$resource_dir/$onnx_name"
    onnx_found=1
  fi
done

if [[ "$onnx_found" -eq 0 ]]; then
  echo "error: source directory must contain model_optimized.onnx or model.onnx" >&2
  exit 1
fi

for name in "${required[@]}"; do
  if [[ ! -f "$src_dir/$name" ]]; then
    echo "error: missing required file: $src_dir/$name" >&2
    exit 1
  fi
  cp "$src_dir/$name" "$resource_dir/$name"
done

rm -rf "$dev_stage_dir"
mkdir -p "$dev_stage_dir"
cp -R "$resource_dir"/. "$dev_stage_dir"/

echo "prepared bundled model at: $resource_dir"
echo "staged local development model at: $dev_stage_dir"
