# zg

`zg` is a local-first search CLI for note-heavy and text-heavy directories.

The product contract is simple:

- the default entry point is `zg <query> [path]`
- search comes first; index management is secondary
- regex semantics stay intact for regex-shaped input
- indexed search stays visible and local under `.zg/`

If a query looks like regex, `zg` behaves like grep. If it does not, `zg` uses hybrid indexed search with lexical and vector signals together.

## Current v1 surface

- `zg <pattern-or-query> [path]`
- `zg grep <pattern> [path]`
- `zg search <query> [path]`
- `zg index init [path]`
- `zg index status [path]`
- `zg index rebuild [path]`
- `zg index delete [path]`

Most users should live on `zg <query> [path]`. The subcommands mainly make the mode explicit.

## Search semantics

- Regex is ground truth. Regex-shaped input always keeps regex semantics, even inside an indexed tree.
- `zg grep` never requires an index.
- Plain-text search uses hybrid recall: lexical and vector signals both participate in retrieval.
- `zg search` uses the nearest ancestor `.zg/` root.
- If no ancestor `.zg/` exists, non-regex search creates a directory-level local index for the current search scope and continues the same request.
- When the search target is a single file and there is no ancestor `.zg/`, `zg` uses the file's parent directory as the index root.
- When search creates `.zg/` implicitly, `zg` tells you where it was created and reminds you that it can be removed later with `zg index delete`.

Examples:

```bash
zg 'TODO|FIXME' .
zg "sqlite adapter" notes/
zg search "meeting notes" docs/
zg index status docs/
```

## Interaction and compatibility

- `zg` is meant to feel familiar to users coming from `grep`, `ripgrep`, `ag`, or `find`.
- There is no separate query language to learn before `zg <query> [path]` becomes useful.
- Operational notes stay non-blocking. Search does not pause for setup prompts or interactive maintenance flows.
- v1 keeps the surface small and ships as a single binary.

## Freshness model

`zg` is lazy-first.

- v1 does not run a watcher or daemon.
- Search is the sync boundary: the current search scope is reconciled on demand when a search runs.
- Reconcile only touches dirty, new, changed, or deleted content in the requested scope.
- Missing work for indexed search is handled on the search path rather than forcing a separate maintenance step.
- `zg index rebuild` remains the explicit full rebuild path.
- `zg index delete` is the explicit local-cache removal path.

This keeps the CLI small: users search first, and the system performs the minimum index maintenance needed for that search.

## Local index boundary

Running `zg index init /some/project` creates:

```text
/some/project/.zg/
  index.db
  state.json
```

`index.db` stores file metadata, chunk rows, FTS5 data, vector rows, and index state.

`.zg/` is the visible local partition boundary:

- users can create it explicitly with `zg index init`
- search can create it implicitly when indexed search needs a local root
- users can remove it explicitly with `zg index delete`
- nested `.zg/` roots are allowed
- overlapping roots are allowed, with the expected tradeoff of more disk usage and slower updates
- search prefers the nearest ancestor `.zg/`

## Index scope and content rules

- Index eligibility is controlled by a suffix whitelist plus an encoding/character whitelist.
- Symlinks are skipped during indexing and regex scanning.
- Walk behavior follows ripgrep-style visibility rules:
  parent ignore files, hidden-file filtering, `.ignore`, `.gitignore`, git excludes, and local `.zgignore`.
- `.zg/` is always skipped during traversal.
- v1 chunking is line-based.
- The inline hard split marker is ` :: `.
- Indexed chunks store both raw text and normalized text.
- Common note decorators like list markers and Markdown headings get light cleanup before normalization.

## Diagnostics

`zg index status [path]` is the human-readable diagnostics surface. Today it reports:

- requested path
- index root
- whether the path is indexed
- chunking mode
- inline marker
- scope policy
- walk policy
- dirty state and dirty reason
- file and chunk counts
- FTS/vector readiness
- last sync time

## Embedding model download

`zg` now relies on `fastembed-rs` built-in model download support for the
hard-coded `ParaphraseMLMiniLML12V2Q` model.

The download/cache path works like this:

1. `HF_HOME` if set
2. otherwise `FASTEMBED_CACHE_DIR` if set
3. otherwise fastembed's default cache directory

Proxy and mirror behavior is delegated to the upstream stack:

- `HTTP_PROXY` / `HTTPS_PROXY`
- lowercase `http_proxy` / `https_proxy`
- `HF_ENDPOINT` for a Hugging Face mirror

Commands:

```bash
scripts/run-local.sh search "sqlite adapter" .
```
