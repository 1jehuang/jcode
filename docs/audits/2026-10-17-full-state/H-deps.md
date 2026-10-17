# Dependency Freshness Audit (KCode fork, 2026-10-17)

Repo: `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode`
Workspace version: `jcode 0.85.1-k1.1.1` (KCode fork, base v0.85.1)
Source: `cargo metadata` + `cargo outdated --workspace --root-deps-only` (raw at `~/.jcode/scratch/audit/H-cargo-outdated.txt`)

## Tools available
- cargo-outdated: **YES** (`cargo-outdated-outdated 0.19.0`)
- cargo-audit: **NO** (not installed; security audit is out of scope for this pass)

## Inventory
- Workspace members (local crates): **93**
- Total packages in lockfile: **883**
- Direct deps (unique, external): **95**
- Transitive (approx, external, non-workspace): **~790**
- Distinct license strings (external): **15**

## Outdated direct deps — overview
- Distinct direct deps with at least one outdated entry: **68** of 95 (~72%)
- Outdated entries classified `Normal`: **66**
- Outdated entries classified `Development` (pre-release): **7**
- Outdated entries requiring major-version bump: **6** (HIGH risk)

## Top-20 outdated direct deps
| Crate | Current | Latest | Semver jump | Risk |
|---|---|---|---|---|
| agent-client-protocol | 0.10.4 | 3.4.0 | 0 → 3 (MAJOR +3) | **HIGH** |
| dirs | 5.0.1 | 7.0.0 | 5 → 7 (MAJOR +2) | **HIGH** |
| ratatui-image | 10.0.6 | 11.1.0 | 10 → 11 (MAJOR +1) | **HIGH** |
| thiserror | 1.0.69 | 2.0.21 | 1 → 2 (MAJOR +1) | **HIGH** |
| similar | 2.7.0 | 3.2.0 | 2 → 3 (MAJOR +1) | **HIGH** |
| toml | 0.8.2 | 1.1.8+spec-1.1.0 | 0 → 1 (MAJOR +1) | **HIGH** |
| aws-sdk-bedrock | 1.148.0 | 1.161.0 | +13 minor | MEDIUM |
| aws-sdk-bedrockruntime | 1.136.0 | 1.148.0 | +12 minor | MEDIUM |
| aws-sdk-sts | 1.108.0 | 1.119.0 | +11 minor | MEDIUM |
| jsonschema | 0.49.3 | 0.59.0 | +10 minor | MEDIUM |
| rusqlite | 0.32.1 | 0.40.2 | +8 minor | MEDIUM |
| uuid | 1.21.0 | 1.28.0 | +7 minor | MEDIUM |
| tokio-tungstenite | 0.24.0 | 0.30.0 | +6 minor | MEDIUM |
| tokio | 1.49.0 | 1.53.3 | +4 minor | MEDIUM |
| criterion | 0.5.1 | 0.8.2 | +3 minor | MEDIUM |
| tempfile | 3.24.0 | 3.27.0 | +3 minor | MEDIUM (some callers see `Development`) |
| aws-config | 1.9.0 | 1.12.0 | +3 minor | MEDIUM |
| mail-parser | 0.9.4 | 0.11.9 | +2 minor | MEDIUM |
| windows-sys | 0.59.0 | 0.61.2 | +2 minor | MEDIUM (windows-only) |
| resvg / usvg | 0.46.0 | 0.48.1 | +2 minor | MEDIUM |

Other notable MEDIUM entries that did not fit top-20 (in no particular order):
`global-hotkey 0.7 → 0.9`, `aws-smithy-types 1.6 → 1.8`, `aws-types 1.4 → 1.6`,
`tokenizers 0.21 → 0.23`, `reqwest 0.12.28 → 0.13.5` (minor bump but the
`rustls-tls` / `rustls-tls-native-roots` features were **removed** in 0.13.5 —
see deprecation warnings emitted by cargo during the run), `chrono 0.4.42 → 0.4.45`,
`serde 1.0.228 → 1.0.229`, `serde_json 1.0.149 → 1.0.154`, `rand 0.9.3 → 0.10.3`,
`regex 1.12.2 → 1.13.1`, `libc 0.2.180 → 0.2.191`, `base64 0.22.1 → 0.23.1`,
`bytes 1.11.1 → 1.12.1`, `open 5.3.3 → 5.4.4`, `image 0.25.9 → 0.25.10`,
`tokio-stream 0.1.18 → 0.1.19`, `flate2 1.1.8 → 1.1.10`, `pulldown-cmark 0.12.2 → 0.13.4`,
`memchr 2.7.6 → 2.8.3`, `unicode-segmentation 1.12 → 1.13.3`, `arboard 3.6 → 3.7`,
`core-graphics 0.23 → 0.25` (macOS-only), `tikv-jemalloc-ctl 0.6 → 0.7`,
`tikv-jemalloc-sys 0.6 → 0.7`, `tokio-util 0.7.18 → 0.7.20`, `glob 0.3.3 → 0.3.4`,
`tar 0.4.45 → 0.4.46`, `filetime 0.2.27 → 0.2.29`, `lettre 0.11.22 → 0.11.23`,
`pdf-extract 0.12.0 → 0.12.1`, `rayon 1.11 → 1.12`, `h2 0.4.19 → 0.4.20`,
`http 1.4.0 → 1.5.0`, `tokio-rustls 0.26.4 → 0.26.6`, `webpki-roots 1.0.6 → 1.0.9`,
`azure_core 1.0.0 → 1.1.0`, `rustls 0.23.45 → 0.23.46` (Development).

