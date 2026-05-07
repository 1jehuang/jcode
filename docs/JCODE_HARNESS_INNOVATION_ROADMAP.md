# jcode-harness Innovation Roadmap

This roadmap turns the public jcode research pass into an implementation backlog for the standalone `chapzin/jcode-harness` project.

## Product thesis

Make jcode-harness the fastest local-first agent runtime for serious engineering workflows: human-friendly in the TUI, scriptable for automation, interoperable with editor clients, safe for first-time evaluation, and capable of preserving procedural knowledge over time.

## North-star capabilities

1. **Interop runtime**: ACP and headless JSONL/WebSocket APIs so editors, dashboards, Slack bridges, CI, and external orchestrators can drive jcode without scraping the TUI.
2. **Safe local trust boundary**: a first-run profile, trust center, MCP approval records, and auditable high-impact tool configuration.
3. **Plan-first swarm execution**: read-only planning in the side panel, explicit approval, then task graph dispatch to scoped workers.
4. **Skill OS**: deterministic skill manager, repo/task scoping, validation, import/export, and approved skill distillation from successful sessions.
5. **Living memory**: inspectable memory, wiki export/import, provenance, stale/conflict checks, and promotion from memories to decisions or skills.
6. **Cost/model autopilot**: thinking vs routine routing, provider health, failover, cache-cold events, and per-session cost telemetry.
7. **Reproducible demos**: mock-provider demos for memory, plan, swarm, skills, browser, and release gates that run without network credentials.
8. **Visual knowledge loop**: Mermaid plus Excalidraw/Obsidian bridge for editable architecture artifacts.

## Implementation phases

### Phase 0: Trustworthy onboarding

- `jcode-harness safe-eval`: create an isolated local evaluation profile with POSIX and PowerShell env files.
- `jcode doctor`: provider/auth/browser/MCP/Windows/config diagnostics with `--json`.
- Reproducible mock demos for README claims.
- Fix or document Windows beta caveats clearly.
- Stabilize compaction success/failure evidence.

### Phase 1: Programmatic runtime

- `jcode-harness session list --json`: first read-only headless runtime inventory for local/imported sessions without starting the TUI.
- `jcode-harness session spawn <goal> --dry-run --json`: safe new-session/run envelope without starting provider/TUI flow.
- `jcode-harness session attach <id> --dry-run --json`: safe local attach envelope without starting the TUI/provider flow.
- `jcode-harness session spawn|attach|resume --dry-run --ndjson`: deterministic JSONL events for dashboards and external orchestrators.
- `jcode-harness session show <id> --json`: read-only local jcode session metadata with opt-in bounded preview.
- `jcode-harness session resume <id> --dry-run --json`: safe local resume envelope without starting the TUI/provider flow.
- `jcode session list/spawn/attach/resume --json` follow-up slices.
- JSONL event stream for text/tool/usage/cache/compaction/memory events.
- Local WebSocket gateway for external dashboards.
- Initial `jcode acp` server with text/tool/done mapping.

### Phase 2: Plan-first swarm

- `/plan` and `jcode-harness plan --goal ... --json`.
- Read-only planning policy and side-panel rendering.
- Approval workflow and editable plan artifact.
- Convert plan steps into swarm task graph with worker skill/model scopes.

### Phase 3: Skill OS

- Skill import from `.agents`, `.claude`, `.codex`, `.jcode`.
- Repo-level skill scope file with visible/discoverable/blocked states.
- Skill validation, prompt-injection/secret scan, and tool allowlist enforcement.
- Manual `/distill-skill` MVP before automatic learning.

### Phase 4: Living memory

- `jcode memory inspect/search/export/purge/doctor --json`.
- LLM wiki sync/lint/export guidance and provenance checks.
- Memory-to-skill and memory-to-decision promotion flows.
- Optional compressed vector backend evaluation.

### Phase 5: Cost/model autopilot

- Configurable thinking/routine model routes.
- Provider failover policy with structured events.
- Cache-cold signal exposed to CLI/API/ACP clients.
- Session budget reporting and dry-run cost estimates.

### Phase 6: Distribution and community

- Submit to ACP registry after MVP is usable.
- Submit to awesome CLI coding agents lists.
- Publish reproducible benchmark scripts.
- Keep public roadmap issues small and contributor-friendly.

## Completed initial implementation slices

- `jcode-harness session show <id> --json`: read-only local session metadata and optional bounded preview without starting the TUI.
- `jcode-harness session spawn <goal> --dry-run --json`: safe `jcode run` argv/cwd envelope for new headless runs without executing provider/TUI flows.
- `jcode-harness session attach <id> --dry-run --json`: safe local attach argv/cwd envelope for external orchestrators without executing the TUI/provider flow.
- `jcode-harness session spawn|attach|resume --dry-run --ndjson`: deterministic `start`/`envelope`/`done` JSONL events over the existing safe dry-run envelopes.
- `jcode-harness session resume <id> --dry-run --json`: safe resume argv/cwd envelope for local jcode sessions without executing the TUI/provider flow.
- `jcode-harness session list --json`: read-only offline metadata inventory for local/imported sessions as the first programmatic runtime slice.
- `jcode-harness safe-eval`: isolated first-run trust boundary.
- `jcode-harness doctor`: offline onboarding diagnostics for safe-eval, privacy, skills, and MCP configs.
- `jcode-harness skills scope`: repo-local Skill OS policy for visible, discoverable, and blocked skill states.
- `jcode-harness skills import`: safe-by-default Skill OS import planner/apply path for `.agents`, `.claude`, `.codex`, and `.jcode` skill directories.
- `jcode-harness skills validate`: offline Skill OS validation for frontmatter, precedence, risky prompt patterns, suspicious secrets, and runtime-compatible tool allowlists.
