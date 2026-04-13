# zg

`zg` is a local-first search CLI for note-heavy directories.

The product contract is simple:

- the default entry point is `zg <query> [path]`
- search comes first; index management is secondary
- regex semantics stay intact for regex-shaped input
- indexed search stays visible and local under `.zg/`

If a query looks like regex, `zg` behaves like grep. If it does not, `zg` uses an explicit `.zg/` index. Indexed search has two levels: `fts` and `fts+vector`.

## Current surface

- `zg <pattern-or-query> [path]`
- `zg grep <pattern> [path]`
- `zg search <query> [path]`
- `zg index init [--level fts|fts+vector] [path]`
- `zg index status [path]`
- `zg index rebuild [--level fts|fts+vector] [path]`
- `zg index delete [path]`

Most users should live on `zg <query> [path]`. The subcommands mainly make the mode explicit.

## Build And Run

`zg` is a standard Rust CLI.

Requirements:

- Rust `1.85` or newer
- Cargo
- `rg` (`ripgrep`) available at runtime, either from `PATH`, `ZG_RG_BIN`, or a bundled binary next to `zg`

Build a debug binary:

```bash
cargo build
```

Build an optimized release binary:

```bash
cargo build --release
```

The release binary is written to:

```text
target/release/zg
```

Run without installing:

```bash
cargo run -- --help
cargo run -- "sqlite adapter" .
cargo run -- index init .
```

Install the CLI into Cargo's bin directory:

```bash
cargo install --path .
zg --help
```

Verify the local build with:

```bash
cargo test
```

## Quick answers

This section keeps short, user-facing answers to the questions most people ask first.

- How do I start using `zg`?
  Use `zg <query> [path]`. In most cases, that is the only command you need.
- Do I need to build an index before searching?
  Regex search works immediately. Plain-text search still requires an explicit `.zg/` index; if none exists, `zg` tells you to run `zg index init`.
- Will `zg` change my files?
  It does not rewrite your documents. When you run `zg index init`, it creates a visible `.zg/` directory for indexing and, for `fts+vector`, may download the embedding model into the normal fastembed / Hugging Face cache.
- Can I remove the index later?
  Yes. Run `zg index delete [path]` to remove the local `.zg/` directory for that scope.
- What search algorithm does the index use?
  `fts` uses SQLite FTS5 only. `fts+vector` uses hybrid recall: SQLite FTS5 handles keyword recall, vector search uses cosine similarity, and results are merged so exact wording and semantic similarity can both surface.
- What embedding model does it use?
  It currently uses `fastembed` with the built-in `ParaphraseMLMiniLML12V2Q` model.
- How does chunking work?
  Chunking is line-based. Each line is a chunk by default, and a line can be split further with the inline marker ` :: `.

## Search semantics

- Regex is ground truth. Regex-shaped input always keeps regex semantics, even inside an indexed tree.
- `zg grep` never requires an index.
- Plain-text search requires an explicit ancestor `.zg/` root.
- `zg search` uses the nearest ancestor `.zg/` root.
- Indexed search still reconciles dirty, changed, new, or deleted content lazily at search time, but it does not create a new `.zg/` root for you.
- `zg index init --level fts` creates a lexical-only index.
- `zg index init --level fts+vector` creates a hybrid lexical + vector index.

## Choosing A Level

- Choose `fts` when your workload is mostly keyword, symbol, identifier, path, or exact-phrase lookup and you want the lowest-latency default.
- Choose `fts+vector` when you want natural-language or semantic recall and are willing to pay more build and query cost.
- Start with `fts` by default. Upgrade later with `zg index rebuild --level fts+vector <path>` when the directory is small enough and semantic recall is worth it.

Examples:

```bash
zg 'TODO|FIXME' .
zg "sqlite adapter" notes/
zg search "meeting notes" docs/
zg index status docs/
```

## Interaction and compatibility

- `zg` is meant to feel familiar to users coming from `grep`, `ripgrep`, `ag`, or `find`.
- There is no separate command to learn before `zg <query> [path]` becomes useful.
- Operational notes stay non-blocking. Search does not pause for setup prompts or interactive maintenance flows.
- The surface stays small. Indexed search is implemented in `zg`; regex search delegates to a runtime `rg` dependency.

## Regex Backend

`zg grep` and the regex-shaped branch of `zg <query>` delegate matching to
`rg`.

