# zg Resources

This directory is the repo-local staging area for bundled assets.

For the current macOS-first build, the only required asset is the bundled
embedding model directory:

`resources/models/bge-small-en-v1.5/`

Expected files:

- `model.onnx` or `model_optimized.onnx`
- `tokenizer.json`
- `config.json`
- `special_tokens_map.json`
- `tokenizer_config.json`

Suggested workflow:

1. Download or prepare the model somewhere outside the repo.
2. Run `scripts/prepare-model.sh <source-dir>` to copy it into `resources/` and stage it for local development.
3. Run `scripts/run-local.sh ...` to launch the local binary with the staged resources.
