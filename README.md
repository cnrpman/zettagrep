# zg

`zg` is a small Rust library for query normalization and lightweight matching.

The `0.1.0` release is intentionally narrow: it gives the future `zg` / zettagrep
tooling a real, publishable core instead of an empty placeholder crate.

## What it does today

- trims input and folds repeated whitespace
- lowercases query text for stable matching
- splits normalized terms
- checks whether all query terms occur in a candidate string

## Example

```rust
use zg::{matches_query, Query};

let query = Query::new("  Rust   Search ");
assert_eq!(query.normalized(), "rust search");
assert!(matches_query("rust search", "Rust-powered search tools"));
```

## Scope

This crate is deliberately small in 2026. The goal is to stabilize basic query
handling first, then extend the crate with richer search primitives in later
releases.
