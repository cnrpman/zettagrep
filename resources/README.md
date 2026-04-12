# zg Resources

This directory is the repo-local staging area for bundled assets.

The embedding model is no longer bundled here.

`zg` now relies on `fastembed-rs` built-in Hugging Face download support for
the hard-coded `ParaphraseMLMiniLML12V2Q` model.

Useful env vars for local runs:

- `HTTP_PROXY` / `HTTPS_PROXY`
- `FASTEMBED_CACHE_DIR`
- `HF_HOME`
- `HF_ENDPOINT`
