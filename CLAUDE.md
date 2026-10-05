# Repository Instructions

@AGENTS.md

Follow `AGENTS.md` for the development workflow and `CONTRIBUTING.md` for the
contribution policy. Pull requests from everyone are welcome for direct review
and merging, regardless of contributor or maintainer status. Preserve unrelated
work and obtain user authorization before merging.

`@AGENTS.md` is included above, so the **Project Isolation Invariants** section
applies here in full. In short: one daemon serves many projects, so `working_dir:
None` must never resolve against the daemon's cwd, new global state must be keyed
by project or documented as deliberately shared, client-supplied session ids must
be checked against the connection's own session, and cross-project reads must be an
explicit opt-in. Backlog:
`docs/plans/PROJECT_ISOLATION_HARDENING_PLAN.md`.
