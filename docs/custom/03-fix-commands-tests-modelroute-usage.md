# [03] Fix commands_tests ModelRoute literals missing `usage`

## Requirement

`cargo test -p jcode` must compile. On the v0.84.0 upstream base
(`e09acaa7a`), `src/cli/commands_tests.rs` fails to build:

`missing field 'usage' in initializer of ModelRoute`

Upstream added `ModelRoute::usage` in `ff329733f` without updating five
struct literals in `commands_tests.rs`. The breakage exists upstream at
that base (and is still present on current upstream master) — this is
not a fork regression.

## Behavior

Adds `usage: None` to the five `ModelRoute` literals in
`src/cli/commands_tests.rs`. Test-only change; no runtime behavior.

## Config surface

None.

## Affected files

- `src/cli/commands_tests.rs`

## Upstream status

Pure upstream fix — PR candidate. Once upstream fixes the literals
this patch can be dropped.
