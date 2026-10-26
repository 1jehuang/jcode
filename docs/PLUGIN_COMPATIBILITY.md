# Plugin compatibility

Status as of October 26, 2026. Jcode supports a subset of the Claude plugin format. Installing a plugin does not prove that every component loaded or that its external programs are available. This document describes the current source, not a full Claude parity or runtime acceptance claim.

## Use plugins

Open `/plugin` in the TUI. Its four tabs are Discover, Installed, Marketplaces and Errors. Left/Right switches tabs. Discovery uses registered marketplaces; an empty catalog offers explicit marketplace-add actions rather than silently adding one. Plugin details offer install scopes. Installed details offer enable/disable, update and uninstall for the selected record. Uninstall and marketplace removal have confirmation screens.

The same lifecycle is available without provider credentials or a running daemon:

```bash
jcode plugin marketplace add /absolute/path/to/marketplace
jcode plugin marketplace list --json
jcode plugin install example@market --scope project
jcode plugin list --json
jcode plugin disable example@market --scope project
jcode plugin enable example@market --scope project
jcode plugin marketplace update market
jcode plugin update example@market --scope project
jcode plugin uninstall example@market --scope project
jcode plugin marketplace remove market
jcode plugin validate /absolute/path/to/plugin
```

A marketplace contains `.claude-plugin/marketplace.json`. A plugin contains `.claude-plugin/plugin.json`. Marketplace sources can be local directories or supported Git sources. Remote acquisition needs Git and network access. URL/GitHub source objects accept `ref` and a verified 40-character lowercase commit `sha`; SHA takes precedence. `git-subdir` selects a contained repository path without symlink components. Unsupported source options are rejected rather than silently ignored.

TUI argument forms include `/plugin install example@market --scope project` and `/plugin marketplace add owner/repo`. These slash forms split on whitespace; use the CLI with shell quoting for paths containing spaces. The CLI accepts `--json` and `--scope` before or after subcommands. Mutation scope defaults to `user`; it is not inferred from an existing installation.

## Scope, updates and activation

- `user` applies across projects. `project` and `local` apply to the exact canonical working directory recorded at installation, not its whole directory tree.
- Records for all three scopes live in Jcode's user-owned plugin registry. A `project` install does not write a shareable repository settings file. There is no managed scope.
- For the same plugin identity, `local` overrides `project`, which overrides `user`. A disabled narrower record masks an enabled wider record. Listing installed records is different from listing effective contributions.
- Conflicting visible plugin names from different marketplaces are rejected. Prefer `name@marketplace` identifiers. Ambiguous removal fails instead of removing multiple records.
- Updates publish a new immutable cache directory. A failed acquisition or validation leaves the previous installed record intact. Uninstall removes the selected registry record, not cached directories. There is no cache-prune command here.
- Contributions refresh before the next prompt. Plugin MCP reconciliation retires changed/removed plugin registrations without rebuilding unrelated dynamic MCP connections. This is not immediate cancellation of an in-flight tool call.

Plugin hooks and servers execute external programs with the user's privileges. Install only trusted sources. Component files are confined to the installed plugin root, including canonical-path checks for symlink escapes. That containment is not an execution sandbox.

## Implemented components

| Component | Current behavior |
| --- | --- |
| Skills and Markdown commands | Qualified slash names such as `/example:review arguments`. Expands `$ARGUMENTS`, `${CLAUDE_SKILL_DIR}` and `${CLAUDE_PLUGIN_ROOT}` into instruction text. A skill wins a same-name command collision. Explicit Jcode skills retain precedence over plugin contributions. |
| Component discovery | Defaults include `commands/`, `agents/` and `skills/`. Explicit manifest `commands` or `agents` replaces its default scan. Explicit skill directories extend the default; a root `SKILL.md` fallback is supported when `skills/` is absent. |
| Agents | Plugin Markdown agents appear as profiles such as `example:reviewer`, selectable with `/agent example:reviewer`. They supply instructions, not a separate agent execution environment. |
| Hooks | Synchronous `command` hooks for `SessionStart`, `SessionEnd`, `PreToolUse` and successful `PostToolUse`. Supports tool-name regex matchers, JSON stdin, shell commands or explicit argument arrays. Pre-tool deny/block and exit-code blocking work; object `updatedInput` rewrites compose before native gates and execution. `ask` blocks with an explanation because this path has no interactive approval dialog. |
| MCP | Reads default `.mcp.json`, then manifest file/inline sources in order; later duplicate server definitions win. Uses namespaced registrations, Jcode's MCP connection/approval flow and authoritative plugin-root environment. Plugin clients are session-owned rather than shared through the name-only pool. |
| LSP | The `lsp` tool starts an enabled plugin's stdio server on demand for hover, definition, references, document symbols or diagnostics. It initializes, opens the current file, queries and shuts down a fresh process per call. Requires the declared executable and its dependencies to be installed already. File targets must be within the current workspace. Positions are one-based lines and UTF-16 characters. Requests have a fixed 20-second timeout; push diagnostics wait up to five seconds. |