## Deprecations in source

`#[deprecated]` attributes and `deprecated` keyword hits in `crates/`:

| File:Line | Item | Reason |
|---|---|---|
| `crates/jcode-tui/src/tui/info_widget.rs:1296` | `#[deprecated(note = "Use calculate_placements instead")]` | internal API; replace callsites before next minor |
| `crates/jcode-tui/src/tui/info_widget.rs:2008` | `#[deprecated(note = "Use render_all instead")]` | internal API; replace callsites |
| `crates/jcode-base/src/provider/startup.rs:154` | log message: "JCODE_USE_CLAUDE_CLI is deprecated and will be removed" | env-var path scheduled for removal; native Anthropic transport is default |
| `crates/jcode-base/src/provider/startup.rs:160` | log message: "Using deprecated Claude CLI provider" | forced by `JCODE_USE_CLAUDE_CLI=1` |
| `crates/jcode-base/src/provider/claude.rs:7` | doc comment: "Claude CLI path is deprecated and only kept…" | runtime path being phased out |
| `crates/jcode-base/src/provider/cursor.rs:9` | doc comment: composer-1.5 deprecated upstream | doc-only; safe |
| `crates/jcode-base/src/provider/external.rs:33` | doc comment: "Registry key for the deprecated Claude CLI provider runtime" | registry key slated for removal |
| `crates/jcode-base/src/gateway.rs:194` | log message: "Gateway: {} connected with deprecated query token auth" | gateway auth path deprecated |
| `crates/jcode-provider-anthropic/src/lib.rs:528` | comment: "the (deprecated) Claude CLI transport" | doc reference |
| `crates/jcode-provider-claude-cli-runtime/src/lib.rs:683` | log: "Claude transport: deprecated CLI subprocess; prefer `--provider claude`" | runtime hint |
| `crates/jcode-provider-claude-cli-runtime/src/lib.rs:691` | connection metadata: "deprecated cli subprocess" | runtime metadata |
| `crates/jcode-tui-mermaid/src/mermaid_runtime.rs:284,288,1778` | `#[allow(deprecated)]` callsites for `from_fontsize` | upstream `mermaid-rs-renderer` API changed |
| `crates/jcode-app-core/src/tool/tests.rs:273` | test string: "the deprecated direct subagent tool must not be exposed; use swarm instead" | test sentinel — already enforced as removed |

No deprecated trait items, fields, or module-level items were found in the
crate source besides the two `#[deprecated]` annotations in `info_widget.rs`.

## License flags
External packages: **790** total.
Top license distribution (counts):
- `MIT OR Apache-2.0`: 354
- `MIT`: 168
- `Apache-2.0 OR MIT`: 56
- `MIT/Apache-2.2` (`MIT/Apache-2.0`): 49
- `Apache-2.0`: 41
- `Unicode-3.0`, `Zlib OR Apache-2.0 OR MIT`, `Apache-2.0 WITH LLVM-exception`: minor
- 1× **`LGPL-2.1-or-later`** (intermixed with `MIT OR Apache-2.0`):
  - `r-efi 5.3.0` — pulled in transitively by the boot/runtime chain. Single
    copyleft-LGPL hit; rest of the graph is MIT/Apache/BSD/Zlib/Unlicense/BSL.
- **AGPL**: 0 hits.
- **GPL (any version)**: 0 hits other than the LGPL row above.

Workspace members (93 local `jcode-*` crates) carry no license field. All
external deps are permissively licensed except `r-efi`.

## Forward recommendation
The repo is healthy but drifts ~2 minor versions behind on most fast-moving
crates (chrono, serde, tokio, uuid, jsonschema). Only **6** of 95 direct deps
sit a major version behind — those are the real risk, and they cluster in 3
hotspots: the **terminal/TUI stack** (`ratatui-image 10 → 11`, `dirs 5 → 7`,
`thiserror 1 → 2`), the **config layer** (`toml 0.8 → 1.x`, which moves to a
new crate family around `spec-1.1`), and **`agent-client-protocol 0.10 → 3.4`**
(a ~3-major jump that has been the subject of upstream renames — see also the
`unstable_session_*` feature warnings emitted during the run). Plan the next
upgrade pass around those six, and `reqwest 0.12 → 0.13` (feature flag
removals) before touching the patch-level bumps.