# Claude plugin REA findings and jcode implementation contract

Investigation: 2026-10-26, installed Claude Code **2.1.310**, macOS arm64. This report comes from actual REA MCP calls against the installed artifact, not previous workers or online documentation. No plugin was installed, no configuration was intentionally changed, and no commit was made by this investigator.

## Executive findings

- The launcher resolves to a native Mach-O with readable, minified JavaScript embedded alongside Bun bytecode. Exact source blocks were recovered without executing their bootstrap code.
- `/plugin` has four baseline tabs: **Discover, Installed, Marketplaces, Errors**. A conditional **Stats** tab exists. Browse is a nested marketplace-browsing state, not a fifth baseline tab.
- **Version-specific correction:** explicit manifest `commands` and `agents` replace their default directory scans in this installed version. `skills` is generally additive. Hooks are additive, with canonical file deduplication. MCP and LSP use default-first, later-definition-wins merging.
- Installation, enablement, successful runtime loading, and reload requirements are distinct states. Do not collapse them into an `installed` boolean or report activation just because a copy succeeded.
- The backend/UI/runtime contract below is actionable. Interactive installation, mutation, hook execution, and destructive rewind were deliberately not exercised. Static findings are not claims of successful execution.

## Target and evidence ledger

Launcher: `/Users/siavash/.local/bin/claude`.
Resolved target: `/Users/siavash/.local/share/claude/versions/2.1.310`.
SHA-256: `ffb64331bc0ee1b69efb7c2c428bd3ab516bc31b75bf60886b356c97c1c25ffd`.
REA server: `rea-agents` 6.5.0.

| Ref | Actual REA Evidence ID | What it establishes |
| --- | --- | --- |
| E1 | `ev_6cb2ca2710309251e409e23c6e327cc452de3e0c25c77771da2b253738906e19` | Complete retained `inspect_macho` observation for this target. |
| E2 | `ev_f9b77aecfce1acef60f2ec7c3e984d4fef27d0be87449cbd6c9a4778eaa3ecb3` | Bounded segment view: `__BUN/__bun` file offset 93,241,344, section size 180,018,944 bytes; arm64 Mach-O. |
| E3 | `ev_5e0d2196c8b37a91fab5cffc382d3c88c77d534b44daa566cca8bdd1c11d3167` | REA process capture of exact-byte extraction for the initial later-source subset. Node reads the target, not Claude execution. |
| E4 | `ev_01dddda9cb5d5fd60df3fe79003b6d5c81575993d8c64119ac4329d73cad84f8` | Retained summary-first JavaScript analysis: four selected UI/command/helper sources parsed, zero parse failures; partial graph. |
| E5 | `ev_e99846209afa13abb853276e53b97623993355a7d54e68064dab8d39bd4b67d6` | Corrected extraction scanning from the Bun segment start, selecting 93 relevant exact source blocks, including earlier authoritative schemas. |
| E6 | `ev_ee1e8f16a79a5dd498f40c935ce63b94542cc71815048d65d67472ec9e6f30a5` | Retained summary-first inert analysis of authoritative schema and plugin diagnostics/data helpers: two sources, zero parse failures; partial graph. |
| E7 | `ev_875064a917dee929c489efe77d3e080fb38e10ac38836a15e322e258972061c5` | Bounded module-identity page from E6. |
| E8 | `ev_ab3dbca0b1b0770494dbc3bcdd2dbeb2807274af44c3608b026e95a2f3f5f70c` | Actual installed executable `plugin --help`, exit 0. Watched settings and plugin registry files unchanged before/after. |
| E9 | `ev_57e3e57093cae5a0ec852842b9213cb6bff80e2f35ea446573baa7ca467963c9` | Actual install/disable/update/marketplace/list `--help` captures, all exit 0. Watched settings and registry files unchanged. |

Local evidence bundle: `/Users/siavash/.jcode/scratch/claude-plugin-rea-evidence-20261026.json`, 64,904,954 bytes, 17 retained records and four unknown records. SHA-256: `51254be9c9465bd60a27f49154840e197f4daea46af5d8770460fc2e0691e4e3`.

Local retained source root: `/Users/siavash/.jcode/scratch/claude-plugin-rea-20261026-full/`.
Its `extraction.json` contains original target byte ranges and each extracted block's SHA-256. Extraction manifest SHA-256 from E5: `8e58cb0e1974ee619aa077e69c9f3d3718ece5a9d2e97de7cb9648abbb848c7b`.
The original subset remains at `/Users/siavash/.jcode/scratch/claude-plugin-rea-20261026/`; its common source files are identical. These local artifacts are not repository source or complete original source recovery.

### Source locator convention

The numeric filename is the **original binary byte offset** of that exact source block. Ranges below are **JavaScript string character offsets within the extracted file**, not machine-code addresses or original binary byte offsets. For non-ASCII text, do not add character offsets to binary offsets without UTF-8 conversion.

