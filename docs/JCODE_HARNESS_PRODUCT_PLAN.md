# jcode-harness Product Engineering Plan

This fork is not a patch set of small upstream improvements. It is a new `jcode-harness` product direction built on top of jcode internals, with a different harness-first workflow, embedded skills, deterministic quality gates, and scriptable automation.

## Product thesis

`jcode-harness` should be a standalone CLI/runtime for rigorous AI engineering workflows:

- offline-first embedded skills and skill routing;
- deterministic project initialization and governance artifacts;
- scriptable `run`, `smoke`, `skills`, and quality-gate commands;
- high-confidence local testing before any production claim;
- clear compatibility boundaries with upstream jcode.

## Engineering principles

1. **Plan before broad implementation**
   - Keep architecture, CLI contracts, skill precedence, and quality gates documented.
   - Every new pillar needs acceptance criteria before implementation.

2. **Harness-first, not UI-first**
   - Prefer scriptable commands, stable outputs, and deterministic tests.
   - TUI behavior may reuse upstream jcode, but harness workflows must stand alone.

3. **Offline and reproducible by default**
   - Built-in skills must not require network or marketplace installation.
   - Tests should isolate `JCODE_HOME`, cwd, runtime dirs, and provider behavior.

4. **Production quality gates**
   - Add regression tests for every CLI contract and precedence rule.
   - Use `cargo fmt`, targeted tests, broader checks, and selfdev build before commits.
   - Record known gaps in `docs/SKILLS_HARNESS_STATUS.md` until closed.

5. **Clear fork identity**
   - Preserve upstream compatibility when useful, but do not block harness-specific architecture on upstream defaults.
   - New behavior should be documented as `jcode-harness` product behavior.

## Current pillars

See `docs/SKILLS_HARNESS.md` and `docs/SKILLS_HARNESS_STATUS.md` for implemented pillars and validation snapshots.

Release readiness is governed by `docs/JCODE_HARNESS_RELEASE_GATES.md`. Automation-facing JSON contracts are documented in `docs/JCODE_HARNESS_JSON_SCHEMAS.md`. The interactive bootstrap model for `/init` is documented in `docs/JCODE_HARNESS_INIT_SWARM.md`. A change is not considered production-ready unless the relevant gates have objective evidence.

## Next planning milestones

1. Define the `jcode-harness` CLI contract as a stable public interface.
2. Expand clean-code rule fixtures and document rule severity policy.
3. Add end-to-end live `/init` swarm smoke once UI/provider automation can verify full swarm completion safely.
4. Add opt-in live-provider smoke tests after mock-provider JSON/NDJSON contracts are stable.
5. Add release-note template for upstream divergence and harness-specific behavior.
