# ratatui-jcode

In-tree fork of [ratatui](https://github.com/ratatui/ratatui) `ratatui-core`
0.1.0 and `ratatui-widgets` 0.3.0 (MIT, see `LICENSE`), wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`.

Purpose: per-frame CPU work in release builds. Profiling the jcode TUI client
showed ~25-35% of busy CPU inside ratatui (`Buffer::diff`, `Cell` clone/eq/reset,
`Paragraph` grapheme splitting and width measurement).

Rules:
- Keep the public API source-compatible with upstream so other crates in the
  graph (ratatui-image, ratatui-crossterm) keep building unmodified.
- Mark every divergence from upstream with a `// ratatui-jcode:` comment so it
  can be rebased or upstreamed.
- Prefer upstreaming generally useful wins.