| Source | Focused range / functions | Evidence |
| --- | --- | --- |
| `embedded-216124080.js` | 60,500–81,900: names, metadata, component schemas; 110,400–113,000: install registry schemas | E5, E6 |
| `embedded-220708418.js` | 6,100–7,050: `L`, `Ff`, `Wze` persistent plugin data; diagnostics elsewhere | E5, E6 |
| `embedded-225156883.js` | 586,500–594,800: `igo`, `RXe`, `ago`, `lgo`, `dgo`, `$xr` LSP loader | E3, E5 |
| same | 3,633,900–3,637,000: command-hook execution and environment setup | E3, E5 |
| same | 3,974,500–3,984,000: `Cft`, `zat`, `Evo`, `PXr`, `kDt` MCP loader | E3, E5 |
| same | 4,706,500–4,717,000: default component scans and hook loading/merge; `h5n` | E3, E5 |
| `embedded-258590484.js` | approximately 13,500–19,600: plugin listing groups, activation guidance, CLI helpers | E3, E4, E5 |
| `embedded-260149013.js` | approximately 36,400–38,000: `qa`, `Lg` install auto-activation; 59,000: keys; 240,000–250,000: tabs | E3, E4, E5 |
| `embedded-251288836.js` | 410,800–416,500: `tUe`, `jo`, `bn` rewind dialog and action dispatch | E3, E5 |

## Observed manifest and component semantics

“Observed static” below means a declaration or algorithm was read in recovered shipped source. It does not mean a plugin using it was executed.

### Metadata and registry

**Observed static, E5/E6:** `.claude-plugin/plugin.json` metadata includes required `name`, optional display name, version, description, author information, homepage, repository, license, keywords, and default enablement. Display name is presentation, not namespacing. Plugin name validation rejects empty names, spaces, control characters, and bidirectional formatting characters. Do not describe the entire name contract as a strict kebab-case regex: the inspected metadata validator does not do that.

Component path validators generally require `./`-prefixed relative paths, with format-specific suffix checks. Skill paths also accept `.`. Resolution adds containment and, in directory-loaded cases, realpath/identity checks. Validation alone is not enough to prevent symlink escape.

**Observed static, E5/E6:** installed registry schema v1 maps IDs to one installation record; v2 maps IDs to arrays. V2 entries include `scope` (`managed`, `user`, `project`, `local`), optional `projectPath`, required `installPath`, optional version/timestamps/git SHA/resolved version, and dependency-auto-install metadata. The `projectPath` schema description says it is required for project/local, though the field itself is declared optional in this schema. Runtime enforcement must be checked separately.

**Observed static, E5/E6:** per-plugin persistent data lives below the plugin home `data/` directory under an encoded ID. `Wze` creates that directory when requested outside test mode. This is separate from versioned installed code. Therefore merely calling runtime expansion helpers in a “read-only probe” could create directories; this investigation never invoked those helpers.

### Component path merge matrix

| Component | Default | Explicit declaration in this installed version | Status |
| --- | --- | --- | --- |
| Commands | `commands/` | Replaces automatic default scan. String, path array, or named object map. Object entry requires source XOR inline content; optional description, argument hint, model, allowed tools. | Observed static, E5/E6; loader independently confirms `!manifest.commands && defaultExists`. |
| Agents | `agents/` | Replaces automatic default scan. Relative Markdown file or array of files. | Observed static, E5/E6; loader confirms `!manifest.agents && defaultExists`. |
| Skills | `skills/` | Generally additional directories, including plugin root via `.` / `./`. Marketplace-root-source special case can replace broad default skill scan. | Observed static, E5/E6; special-case full behavior not runtime tested. |
| Hooks | `hooks/hooks.json` | Additional relative JSON path, inline event map, or mixed array. Per-event arrays append. Default canonical path loads once. | Observed static, E5/E6. |
| MCP | `.mcp.json` | String path, MCPB reference, inline server map, or mixed array. Default first, then explicit in order; later duplicate server names win. | Observed static, E5/E6. MCPB execution/configuration not tested. |
| LSP | `.lsp.json` | String path, inline server map, or mixed array. Default first, then explicit in order; later duplicates win. | Observed static, E5/E6. |

The installed code emits a `folder-shadowed-by-manifest` diagnostic for existing default command/agent/style/theme directories that explicit declarations exclude. A blanket additive-path implementation does not match this version.

The skill schema describes directory paths. The loader also supports a root `SKILL.md` fallback when no skills directory exists. Acceptance of a direct `SKILL.md` or raw Markdown manifest skill path was not established. Agent file-stem versus frontmatter naming, and nested-command namespace construction, were not traced far enough to claim parity.

### MCP runtime contract

**Observed static, E5, main `Cft`/`zat`/`PXr`/`Evo`:**