Errors reports registry/discovery failures. Hook, MCP and LSP loaders also emit diagnostics to logs or operation errors. An empty Errors tab is not proof that every component activated.

## Agent profile YAML notices

A successful selection says that the profile was applied and that Markdown instructions take effect next turn. Model, tools and permissions remain unchanged. Frontmatter `name` and `description` are informational. Other fields produce an ignored-field notice only when those fields are actually present; the Markdown body still applies.

Profiles are saved instruction snapshots. `/agent reload` explicitly rereads the selected file; a malformed reload leaves the previous snapshot active. Source-selection changes reuse the saved snapshot rather than rereading a changed file. Disabling or uninstalling a plugin prevents new discovery/selection, but does not promise to revoke an already selected instruction snapshot. Use `/agent off` to clear it.

## Session-local prompt recall

Up/Down and Ctrl+R use the same session-local prompt journal, independent of the visible transcript. Rewind/checkpoint restoration does not erase submitted prompts from recall. Switching sessions, including `/clear` creating a new session, binds recall to the new session and hides the old journal while the switch is pending. Returning to a session restores its own recall.

Local journals live under `$JCODE_HOME/prompt-history/<hex-encoded-session-id>.jsonl`, using the default Jcode home when the variable is unset. They contain plaintext JSON strings. Repeated prompts move to the newest slot. Recall retains up to 1,000 unique entries. Current source has no additional history-only prompt-size rejection; it records accepted conversational input. Slash commands, input-line shell commands and intercepted login/account/SSH setup inputs are excluded. Ctrl+R keeps up to 50 search matches. Ordinary prompts may contain sensitive text, so protect the Jcode home accordingly.

SSH attachments keep recall in memory for that attachment. The legacy global prompt-history file is not loaded. Existing visible user messages can seed a session journal, but prompts removed before this journal existed cannot be recovered. This is recall storage, not an exact submission audit log or a coordinated multi-writer backup.

## Remaining Claude compatibility gaps

- Dependencies accept basic string identities and `{name, marketplace?}` objects. Install/update stages catalog and manifest dependency closures before publishing a single registry update. Missing/cyclic graphs fail atomically. Only the root marketplace's cross-market allowlist grants trust; an already enabled same-scope dependency may be reused. Explicit disabled choices are not overridden. Unsatisfied enabled roots stop contributing and appear in Errors. Version ranges remain explicitly unsupported. Package managers and server executables are not installed for you.
- Named-object command declarations support contained `source` files or inline `content`, exclusively. Description and advisory allowed-tools metadata are retained; argument hints/model declarations do not establish execution policy. Markdown command shell interpolation is not performed; supported placeholder replacement only produces instruction text. Model/tool/permission frontmatter does not reproduce Claude's command or agent execution policy.
- Stop and other unsupported hook events, prompt/agent/HTTP hooks and asynchronous hooks are skipped with diagnostics. Hook defaults use Jcode's hook timeout policy, not a claimed Claude timeout match.
- Provider-specific `userConfig`, plugin consent/configuration prompts, plugin OAuth provisioning, MCPB bundles and a persistent `CLAUDE_PLUGIN_DATA` contract are not implemented as Claude-compatible workflows. Existing Jcode MCP capabilities do not establish plugin-specific parity for them.
- LSP socket/non-stdio transport is rejected. Persistent workspace servers, automatic installation, restart policies and the full configurable timeout/workspace lifecycle are not implemented.
- Managed installs, complete Claude registry migration, dependency version-range enforcement, broad CLI aliases/actions, `--all` operations and automatic mutation-scope selection are outside this implementation. Existing read-only discovery of Claude-store skills is not full registry lifecycle interoperability.
- Marketplace-root special skill scanning and the full Claude activation/error-state UI are not verified or reproduced. Output styles/themes and other undeclared component kinds are not covered by the implemented-components table.

The comparison target was the installed Claude Code 2.1.310 artifact. See [the REA evidence ledger](claude-plugin-rea.md) for source locators and limits. Its static extraction and help captures did not exercise live plugin installation or UI behavior.

## Expanded acceptance: command maps, pinned sources and dependencies

Coordinator build `295129p18r` produced `869d64058-dirty-063e4fe32ea3`, fingerprint `063e4fe32ea3b81d`, SHA-256 `e7b1dfac006ae4bbb7c247f4ef6723d394e5153b7885a9ddf71657e11c784ad4`. On October 26, tasks `542122vpb5` and `534160e1bh` passed all 14 workflow checks and 10 additional public CLI checks, respectively, with no skips. The workflow rerun now verifies both mapped source and inline command bodies in actual provider requests, plus the unchanged strict rewind/session-isolation assertions.

Evidence: `/Users/siavash/.jcode/scratch/jpi-0yicmcbr/plugin-history-report.json` and `/Users/siavash/.jcode/scratch/jpi-2jxue2z3/plugin-parity-report.json`. Both private environments reaped their daemon and stopped the fake provider without cleanup errors. Git acceptance used private URL rewriting and file-only transport, with no network access or user configuration changes.

