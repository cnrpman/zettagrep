# zg

`zg` is a local-first filesystem query engine.

Without an index it behaves like a regex-oriented grep runner.
With an index it uses per-directory SQLite state under `.zg/` and serves lazy-first hybrid recall.

## Current v1 surface

- `zg <pattern-or-query> [path]`
- `zg grep <pattern> [path]`
- `zg search <query> [path]`
- `zg index init [path]`
- `zg index status [path]`
- `zg index rebuild [path]`

## On-disk layout

Running `zg index init /some/project` creates:

```text
/some/project/.zg/
  index.db
  state.json
```

`index.db` stores file metadata, chunk rows, FTS5 data, vector rows, and index state.

## Notes

- `zg grep` does not require an index.
- Regex-shaped input keeps regex semantics even when the target directory is already indexed.
- `zg search` uses the nearest ancestor `.zg/` root; if none exists and the query is not regex-shaped, it lazily initializes a local `.zg/` for that search scope.
- `zg grep` reuses ripgrep-family crates for file walking and regex scanning.
- `zg` does not run a watcher or daemon in v1. Freshness comes from on-demand reconcile during search.
- Indexed search uses hybrid recall with lexical and local vector signals together.

## Embedding Model Path

On macOS, `zg` looks for a bundled local model in this order:

1. `ZG_MODEL_DIR`
2. `<prefix>/share/zg/models`
3. `~/Library/Application Support/zg/models` if it already exists

If no bundled model is found, `zg` fails fast. v1 does not use first-run model
downloads as a fallback path.
