# jcode-harness Release Notes Template

Use this template for every jcode-harness release candidate. It makes intentional fork behavior explicit, separates upstream-compatible changes from harness-specific divergence, and records validation evidence before publishing.

> Copy this file into the release note draft. Replace bracketed placeholders and remove sections that truly do not apply.

## Release: `[version]`

- Date: `[YYYY-MM-DD]`
- Branch/commit: `[branch @ short-hash]`
- Upstream base: `[upstream remote/branch @ short-hash]`
- Release owner: `[name/session]`
- Governance gate: `[PASS/FLAG/BLOCK + link/path]`

## Summary

`[Two to five sentences describing the release outcome, who should upgrade, and the main operational impact.]`

## Harness CLI changes

- `[Command or behavior]`: `[what changed and why]`
- JSON/NDJSON compatibility: `[additive fields only / migration note / none]`
- Offline/safe-eval behavior: `[confirmed unchanged / changed with details]`

## Embedded skills and routing changes

- Built-in skills: `[added/removed/changed; include names]`
- Skill precedence/routing: `[changed or unchanged]`
- Project-local override compatibility: `[changed or unchanged]`
- LLM wiki memory behavior: `[changed or unchanged; include permission boundaries]`

## Quality gate changes

- Clean Code Guardian: `[rules/thresholds/output changed or unchanged]`
- Release gates: `[new/removed/changed gates]`
- Schema docs: `[updated files and new contracts]`

## Provider/runtime compatibility

- `jcode run`: `[compatible / migration note]`
- `jcode serve`: `[compatible / migration note]`
- `jcode connect`: `[compatible / migration note]`
- Providers/auth: `[compatible / changed; include sensitive boundary notes]`

## Upstream divergence review

List intentional differences from upstream jcode and why they are kept in this fork.

| Area | Divergence | Rationale | Compatibility risk | Follow-up |
| --- | --- | --- | --- | --- |
| `[area]` | `[harness-specific behavior]` | `[why]` | `[low/medium/high]` | `[issue/owner]` |

## Security, secrets, and MCP review

- MCP config reviewed: `[yes/no/path]`
- Network or package install added: `[no / yes with rationale]`
- Secrets handling changed: `[no / yes with details]`
- Telemetry/deployment impact: `[none / details]`

## Validation evidence

Paste the exact commands and outcomes. Keep output paths when available.

```bash
cargo fmt --check
cargo test -p jcode project_init --lib -- --nocapture
cargo test -p jcode skill::tests --lib
cargo test -p jcode clean_code --lib
cargo test --test e2e harness_init_json -- --nocapture
cargo test --test e2e clean_code_check_json -- --nocapture
cargo test --test e2e harness_cli -- --nocapture
cargo test --test e2e harness_smoke -- --nocapture
cargo test --test e2e harness_live_provider -- --nocapture  # skips unless JCODE_HARNESS_LIVE_PROVIDER_SMOKE=1
cargo check -p jcode
selfdev build target=auto
```

- `selfdev reload`: `[passed / skipped / failed with known issue]`
- Governance audit: `[PASS/FLAG/BLOCK + path]`
- Additional evidence: `[logs, traces, gate files]`

## Known gaps and opt-in integration tests

- `[Gap]`: `[risk and mitigation]`
- Live-provider smoke: `[default skipped offline / run with isolated credentials and quota]`
- Browser/GitHub/database/telemetry MCP tests: `[not run / run with scope]`

## Migration notes

`[Only include if CLI behavior, config, schemas, or provider/auth behavior changed in a non-additive way. Otherwise write: No migration required.]`

## Rollback plan

- Revert commit(s): `[hashes]`
- Restore build channel: `[stable/current/version path]`
- Disable/avoid feature: `[env/config/command]`