`zg` resolves the `rg` binary in this order:

1. `ZG_RG_BIN`
2. a bundled `rg` next to `zg`
3. a bundled `rg` at `../libexec/rg` relative to `zg`
4. `rg` from `PATH`

## Index freshness model

`zg` is lazy-first.

- Does not run a watcher or daemon.
- Search is the sync boundary: the current search scope is reconciled on demand when a search runs.
- Reconcile only touches dirty, new, changed, or deleted content in the requested scope.
- `zg index rebuild` remains the explicit full rebuild path.
- `zg index delete` is the explicit local-cache removal path.

This keeps the CLI simple: users search first, and the system performs the minimum index maintenance needed for that search.

## Index boundary

Running `zg index init --level fts /some/project` or `zg index init --level fts+vector /some/project` creates:

```text
/some/project/.zg/
  index.db
  state.json
```

`index.db` stores file metadata, chunk rows, FTS5 data, vector rows, and index state.

`.zg/` is the visible local partition boundary:

- users can create it explicitly with `zg index init`
- users can remove it explicitly with `zg index delete`
- nested `.zg/` roots are allowed
- overlapping roots are allowed, with the expected tradeoff of more disk usage and slower updates
- search prefers the nearest ancestor `.zg/`

## Index scope and content rules

- Index eligibility is controlled by a document suffix whitelist, a supported code-language whitelist, and an encoding/character whitelist.
- Files with more than `100000` lines are skipped during indexing.
- Symlinks are skipped during indexing and regex scanning.
- Indexed traversal follows ripgrep-style visibility rules:
  parent ignore files, hidden-file filtering, `.ignore`, `.gitignore`, git excludes, and local `.zgignore`.
- `.zg/` is always skipped during traversal.
- Chunking is line-based.
- The inline hard chunk split marker is ` :: `.

Regex traversal is different:

- `zg grep` delegates to `rg`
- local `.zgignore` is not part of the regex-path contract

Supported code symbol extraction languages in the current build:

- Rust
- Python
- JavaScript
- TypeScript / TSX

## Diagnostics

`zg index status [path]` is the human-readable diagnostics surface.

## Developer Evaluation

The repo now carries a fixed high-level evaluation vault and developer probes.

- Sample vault: ripgrep `14.1.1`, fetched into `resources/sample-vaults/.cache/`
- Search-quality fixture: `resources/search-quality/ripgrep-14.1.1.fixtures.json`
- Search-quality golden: `resources/search-quality/ripgrep-14.1.1.golden.json`

Convenience entry points:

```bash
scripts/ensure-sample-vault.sh
scripts/eval-search-quality.sh
scripts/bench-sample-vault.sh
scripts/probe-chunks.sh path/to/file.rs
scripts/probe-db-cache.sh path/to/indexed/root
```

`scripts/eval-search-quality.sh` defaults `ZG_TEST_FAKE_EMBEDDINGS=1` so the
ranking baseline stays deterministic in local runs and CI. The eval path still
uses a hybrid `fts+vector` index. If you want to inspect behavior with the real
embedding backend instead, override that env var before running the script.

`scripts/bench-sample-vault.sh` is the benchmark surface for developers. It
reuses the same query fixture as search-quality evaluation, builds fresh scratch
indexes for both `fts` and `fts+vector`, and prints or writes a structured
timing report.

Examples:

```bash
scripts/bench-sample-vault.sh
scripts/bench-sample-vault.sh --fake-embeddings --json
scripts/bench-sample-vault.sh --repeat 3 --out /tmp/zg-sample-bench.json
```

## Embedding model download

`zg` uses `fastembed` and the built-in `ParaphraseMLMiniLML12V2Q` model for
vector search.

The first indexed search on a machine may download model assets into the normal
fastembed / Hugging Face cache. Exact cache precedence is delegated to the
upstream stack. The main knobs are:

- `HF_HOME`
- `FASTEMBED_CACHE_DIR`
- standard proxy variables such as `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY`
- `HF_ENDPOINT` for a Hugging Face mirror

For local development:

```bash
cargo run -- search "sqlite adapter" .
```

## TODO

- Replace the current brute-force vector retrieval path with an ANN index.
- Parse source code with ASTs, collect identifiers, and feed them into the hybrid BM25 + vector index.
