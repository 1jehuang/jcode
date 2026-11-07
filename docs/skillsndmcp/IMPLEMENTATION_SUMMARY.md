# Implementation summary

This file used to describe a `skill/parser.rs` + `skill/validator.rs` refactor.
That refactor never compiled into the workspace (the modules were never
declared) and was not the goal; it is kept in `git stash@{0}`.

What was actually built: jcode's `/mcp` and `/skills` now work like Claude
Code's, and skills and MCP behave the same way for the model. Details are in
`claude-code-parity.md` (design and evidence), `claude-code-ui-reference.md`
(the captured Claude Code screens) and `../SKILLS_AND_MCP.md` (user guide).
Per-doc status is in `STATUS.md`.
