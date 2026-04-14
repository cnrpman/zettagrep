# Homebrew Distribution Notes

Status: Draft  
Date: 2026-04-12

## Goal

Ship `zg` on macOS via Homebrew for both Apple Silicon and Intel while keeping
`ripgrep` as a first-class runtime dependency for the whole search surface and
keeping regex-path behavior aligned with upstream `rg`.

## Runtime Dependency Policy

- `zg` depends on `ripgrep` at runtime for both regex search and indexed search
- Homebrew formula should declare `depends_on "ripgrep"`
- `zg` itself also supports a bundled `rg` binary for standalone tarball-style distributions

## rg Resolution Order

At runtime, `zg` looks for `rg` in this order:

1. `ZG_RG_BIN`
2. sibling binary: `<zg-dir>/rg`
3. bundled helper path: `<zg-prefix>/libexec/rg`
4. `PATH`

This lets us support both:

- Homebrew dependency installs
- bundled release archives

## Formula

The repo now includes a Homebrew formula at:

- `pkg/brew/zg.rb`

It is currently a `head` formula and already declares:

- build dependency: `rust`
- runtime dependency: `ripgrep`

## Packaging Notes

- Homebrew bottles are built per architecture; do not try to make one universal bottle by hand
- `depends_on "ripgrep"` is the correct Homebrew-level declaration for both Intel and Apple Silicon because `rg` is required for the full search surface, not only the regex path
- If we also ship standalone tarballs, copy `rg` beside `zg` or into `libexec/rg`
