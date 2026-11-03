# [FEAT-003] Configuration validation is absent

**ID:** FEAT-003
**Category:** feature
**Severity:** low
**File:** crates/kcode-config* (or wherever config is loaded)
**Status:** open

## Description
No explicit config-validation step was found: invalid values appear to be consumed lazily at use
sites rather than rejected up front with a clear message. This makes misconfiguration surface as
cryptic runtime failures (e.g. the canonical-routing "UNKNOWN route" ambiguity recorded for the
fork) instead of a startup error.

## Evidence
- No `validate(`-style config check found while scanning; routing-config ambiguity is documented in
  AGENTS.md ("Forge effective route: UNKNOWN") as a real failure mode.

## Suggested fix
Add a `Config::validate()` invoked at startup that checks required fields, enum-like strings, and
mutually-exclusive options, returning aggregated, human-readable errors. Cover with unit tests for
each invalid case.

## Effort
- M (1-2h)
