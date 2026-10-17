# Cherry-Pick Analysis (KCode fork, 2026-10-17)

## Upstream state
- **Upstream HEAD:** `6f2926e0c` (docs: update weekly stars chart)
- **Fork HEAD:** `748eeaa1e` (docs(handoff): name the real root cause of the Actions block)
- **Fork version:** `0.85.1-k1.1.1` (Cargo.toml)
- **Last upstream sync point:** v0.85.0 (merge-base `752df77d3`)
- **Commits behind upstream (since v0.85.0):** 2,840
- **Commits ahead of upstream (since merge-base):** 334
- **Upstream release tags available:** v0.85.0 through v0.98.1 + master

Counts reverified using immutable tips (objects fetched only from the fork origin):

```sh
git rev-list --left-right --count 6f2926e0c63d885f5858daed5246566a7838e948...748eeaa1e1f45a87a5d6d230963c6ad98495525b
# 2840 334
git rev-list --count 752df77d3c13fa7a648eda257dbfcb8d3ea5974d..748eeaa1e1f45a87a5d6d230963c6ad98495525b
# 334
```

The earlier 7,762 ahead figure was incorrect. D-fork-delta.md uses an older
upstream tip (`4a4c678f1`); its corrected comparison is 2,828 behind / 334 ahead.
The two upstream snapshots differ by 12 upstream-only commits.

## Scope of analysis
- Analyzed upstream commits from v0.85.0 through upstream/master (6f2926e0c)
- Focused on v0.90.0..upstream/master (most recent, highest relevance)
- Cross-referenced against 300+ fork-modified source files
- Tested cherry-pick applicability in temp branches off 748eeaa1e
- Ran `cargo check --workspace` on code-touching candidates

## Cherry-pick candidates (upstream-clean)

| SHA | Title | Category | Files | Cargo check | Notes |
|-----|-------|----------|-------|-------------|-------|
| `d0091d078` | fix(fuzzy): remove redundant slice conversion in position tracker | Bug fix | `crates/jcode-fuzzy/src/lib.rs` (1 file) | **PASSED** | Isolated fix in crate not modified by fork |
| `5cbb82c81` | test: guard CLI parse exit statuses (refs #1823) | Test | `tests/cli_exit_status.rs` (1 new file) | N/A (new test file) | Purely additive test, no source conflicts |
| `0d240f1d4` | ios: support all orientations so App Store accepts iPad multitasking build | Platform fix | `ios/Sources/JCodeMobile/Info.plist` (1 file) | N/A (plist only) | iOS-specific, no Rust code |
| `4121a46e6` | Add optional pinned Nix development shell | DevEx | `flake.lock`, `flake.nix`, `CONTRIBUTING.md` (3 files) | N/A (new files + doc) | Creates new Nix flake files; modifies CONTRIBUTING.md |
| `458384094` | Add .gitattributes so line endings do not depend on each contributor's global config | DevEx | `.gitattributes` (1 new file) | N/A (config only) | Purely additive, standard git config |
| `7e696d778` | docs: plan guided bring-your-own Google OAuth setup | Docs | `docs/GOOGLE_GUIDED_SETUP.md` (1 new file) | N/A (doc only) | New doc file, no code impact |

## Conflicts (cannot cherry-pick cleanly)

| SHA | Title | Reason |
|-----|-------|--------|
| `0a4e871a0` | fix(security): update rustls and quinn-proto to patched releases | CONFLICT in `Cargo.lock` (fork has diverged dependency tree) |
| `fa9216cd0` | fix: document hook arrays and verified turn_start behavior (fixes #1799) | CONFLICT in `docs/HOOKS.md` (fork has different hooks doc) |
| `08ea2864f` | fix(changelog): compare successor against last published release | CONFLICT in `changelog/index.json` (fork changelog diverged) |
| `b9f86201e` | docs: remove planned features and reference sections | CONFLICT in `README.md` (fork has different README) |
| `fc462619c` | ci(release): smoke test public installer before announcement | CONFLICT in `.github/workflows/release.yml` + deleted test file |
| `cb2895ab0` | fix(release): disclose unsigned Windows publication mode | CONFLICT in `RELEASING.md` + deleted test file |
| `027434399` | ci: allow fork PR builds without an unused SSH deploy key | CONFLICT in `.github/workflows/ci.yml` (CI config diverged) |
| `6de800d2f` | fix(tui): collapse keyboard error reporting guards | Touches fork-modified `crates/jcode-tui/src/tui/app/input.rs` |
| `6a2ffdcf7` | fix(cli): collapse dashboard open error conditional | Touches fork-modified `src/cli/commands.rs` |
| `1c7426eb1` | fix(desktop-open): cfg-gate Niri deserialization import | Touches fork-modified `crates/jcode-desktop-open/src/lib.rs` |
| `06630fc9e` | refactor: restore owned Rust budgets and handle runtime errors | 56 files, massive diff touching many fork-modified crates |
| `783578b93` | fix(tui): unblock release budgets and require CI-gated publication | Touches many fork-modified TUI files |
| `0f966d94c` | fix(usage): compact recorded token and cost summaries | Touches fork-modified usage code |
| `b8f30bbdf` | fix(server): reduce model catalog dedup CPU overhead | Touches fork-modified server code |
| `2b49257ed` | fix(mcp): bind validated remote URLs without panic assumptions | Touches fork-modified MCP code |
| `28ef555d5` | fix(provider): bind validated account route without unwrap | Touches fork-modified provider code |
| `bb9a619aa` | fix(browser): inspect handoff failures without remapping errors | Touches fork-modified browser code |
| `4c72e77f4` | fix(browser): recover runtime handoff provider failures with direct actions | Touches fork-modified browser code |

## Notable conflict requiring manual resolution

**`0a4e871a0` fix(security): update rustls and quinn-proto to patched releases** is the highest-priority upstream commit that cannot be cleanly cherry-picked. The Cargo.lock conflict means the fork's dependency tree has diverged. **Manual resolution approach:** update rustls and quinn-proto versions in the fork's Cargo.lock directly, matching the upstream patched versions. This is a security-critical dependency update that should be addressed regardless of cherry-pick feasibility.

## Forward recommendation
- These upstream-clean commits CANNOT be filed as PRs against upstream from the KooshaPari account per binding memo 2026-10-15. Listed for operator planning only.
- The 6 clean candidates are low-risk, additive changes (1 bug fix, 1 test, 1 iOS fix, 3 devex/docs). They can be applied as a batch.
- The security fix (`0a4e871a0`) requires manual Cargo.lock resolution and should be prioritized separately.
- The fork's extensive modifications (300+ files across 60+ crates) make most upstream commits conflict-prone. A full rebase or merge is not feasible without significant effort.

## Methodology
- Added upstream remote (`https://github.com/1jehuang/jcode.git`), fetched tags and refs
- Identified fork-modified files via `git diff v0.85.0 HEAD -- crates/ src/ Cargo.* build.rs scripts/`
- Scanned upstream commits v0.90.0..upstream/master for isolated (1-3 file) non-merge commits
- Cross-referenced each candidate's touched files against fork-modified file list
- Tested cherry-pick applicability in temp branches (`git checkout -b temp-cp-<sha> 748eeaa1e`)
- Ran `cargo check --workspace` on code-touching clean candidates
- Deleted all temp branches after testing