1. Read default `.mcp.json`, then merge manifest sources in declared order. File input accepts a `mcpServers` wrapper or a bare named map. Individual server entries are validated.
2. Preserve enabled gating: disabled plugins do not contribute live server registrations.
3. Namespaced registrations use `plugin:<plugin-namespace>:<server>`, with dynamic scope and plugin source/path metadata. The namespace helper takes both plugin name and source; do not silently assume all source classes reduce to the bare display name.
4. Expand plugin references and user options in command, args, environment; remote transports also expand URL/headers. Missing configuration and invalid URLs produce diagnostics rather than a successful connected state.
5. Host-only connector/IDE transport types are rejected as plugin declarations. Static source includes ordinary stdio, SSE, HTTP and WebSocket handling.
6. Actual source seeds `CLAUDE_PLUGIN_ROOT` and `CLAUDE_PLUGIN_DATA` **before** spreading the plugin server's declared `env`. Thus making those variables authoritative in jcode is a deliberate hardening choice, not exact literal parity with this artifact.

MCP bundle download, OAuth, executable command sources, headers-helper execution, dependency installation and remote registry behavior were not exercised. Unsupported implementations must say so before making filesystem or execution changes.

### LSP runtime contract

**Observed static, E5/E6, main `RXe`/`ago`/`lgo`/`dgo`:**

- Default `.lsp.json` is a bare named server map. Explicit sources are confined to the plugin directory and merged after it.
- Required nonempty command and nonempty `extensionToLanguage`; extension keys start with a dot. `args`, `env`, initialization options, settings, workspace folder, startup/shutdown/request timeout, restart policy and diagnostics policy are represented in the shipped schema. Transport is `stdio` or `socket`, default `stdio`. Schema text gives a 60,000 ms default request timeout; that is not the hook timeout.
- Expansion covers command/args/env/workspace folder and recursively traverses initialization options/settings. Missing variables in initialization/settings data are retained literally with a diagnostic; spawn-critical substitutions have a different error path.
- Names are `plugin:<pluginName>:<server>`, dynamic scope, with source provenance. Extension conflict diagnostics retain which server is active.
- The inspected expansion seeds root/data/project variables then spreads declared env, just as the MCP observation above.
- `workspaceFolder` is carried through configuration; exact cwd versus LSP rootUri use was not traced in this pass. Do not claim either from the field name alone.

### Hooks

**Observed static, E5/E6:** standard hooks file first; manifest hook files or inline objects afterward. `h5n` concatenates each event's matcher arrays. Canonical path sets prevent loading the default twice. Invalid traversal, unreadable hook files, duplicate explicit paths and unloadable guards have separate diagnostics. Disabled plugin hooks may be parsed for inventory but are not registered.

Command hooks support a shell-form command and an explicit `args` form. The executor recognizes plugin root/data/project variables, verifies plugin directory existence, and supplies plugin environment. Explicit timeout is seconds, converted to milliseconds. The fallback is an imported `yp` constant whose value has **not** been resolved here. Do not infer a default hook timeout from LSP's request timeout or from old documentation.

**Unknown in this pass:** exact Stop-hook blocking/continuation semantics, default timeout, full matcher grammar, all hook stdin payload fields, and event-specific exit-2 handling. Source contains richer permission aggregation and async/other hook paths, but they were not followed far enough to assert complete behavior. In jcode, reject unsupported Stop/prompt/agent/http/async forms explicitly rather than aliasing Stop to a generic turn-end notification.

## CLI and `/plugin` UI contract

### Runtime-observed CLI

**E8:** top-level aliases `plugin|plugins`; commands include configure, details, disable, enable, eval, init|new, install|i, list, marketplace, prune|autoremove, tag, test, uninstall|remove, update, validate.

**E9:**

- Install takes `plugin@marketplace` or a bare name and defaults `--scope` to `user`; permitted install scopes are user/project/local.
- Disable supports `--all`; scope defaults to auto-detect, not unconditionally user.
- Update advertises restart required and auto-detected scope, including managed.
- Marketplace has add/list/remove|rm/update; update without a name updates all.
- List exposes JSON; `--available` and `--data-size` require JSON.
- Install supports declared user configuration and explicit command-consent flags. These are feature contracts, not permission to execute catalog-provided commands during this investigation.

Both help captures show the same hashes before and after for the three watched user files. They do not establish that every possible file on the machine was unchanged.

### Observed-static interactive UI

**E4/E5, manager module:**

- Baseline tabs: Discover, Installed, Marketplaces, Errors; Errors includes a live count. Stats is conditional on a feature check. Browse is a nested marketplace state.
- Search is typed text; Space toggles multiselection; `i` installs selected items; Enter views details; Escape goes back. Operations use loading and error states and do not merely print an installed list.
- Installed details distinguish configurable bundled MCP servers, disabled plugins, dependency outcomes, and runtime-load errors.
- Post-install helper returns activated, needs-config, load-failed or reload-required. It checks whether refreshing would invalidate cached tool configuration, can resolve dependencies, reloads, and checks diagnostics. Failures mark refresh needed. A successful disk install therefore need not mean live activation.