The additional checks cover tag pins, exact SHA precedence, branch updates, contained Git subdirectories, failed-source rollback, string/object catalog-plus-manifest transitive closure, project isolation, narrower disabled masks, preservation of explicit disablement, missing/cyclic update rollback and root-only cross-market trust. Integrated task `285516n9fu` passed 78 base plugin tests, 15 TUI plugin tests and 3 app-core plugin MCP tests, including the Errors-tab dependency snapshot test.

Dependency handling deliberately fails and rolls back untrusted/missing manifest-only edges, where recovered Claude code sometimes warns and skips. Disabled-choice precedence is conservative, not claimed identical to an untraced Claude mutation path. Version ranges remain unsupported. Git acquisition uses full clones rather than claiming Claude's sparse/partial optimization.

Run the additional private acceptance with:

```bash
python3 scripts/tests/plugin_parity_acceptance.py \
  --binary /Users/siavash/Projects/jcode/target/selfdev/jcode --execute
```

## Runtime acceptance status

**Passed, 14 checks, no skips.** On October 26, 2026, task `1546154z92` ran the full isolated public-binary test in 17.07 seconds with exit code 0. The tested binary was `/Users/siavash/Projects/jcode/target/selfdev/jcode`, built by coordinator task `088891gz7p` from fingerprint `fa5838ace24f7593`. Its SHA-256 was `4732f496f71ffaef1953d1fa9d8689d6783661306f0486ebc34b4696d7088da9`.

The actual run verified:

1. Public CLI validation, marketplace registration, installation and duplicate-install rejection.
2. Project/local visibility stays in its project; scoped removal preserves the user install.
3. Successful profile notices distinguish applied Markdown from ignored settings.
4. Plugin agent instructions appear exactly once in the actual provider request.
5. Unsupported YAML keys warn without rejecting the body; malformed reload preserves the snapshot.
6. CLI disable/enable updates discovery in an already-running private daemon.
7. Invalid updates preserve the working install; successful updates refresh live profile bytes.
8. Escaping relative/absolute and disguised remote-local sources reject without registry mutation.
9. A remote URL cannot fall back to a URL-shaped workspace directory; a Git stub prevents network access.
10. The actual TUI renders `/plugin`, all four tabs and the registered fixture row.
11. Actual TUI plugin skill and legacy-command invocation inserts their bodies and arguments into provider requests.
12. Real `/rewind 1` removes the later transcript entry while Up recalls the latest submitted prompt, the persisted journal retains that newest entry, Down restores the draft and Ctrl+R finds the rewound prompt.
13. `/clear` creates a new session whose Up recall does not expose the previous session's journal.
14. Uninstall removes live runtime discovery and marketplace removal clears the registry entry.

Evidence is retained at `/Users/siavash/.jcode/scratch/jpi-_tp_yco5/plugin-history-report.json`. The directory also contains CLI/wire transcripts, provider payloads, actual PTY output, plugin/search frames and pre/post-rewind journal snapshots. Cleanup confirmed the private daemon was reaped, the fake HTTP provider stopped, and no daemon or tester cleanup errors occurred. No live user daemon or actual user configuration was used.

Earlier failed runs remain separate evidence. In particular, `jpi-wt5or49r` and `jpi-yieaaaz_` reproduced an older replayed command argument overtaking newer submissions. The strict scenario was retained; the final rebuilt run passed after replay-only historical backfill and durable ordering were corrected.

The verified build `b53a1b730-dirty-fa5838ace24f` was subsequently activated; `selfdev status` confirmed both the running process and shared-server channel use it. Separate integrated suites passed 30 prompt-history tests and 46 rewind tests.

Large-prompt retention, including Unicode and inputs above 3 MiB, has source/module regression coverage. This end-user run used short prompts and is not an actual large-prompt load/performance test. It also does not prove concurrent multi-client compaction, persistent LSP lifecycle or every real external hook/MCP provider. The remaining compatibility gaps above still apply.

The frame test uses forced fresh-draw client debug requests plus actual PTY output. Visual frame JSON does not include modal dialog text. The exact successful command, from the repository root, was:

```bash
python3 scripts/tests/plugin_history_acceptance.py \
  --binary /Users/siavash/Projects/jcode/target/selfdev/jcode \
  --execute --diagnostic-hold-seconds 15
```

The script creates private HOME, JCODE_HOME, working directory, runtime directory and sockets; uses a loopback fake provider; and records CLI results, binary provenance, UI frames and a JSON report. It covers lifecycle/scopes, containment rejection, update rollback, actual profile instruction payloads, plugin slash invocation, the `/plugin` frame and recall across rewind/new-session boundaries. It does not need real provider credentials or the live user daemon. `--skip-tui` explicitly skips TUI coverage and must not be reported as a full pass. Never enable debug control in the real Jcode home to run this test.