**Unknown:** no live `/plugin` screen was opened or clicked. Exact default tab under every entry route, navigation focus, configuration dialog rendering, network failure UX and keyboard interaction were not runtime-validated.

## Secondary: rewind semantics

**Observed static, E5, rewind module `tUe`/`jo`/`bn`:** selection restores to the point **before** a chosen message. When file history is available, choices include code and conversation, conversation only, code only. Otherwise conversation is offered. Optional summarize-from/summarize-up-to actions and Never mind are distinct operations.

For restore-both, code restoration is attempted first, conversation restoration second, in independent try/catch blocks. Partial failure is representable. Symlink-skipped files are reported. UI copy says manual edits and edits made through bash are unaffected by checkpoint rewind.

**Unknown:** persistence and trimming semantics of prompt recall/search history across rewind. The inspected dialog does not establish that recalled prompts are deleted or retained. Keeping jcode's session-local input recall independent from the active conversation timeline is a design requirement, not proven Claude parity from this evidence. No destructive rewind was performed.

## Actionable jcode parity checklist

This is an implementation recommendation inferred from the observations, not a claim that the concurrently edited jcode code already passes it.

- [ ] Shared backend model keeps catalog entries, scoped install records, enablement, loaded contributions and diagnostics separate.
- [ ] CLI and TUI use the same backend operations; preserve aliases, scope defaults and actionable failures.
- [ ] Component loader implements the merge matrix above, including commands/agents replacement and canonical hook dedup.
- [ ] Hook, skill/command, profile, MCP and LSP contributions retain plugin identity and scope. Reload removes only the disabled/uninstalled plugin's contributions.
- [ ] Runtime refresh reports activated versus requires-config/reload/load-failed honestly; unsupported components remain visible as diagnostics.
- [ ] Validate all component paths and realpath boundaries before reading, copying or spawning. Treat manifest strings as data.
- [ ] Unsupported remote source types, command installers, MCP bundles and unsupported hook forms fail explicitly without a partial install advertised as successful.
- [ ] Installation/update failure leaves the previous usable installation intact. Updating code does not erase persistent plugin data.
- [ ] UI covers loading, empty list, selection, details, confirmation, busy operation and failure states across the four baseline tabs.
- [ ] Synthetic fixtures verify default-first server collisions, mixed path/object arrays, disabled plugin gating, canonical hook dedup, command/agent shadowing, scope isolation, malformed manifests and symlink escapes.
- [ ] End-to-end acceptance launches the rebuilt jcode daemon on a private socket; a build alone is not runtime evidence.

Normal repository inspection located current integration boundaries: `crates/jcode-base/src/plugin.rs`, `plugin_components.rs`, `agent_profile.rs`, `hooks/plugin_hooks.rs`, `mcp/plugin_source.rs`, `mcp/protocol.rs`; `crates/jcode-app-core/src/tool/mod.rs`; TUI command/UI modules; and `src/cli/args/plugin.rs` / `src/cli/plugin.rs`. These paths were inspected with repository tools, not misrepresented as REA target evidence. Other workers own their changes; this investigation does not validate or commit those implementations.

## Coverage, limitations and handoff

- Actual MCP operations: target/session inspection, open, Mach-O summary and bounded pages, process captures, summary-first inert JS analysis, retained module paging. No Ghidra/Hopper/IDA decompilation is claimed: those deep providers were unavailable, and the readable embedded source route answered the core questions instead.
- Source extraction is an analyst-controlled byte-copy process with original target hash/ranges, not a REA built-in Bun decoder. E3/E5 capture that process; E4/E6 analyze selected recovered text. The original executable's bootstrap was not executed for extraction.
- E4 analyzed four selected files; E6 analyzed two. Other files remain outside their semantic graph coverage, and node ceilings make those graphs partial. Unresolved imports and conservative relationships cannot prove absence.
- Integrity checks compared all 93 recovered files byte-for-byte with their original binary ranges and verified each SHA-256. All nine cited evidence IDs exist in the exported bundle. These checks passed; they do not validate plugin runtime behavior.
- Native target ownership stayed with this investigator. The owned REA native session closed successfully after export. No existing user Claude process was attached to, killed or driven. Only owned, short-lived help commands were launched. No install/update/enable/disable/uninstall/configure operation was executed.
- Required questions: target identity answered; manifest/merge/UI declaration/lifecycle model answered statically; CLI grammar observed at runtime; mutation and interactive activation intentionally unverified; rewind partially answered; prompt-history retention unresolved.
