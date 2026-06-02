# Permission Modes Research Report

> Generated from deep research across 7 reference repos + dcg-core analysis
> Date: 2026-05-30
> Branch: experiment/dcg-permission-modes

---

## 1. Research Scope

| Repo | Stack | Permission Relevance |
|------|-------|---------------------|
| claude-code | TypeScript / Bun | ⭐⭐⭐ Direct ancestor, 6-mode pipeline |
| codex (OpenAI) | Rust | ⭐⭐⭐ OS sandboxing, exec policy engine |
| pi-agent-rust | Rust 2024 | ⭐⭐⭐ Enum-driven policy, O(1) snapshot |
| opencode | TypeScript / Bun | ⭐⭐ Layered ruleset, wildcard matching |
| oh-my-openagent | TypeScript | ⭐⭐ Hook-chain, CC mode compat |
| oh-my-pi | TypeScript + Rust | ⭐⭐ 3-tier/3-mode, ACP bridge |
| oh-my-claudecode | TypeScript / CC plugin | ⭐⭐⭐ Hook-based defense-in-depth, worker RBAC, execution modes |
| oh-my-codex | TypeScript / Codex plugin | ⭐⭐ Pass-through wrapper, madmax isolation, workflow exclusivity |
| codebuff | TypeScript | ⭐ Tool whitelist, propose-vs-execute |

---

## 2. Per-Repo Detailed Reports

### 2.1 claude-code (CCB) — ⭐⭐⭐ HIGH

**Relevance:** Direct ancestor của jcode. Contains the complete reference implementation for permission modes, command safety classification, sandboxing, user consent flows, dangerous command detection, and tool policy.

**Modes:** `plan` → `default` → `acceptEdits` → `dontAsk` → `auto` → `bypassPermissions` (6 modes, Shift+Tab cycling)

**Permission Pipeline (~1500 lines in `permissions.ts`):**
```
Step 1a: deny rules → deny
Step 1b: ask rules → ask
Step 1c: tool.checkPermissions() → tool-specific result
Step 1d: Tool denied → deny
Step 1e: requiresUserInteraction() → ask (even in bypass)
Step 1f: Content-specific ask rules → ask (even in bypass)
Step 1g: Safety checks (.git/, .claude/, shell configs) → ask (even in bypass)
Step 2a: bypassPermissions mode → allow all remaining
Step 2b: Always-allowed rules → allow
Step 3:  Convert passthrough → ask
Post: dontAsk→deny, auto→YOLO classifier, denial tracking (3 consecutive / 20 total → fallback)
```

**Key Files:**
| File | Role |
|------|------|
| `src/types/permissions.ts` | `PermissionMode` union, `PermissionBehavior`, `PermissionRule`, `PermissionDecision`, `YoloClassifierResult` |
| `src/utils/permissions/permissions.ts` | Main pipeline (~1500 lines), `hasPermissionsToUseTool()` entry point |
| `src/utils/permissions/permissionSetup.ts` | `initialPermissionModeFromCLI()` parses `--dangerously-skip-permissions` and `--permission-mode` |
| `src/utils/permissions/dangerousPatterns.ts` | `DANGEROUS_BASH_PATTERNS`: python, node, eval, sudo, curl, wget, git, kubectl, aws, gcloud... |
| `src/utils/permissions/filesystem.ts` | `DANGEROUS_FILES` (.gitconfig, .bashrc, .zshrc, .mcp.json) + `DANGEROUS_DIRECTORIES` (.git, .vscode, .claude) |
| `src/utils/permissions/denialTracking.ts` | `DENIAL_LIMITS = { maxConsecutive: 3, maxTotal: 20 }`, fallback to interactive prompt |
| `src/utils/permissions/yoloClassifier.ts` | `classifyYoloAction()` — LLM subagent auto-approval, two-stage (fast + thinking) |
| `src/utils/permissions/PermissionMode.ts` | Mode config map with titles/symbols/colors, `permissionModeFromString()` |
| `src/utils/permissions/getNextPermissionMode.ts` | Shift+Tab cycle logic |
| `src/utils/shell/readOnlyCommandValidation.ts` | ~1900 lines whitelist of safe git/gh/docker/rg commands with per-flag validation |
| `src/utils/sandbox/sandbox-adapter.ts` | `SandboxManager` wrapping `@anthropic-ai/sandbox-runtime` |
| `src/hooks/toolPermission/PermissionContext.ts` | Permission context factory with `handleUserAllow()`, `runHooks()`, `tryClassifier()` |
| `src/hooks/toolPermission/handlers/interactiveHandler.ts` | Interactive REPL permission handler: TUI dialog |
| `src/components/permissions/` | Full directory of permission request UI components (15+ files) |

**Architecture Pattern:** Layered permission pipeline with 6 modes, 3 decision outcomes. Rule sources priority: policySettings > flagSettings > userSettings > projectSettings > localSettings > cliArg > command > session.

**Gaps:** No formal policy DSL. Classifier prompts bundled (not readable). GrowthBook/Statsig feature flag dependency. BashTool internal permission checks in separate `@claude-code-best/builtin-tools` package.

---

### 2.2 codex (OpenAI) — ⭐⭐⭐ HIGH

**Relevance:** Rust-native, most comprehensive OS-level sandboxing, exec-policy rule engine, LLM guardian auto-reviewer. Directly applicable patterns for dcg-core evolution.

**Key Enums (Rust):**
```rust
AskForApproval { UnlessTrusted, OnFailure, OnRequest, Granular(Config), Never }
SandboxPolicy { DangerFullAccess, ReadOnly{network}, WorkspaceWrite{writable_roots,network}, ExternalSandbox }
ExecApprovalRequirement { Skip{bypass_sandbox,amendment}, NeedsApproval{reason,amendment}, Forbidden{reason} }
Decision { Allow, Prompt, Forbidden }
```

**Key Files:**
| File | Role |
|------|------|
| `codex-rs/protocol/src/protocol.rs` | Core enums: `AskForApproval`, `SandboxPolicy`, `GranularApprovalConfig` |
| `codex-rs/protocol/src/approvals.rs` | `ExecApprovalRequestEvent`, `GuardianAssessmentEvent`, amendment types |
| `codex-rs/core/src/exec_policy.rs` | `ExecPolicyManager`, `BANNED_PREFIX_SUGGESTIONS`, unmatched command decision flow |
| `codex-rs/core/src/guardian/mod.rs` | LLM-based auto-reviewer: risk assessment, 90s timeout, fail-closed, circuit breaker |
| `codex-rs/core/src/tools/sandboxing.rs` | `ExecApprovalRequirement`, `ApprovalStore`, approval flow orchestration |
| `codex-rs/core/src/safety.rs` | `SafetyCheck { AutoApprove, AskUser, Reject }`, `assess_patch_safety()` |
| `codex-rs/shell-command/src/command_safety/is_safe_command.rs` | Explicit safelist: cat, ls, grep, head, tail, git status/log/diff, rg, find... |
| `codex-rs/shell-command/src/command_safety/is_dangerous_command.rs` | `command_might_be_dangerous()`: rm -rf, sudo chains, Windows dangerous commands |
| `codex-rs/utils/cli/src/shared_options.rs` | `--sandbox`, `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`) |
| `codex-rs/utils/approval-presets/src/lib.rs` | 3 presets: Read Only, Default/Auto, Full Access |
| `codex-rs/config/src/permissions_toml.rs` | Named permission profiles with `extends` inheritance |
| `codex-rs/core/src/landlock.rs` | Linux sandbox (bubblewrap + landlock fallback) |
| `codex-rs/sandboxing/src/seatbelt.rs` | macOS Seatbelt policy generation |
| `codex-rs/tui/src/bottom_pane/approval_overlay.rs` | TUI approval modal |

**Architecture Pattern:** Defense-in-depth: CLI flags → Config TOML → PermissionProfile → OS Sandbox (bubblewrap/Seatbelt/WFP) → AskForApproval → Exec Policy Engine → Command Safety Classifier → Guardian Auto-Review → TUI Approval Overlay.

**Gaps:** Guardian is OpenAI-specific. No intermediate trust levels (only trusted/untrusted). No dry-run/preview mode. `--yolo` bypass has no audit trail.

---

### 2.3 pi-agent-rust — ⭐⭐⭐ HIGH

**Relevance:** Rust 2024 edition, same language as jcode. Enum-driven policy with O(1) hot path, WASM sandbox, graduated enforcement rollout. Patterns directly transferable.

**Key Enums (Rust):**
```rust
ExtensionPolicyMode { Strict, Prompt, Permissive }
PolicyDecision { Allow, Prompt, Deny }
PolicyProfile { Safe, Standard, Permissive }
EnforcementState { Allow, Harden, Prompt, Deny, Terminate }
ExtensionTrustState { Pending, Acknowledged, Trusted, Killed }
RolloutPhase { Shadow, LogOnly, EnforceNew, EnforceAll }
DangerousCommandClass { RecursiveDelete, ForkBomb, ReverseShell, DiskWipe, ... }
```

**Key Files:**
| File | Role |
|------|------|
| `src/extensions.rs` (~50K lines) | Core: all policy types, evaluation pipeline, exec mediation, dangerous command classifier |
| `src/permissions.rs` | JSON-file persistent store: Allow/Deny per extension+capability, file-lock, 0o600, expiry |
| `src/config.rs` | `ExtensionPolicyConfig`, resolution chain: CLI > env > config > default |
| `src/cli.rs` | `--extension-policy {safe|balanced|permissive}`, `--explain-extension-policy` |
| `src/extension_scoring.rs` | `RiskLevel { Low, Moderate, High, Critical }` for extension risk scoring |
| `src/resource_governor.rs` | Host-level resource admission (CPU, memory, FD, backpressure) |
| `src/pi_wasm.rs` | WASM runtime bridge with wasmtime, per-instance limits, memory caps |
| `docs/security/invariants.md` | Normative 5-stage decision pipeline (A-E) |

**5-Layer Precedence Chain:**
```
Layer 1: per-extension deny → Deny
Layer 2: global deny_caps → Deny
Layer 3: per-extension allow → Allow
Layer 4: global default_caps → Allow
Layer 5: mode fallback → Strict=Deny, Prompt=Prompt, Permissive=Allow
```

**Unique Patterns:**
- **O(1) PolicySnapshot**: Precompiled at dispatcher creation time, zero-cost hot-path lookup
- **Graduated Rollout**: Shadow→LogOnly→EnforceNew→EnforceAll with auto-rollback on false-positive rate
- **Dangerous Command Classes**: 10 classes (RecursiveDelete, DeviceWrite, ForkBomb, PipeToShell, SystemShutdown, PermissionEscalation, ProcessTermination, CredentialFileModification, DiskWipe, ReverseShell)
- **Per-Extension Quotas**: hostcalls/sec, max subprocesses, max write bytes, max HTTP requests

**Gaps:** No top-level agent permission mode (only extension-scoped). No `--dangerously-skip-permissions` CLI flag. Built-in tools lack classification. WASM sandbox is extensions-only.

---

### 2.4 opencode — ⭐⭐ HIGH

**Relevance:** Layered ruleset evaluation with wildcard matching, per-agent and per-session permissions, doom-loop detection, bash arity model. Clean architecture.

**Key Pattern:** `allow/deny/ask` per-tool rules with `findLast`-wins evaluation, default = `ask`.

**Key Files:**
| File | Role |
|------|------|
| `packages/opencode/src/permission/index.ts` | Core service: `ask()`, `reply()`, `list()` triad, deferred/pending state machine |
| `packages/core/src/permission.ts` | Pure evaluation: `evaluate()` with findLast + wildcard matching against merged rulesets |
| `packages/opencode/src/permission/arity.ts` | Bash arity dictionary: command prefix → token count for human-readable matching |
| `packages/opencode/src/config/permission.ts` | Config schema: actions are `ask/allow/deny`, known keys (read, edit, bash, glob, grep...) |
| `packages/opencode/src/agent/subagent-permissions.ts` | Derives subagent session permission from parent denies + external_directory rules |
| `packages/opencode/src/session/tools.ts` | Tool execution permission gate: merges agent.permission + session.permission |
| `packages/opencode/src/session/processor.ts` | Doom loop detection: 3 identical tool calls → `doom_loop` permission ask |
| `packages/opencode/src/cli/cmd/run.ts` | `--dangerously-skip-permissions` auto-approves; non-interactive auto-rejects |
| `packages/opencode/src/cli/cmd/tui/routes/session/permission.tsx` | Full TUI permission dialog: 3-stage flow (permission → always/reject) |

**Architecture Pattern:** Config → Agent → Session → Runtime-approved rules merge. `evaluate()` uses `findLast` wins with wildcard matching. Three-way ask: Allow Once / Always / Reject.

**Gaps:** No OS sandboxing. No persistent allowlist across restarts. No user/role-based access control. Policy system is experimental (provider-only). No audit log.

---

### 2.5 oh-my-openagent — ⭐⭐ HIGH

**Relevance:** Hook-chain permission system with Claude Code mode compatibility, multi-agent RBAC, per-agent tool restrictions, write-before-read guard.

**Permission Mode:** `PermissionMode = "default" | "plan" | "acceptEdits" | "bypassPermissions"` (CC compatible)

**Key Files:**
| File | Role |
|------|------|
| `src/config/schema/internal/permission.ts` | `PermissionValue` enum (`ask/allow/deny`), `AgentPermissionSchema` per-tool |
| `src/hooks/claude-code-hooks/types.ts` | `PermissionMode` + `PermissionDecision` + full Claude Code hooks interface |
| `src/hooks/claude-code-hooks/pre-tool-use.ts` | Pre-tool-use gate: exit codes (2=deny, 1=ask, 0=allow) |
| `src/hooks/write-existing-file-guard/hook.ts` | Prevents overwriting files without reading first (LRU: 256 sessions × 1024 paths) |
| `src/hooks/team-tool-gating/hook.ts` | Role-based access: lead-only ops, member-or-lead ops, participant-scoped |
| `src/hooks/bash-file-read-guard.ts` | Warns when bash used for simple file reads instead of Read tool |
| `src/shared/agent-tool-restrictions.ts` | Per-agent tool denylists (explore agents can't write/edit/task) |
| `src/features/claude-code-mcp-loader/configure-allowed-env-vars.ts` | MCP env allowlist with user-only security boundary |
| `src/plugin/tool-execute-before.ts` | Master pipeline: chains all 17+ guards sequentially |

**Architecture Pattern:** 5-layer enforcement: (1) config-level disable arrays, (2) per-agent denylists, (3) hook-chain pre-execution gates (17+ guards), (4) Claude Code hooks bridge (exit code mapping), (5) path/resource boundaries.

**Gaps:** No actual sandbox execution engine (schema only). No centralized policy engine. No runtime permission escalation. No audit logging.

---

### 2.6 oh-my-pi — ⭐⭐ HIGH

**Relevance:** Clean 3-tier/3-mode architecture with per-tool user overrides, ACP client-bridge permissions, critical bash pattern detection, plan mode guard.

**Key Types:**
```typescript
type ApprovalMode = "always-ask" | "write" | "yolo"  // default: yolo
type ToolTier = "read" | "write" | "exec"
type ApprovalPolicy = "allow" | "deny" | "prompt"
```

**Key Files:**
| File | Role |
|------|------|
| `packages/coding-agent/src/tools/approval.ts` | Core engine: `ApprovalMode`, `ToolTier`, `resolveApproval()`, mode-to-tier mapping |
| `packages/coding-agent/src/tools/bash.ts` | `CRITICAL_BASH_PATTERNS` (26 regex): rm -rf /, fork bombs, disk destruction, remote-fetch-then-execute... |
| `packages/coding-agent/src/tools/plan-mode-guard.ts` | Blocks renames/deletes/writes except to approved plan file |
| `packages/coding-agent/src/tools/auto-generated-guard.ts` | Blocks editing auto-generated files (protoc, sqlc, swagger...) |
| `packages/coding-agent/src/extensibility/extensions/wrapper.ts` | `ExtensionToolWrapper`: approval gate fires BEFORE extension handlers |
| `packages/coding-agent/src/session/client-bridge.ts` | ACP bridge: `allow_once/allow_always/reject_once/reject_always` semantics |
| `packages/coding-agent/src/config/settings-schema.ts` | `tools.approvalMode` + `tools.approval` per-tool override |
| `packages/coding-agent/src/cli/args.ts` | `--auto-approve` / `--yolo` / `--approval-mode` flags |
| `packages/coding-agent/examples/extensions/plan-mode.ts` | Full plan-mode extension: SAFE_COMMANDS (22) + DESTRUCTIVE_PATTERNS (35) |
| `docs/approval-mode.md` | Canonical documentation of approval system |

**Architecture Pattern:** 4 axes: (1) Tool self-declares tier (`read/write/exec`), (2) Global approval mode sets auto-approve threshold, (3) Per-tool user override always wins, (4) Extension/hook interception post-approval.

**Gaps:** No OS sandboxing. Protected paths only as hook examples. yolo is default (convenience > safety). No audit trail.

---

### 2.7 oh-my-claudecode — ⭐⭐⭐ HIGH

**Relevance:** Claude Code plugin with deeply layered permission/safety architecture: Bash command allowlisting, worker RBAC, execution mode mutual exclusion, SSRF prevention, security config strict mode, verification tiers, and session isolation.

**Key Pattern:** Hook-based defense-in-depth — 11 Claude Code lifecycle events intercepted, three decision layers (permission, security config, execution mode), worker RBAC (advisory), verification tiers with auto-security escalation.

**Key Files:**
| File | Role |
|------|------|
| `src/hooks/permission-handler/index.ts` | Central Bash permission handler: safe command regex patterns, dangerous shell char detection, heredoc auto-approval, repo path validation, reads `settings.local.json` allow/ask |
| `src/lib/security-config.ts` | `OMC_SECURITY=strict` master switch. Governs: tool path restriction, python sandbox, project skills disable, auto-update disable, hard max iterations, remote MCP disable, external LLM disable. Strict = one-way (only tightens) |
| `src/team/permissions.ts` | Worker RBAC: `WorkerPermissions` with `allowedPaths`, `deniedPaths`, `allowedCommands`, `maxFileSize`. Glob matching, secure deny-defaults (`.git/**`, `.env*`, `**/secrets/**`, `**/.ssh/**`) |
| `src/hooks/mode-registry/index.ts` | Execution mode mutual exclusion: autopilot vs autoresearch. Session-scoped state with tombstone TTL |
| `src/hooks/persistent-mode/index.ts` (~2185 lines) | "Boulder never stops" engine: hard max iteration enforcement, cancel signal TTL, stale state detection, circuit breakers, context limit escape hatches |
| `src/utils/ssrf-guard.ts` | URL validation blocking private IPs, loopback, link-local, IPv6-mapped, hex/octal encoded, cloud metadata paths |
| `src/verification/tier-selector.ts` | Three-tier verification: LIGHT (haiku, <5 files), STANDARD (sonnet), THOROUGH (opus, security/auth changes always THOROUGH) |
| `src/lib/session-isolation.ts` | `isStateForSession()` with strict vs lenient modes preventing cross-session state leakage |
| `hooks/hooks.json` | Full lifecycle hook registration: 11 events (UserPromptSubmit, SessionStart, PreToolUse, PermissionRequest, PostToolUse, Stop, SessionEnd...) |
| `src/team/governance.ts` | Governance flags: delegation_only, plan_approval_required, nested_teams_allowed, cleanup_requires_all_workers_inactive |

**Safe Command Patterns:**
```typescript
const SAFE_PATTERNS = [
  /^git (status|diff|log|branch|show|fetch)/,
  /^npm run (lint|build|check|typecheck)/,
  /^cargo (check|clippy|build)/,
  /^ls( |$)/,
  /^eslint /,  /^prettier /,
];
const DANGEROUS_SHELL_CHARS = /[;&|`$()<>\n\r\t\0\\{}\[\]*?~!#]/;
```

**Security Config Strict Mode:**
```typescript
const STRICT_OVERRIDES: SecurityConfig = {
  restrictToolPaths: true, pythonSandbox: true,
  disableProjectSkills: true, disableAutoUpdate: true,
  hardMaxIterations: 200, disableRemoteMcp: true,
  disableExternalLLM: true,
};
```

**Architecture Pattern:** 3-layer hook-based defense: (1) Permission Layer (Bash classification via regex + shell metachar detection, reads Claude native allow/ask lists), (2) Security Config Layer (`OMC_SECURITY=strict` master switch, one-way tightening), (3) Execution Mode Layer (mutual exclusion, hard iteration caps, circuit breakers). Worker RBAC is advisory (injected into prompts, not mechanically enforced).

**Gaps:** No OS-level sandbox. Worker permissions advisory-only. No `--dangerously-skip-permissions` equivalent. No per-tool granularity beyond Bash. No audit log for permission decisions.

---

### 2.8 oh-my-codex — ⭐⭐ MEDIUM

**Relevance:** Workflow orchestration wrapper for Codex/Claude/Gemini CLIs. Does NOT implement its own permission mode system — acts as pass-through translator. Has meaningful safety infrastructure: madmax isolation, MCP path traversal guards, workflow exclusivity, team worker permission bypass automation.

**Key Pattern:** Pass-through wrapper that translates upstream permission flags per CLI backend: `--madmax` → Codex `--dangerously-bypass-approvals-and-sandbox`, `--dangerously-skip-permissions` for Claude workers, `--approval-mode yolo` for Gemini workers.

**Key Files:**
| File | Role |
|------|------|
| `src/cli/constants.ts` | Core permission flag constants: `--madmax`, `--dangerously-bypass-approvals-and-sandbox`, `--dangerously-skip-permissions` |
| `src/cli/index.ts` | `shouldAutoIsolateMadmaxLaunch()` detects madmax → creates isolated run dirs with context-key locking. `createMadmaxIsolatedRoot()` with stale detection (30s timeout) |
| `src/team/tmux-session.ts` | `translateWorkerLaunchArgsForCli()`: per-CLI flag mapping. `shouldGrantExecutionBypassForRole()` bypasses only for `tools === 'execution'` agents. Auto-accept Claude bypass/trust prompts in tmux |
| `src/team/state.ts` | `PermissionsSnapshot` with `approval_mode`, `sandbox_mode`, `network_access`. `resolvePermissionsSnapshot()` reads from multiple env vars (OMX/CODEX/CLAUDE/OMX_SANDBOX) |
| `src/config/mcp-registry.ts` | MCP server registry with `approval_mode` pass-through to Claude Code settings |
| `src/config/codex-hooks.ts` | Full hook lifecycle: SessionStart, PreToolUse, PostToolUse, Stop. Trust state (hash-based) for managed hooks |
| `src/mcp/hermes-bridge.ts` | `SAFE_ARTIFACT_PREFIXES` whitelist, `normalizeArtifactRelativePath()` rejects `../`, NUL bytes, absolute paths |
| `src/mcp/state-paths.ts` | Path traversal defense: strict regex for session IDs, `enforceWorkingDirectoryPolicy()` restricts MCP dirs to `OMX_MCP_WORKDIR_ROOTS` |
| `src/state/workflow-transition.ts` | Exclusive workflow mode enforcement: `evaluateWorkflowTransition()` with allow-overlap matrix for ralph/autopilot/team/ultrawork modes |
| `src/modes/base.ts` | 8 execution modes: autopilot, autoresearch, deep-interview, ralph, ultrawork, team, ultraqa, ralplan |

**Permission Flag Translation:**
```typescript
// Codex workers: no special flags (inherits upstream)
// Gemini workers: --approval-mode yolo (for execution role)
// Claude workers: --dangerously-skip-permissions (for execution role)
export function translateWorkerLaunchArgsForCli(workerCli, args, prompt, role) {
  if (workerCli === 'codex') return [...args];
  if (workerCli === 'gemini') return shouldGrantExecutionBypassForRole(role)
    ? [GEMINI_APPROVAL_MODE_FLAG, GEMINI_APPROVAL_MODE_YOLO] : [];
  return shouldGrantExecutionBypassForRole(role) ? [CLAUDE_SKIP_PERMISSIONS_FLAG] : [];
}
```

**Architecture Pattern:** Pass-through wrapper: (1) Translates upstream permission flags per CLI backend, (2) Auto-isolates madmax launches with context-key locking, (3) Automates permission prompt acceptance in tmux team workers, (4) Enforces workflow exclusivity (mode mutual exclusion), (5) Guards MCP surfaces with path traversal defense and working directory policy.

**Gaps:** No custom permission mode enum (relies on upstream). No tool-level classification or allowlist/denylist. No user consent prompt flow (auto-accepts). No policy config file. No sandboxing/containerization. No audit trail.

---

### 2.9 codebuff — ⭐ MEDIUM

**Relevance:** Per-agent tool whitelist with runtime enforcement, propose-vs-execute two-phase editing, organization RBAC. No user-facing permission mode enum.

**Key Files:**
| File | Role |
|------|------|
| `packages/agent-runtime/src/tools/tool-executor.ts` | Runtime enforcement: checks `agentTemplate.toolNames.includes(toolName)`, errors if unauthorized |
| `packages/agent-runtime/src/templates/strings.ts` | Subagent restriction message: "You only have access to tools: X" |
| `packages/agent-runtime/src/tools/handlers/tool/spawn-agent-utils.ts` | `spawnableAgents[]` validation: parent can only spawn whitelisted children |
| `common/src/tools/params/tool/run-terminal-command.ts` | Prompt-based safety rules (DO NOT list) — not code-enforced |
| `common/src/tools/params/tool/propose-str-replace.ts` | Propose tool: creates diff preview without writing |
| `agents/editor/best-of-n/editor-implementor.ts` | Restricted agent: only `propose_write_file`, `propose_str_replace` |
| `web/src/lib/organization-permissions.ts` | RBAC: member < admin < owner role hierarchy |

**Architecture Pattern:** 4 layers: (1) Agent template tool whitelist (structural), (2) Spawnable agent allowlist (delegation), (3) Propose-then-apply (separation of duty), (4) Prompt-based safety (LLM guidance, not enforced).

**Gaps:** No user-facing permission mode enum. No sandboxing. No protected path enforcement. No tool classification by danger level. No escalation flow. Prompt-based safety is bypassable.

---

### 2.10 dcg-core (current state) — ⭐⭐⭐ BASE LIBRARY

**Relevance:** Core library that jcode depends on. Already provides Engine, Effect, ToolCall, Mode, Decision, Session, ProtectedPaths.

**Already Available (v0.6.0-rc.1):**
| Feature | File | Status |
|---------|------|--------|
| `Mode` enum (6 variants + `pre_check()`) | `mode.rs` | ✅ Complete |
| `Effect` enum (7 variants + `is_read_only()` + `is_subset()`) | `effect.rs` | ✅ Complete |
| `ToolCall` enum (5 variants) | `tool_call.rs` | ✅ Complete |
| `Decision` tri-state (Allow/Prompt/Deny with reasons + alternatives) | `decision.rs` | ✅ Complete |
| `Engine::evaluate()` pipeline | `engine.rs` | ✅ Complete |
| `EngineConfig` builder (working_dir + protected_paths) | `engine.rs` | ✅ Complete |
| `Session` (allow-once codes + per-command deny counter) | `session.rs` | ✅ Complete |
| `ProtectedPaths` prefix matcher | `protected_paths.rs` | ✅ Complete |

**Evaluation Pipeline:**
```
1. Resolve path against protected-paths list
2. Mode::pre_check() (short-circuit for Bypass/Plan/AcceptEdits+protected)
3. Fallthrough: mode.fallthrough_allows() decides Allow vs Deny+deny_counter bump
```

**Needs Building (Phase 2):**
- Dangerous command patterns (26-50 regex, severity, alternatives)
- Safe command whitelist (known-safe read-only commands)
- Denial escalation logic (use existing deny_counter → escalate after N)
- Session-wide denial budget (track total denials, not just per-command)
- Pack rule integration (50+ security packs from dcg-cli)
- YOLO classifier trait (interface for consumer to inject LLM)
- Per-tool user overrides (TOML config for allow/deny/prompt)
- Network policy (host allowlist/denylist)

---

## 3. Cross-Repo Comparison Tables

### 3.1 Mode Enums

| Repo | Modes | Count |
|------|-------|-------|
| **claude-code** | `plan`, `default`, `acceptEdits`, `dontAsk`, `auto`, `bypassPermissions` | 6 |
| **codex** | `UnlessTrusted`, `OnFailure`, `OnRequest`, `Granular(Config)`, `Never` | 5 |
| **pi-agent-rust** | `Strict`, `Prompt`, `Permissive` (extension policy) | 3 |
| **opencode** | No mode enum; `allow/deny/ask` per-tool rules + `--dangerously-skip-permissions` | N/A |
| **oh-my-pi** | `always-ask`, `write`, `yolo` (approval mode) | 3 |
| **oh-my-openagent** | `default`, `plan`, `acceptEdits`, `bypassPermissions` (CC compat) | 4 |
| **codebuff** | No user-facing mode enum | 0 |

### 3.2 Decision Outcomes

All repos converge on **tri-state decision**:

| Decision | claude-code | codex | pi-agent-rust | opencode | oh-my-pi |
|----------|-------------|-------|---------------|----------|----------|
| Allow | ✅ | ✅ Allow | ✅ | ✅ | ✅ |
| Prompt/Ask | ✅ | ✅ Prompt | ✅ | ✅ | ✅ |
| Deny/Forbidden | ✅ | ✅ Forbidden | ✅ | ✅ | ✅ |

### 3.3 Tool Classification

| Repo | Classification Method | Tiers |
|------|----------------------|-------|
| **claude-code** | Per-tool `checkPermissions()` + effect-based + dangerous patterns | Read/Write/Exec + Destructive |
| **codex** | `is_known_safe_command()` whitelist + `command_might_be_dangerous()` + exec policy rules | Safe/Unsafe/Forbidden |
| **pi-agent-rust** | `DangerousCommandClass` (10 classes) + `ExecRiskTier` (High/Critical) + `Effect` (7 variants) | 10 danger classes |
| **oh-my-pi** | `ToolTier` self-declared: `read/write/exec` + `CRITICAL_BASH_PATTERNS` (26 regex) | Read/Write/Exec + Critical |
| **opencode** | Per-tool permission keys (read, edit, bash, glob...) | Per-tool |
| **oh-my-openagent** | Per-agent denylist + hook-chain guards | Per-agent |

---

## 4. Proven Patterns (consistent across 3+ repos)

### 4.1 Enum-Driven Mode + Pre-Check Fast Path

Every mature system uses an enum to represent the active mode, with a `pre_check()` or equivalent that short-circuits before expensive evaluation:

```
pre_check(mode, effects) → AllowImmediately | DenyImmediately | PromptImmediately | Continue
```

**Sources:** claude-code (`PermissionMode`), codex (`AskForApproval`), pi-agent-rust (`ExtensionPolicyMode`), dcg-core (`Mode::pre_check()`)

### 4.2 Effect Taxonomy

Tag every tool call with effects, then mode determines which effect sets auto-allow:

| Effect | Who uses it |
|--------|-------------|
| `Read` | All 7 repos |
| `Write` | All 7 repos |
| `Exec/Spawn` | claude-code, codex, oh-my-pi, pi-agent-rust |
| `Irreversible` | claude-code, pi-agent-rust, dcg-core |
| `Network` | claude-code, codex, oh-my-pi, dcg-core |
| `MutateVcs` | dcg-core |
| `Fs` | claude-code, dcg-core |

**dcg-core already has this:** `Effect` enum with 7 variants + `is_read_only()` + `is_subset()`.

### 4.3 ToolCall Abstraction

Normalize agent-specific tool names into a common taxonomy:

| Variant | Maps from |
|---------|-----------|
| `Bash { cmd }` | Shell, terminal, run_terminal_cmd, execute_command |
| `Edit { path }` | MultiEdit, ApplyPatch, str_replace |
| `Write { path }` | write_file, create_or_update_file |
| `Read { path }` | read_file, glob, grep, ls |
| `Network { url }` | webfetch, websearch, browser |

**dcg-core already has this:** `ToolCall` enum with 5 variants.

### 4.4 `--dangerously-skip-permissions` Escape Hatch

Universal across all TypeScript-based agents:

| Repo | Flag |
|------|------|
| claude-code | `--dangerously-skip-permissions` |
| opencode | `--dangerously-skip-permissions` |
| codex | `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`) |
| oh-my-pi | `--yolo` / `--auto-approve` |

**jcode already has this:** `--dangerously-skip-permissions` (added in this branch).

### 4.5 Per-Tool User Overrides

Users can set `allow/deny/prompt` per tool in config, overriding mode baseline:

| Repo | Config location |
|------|----------------|
| claude-code | `permissions.allow/deny/ask` arrays in settings.json |
| opencode | `permission.bash: "allow"` in config |
| oh-my-pi | `tools.approval.<toolName>: allow|deny|prompt` |
| codex | Named permission profiles in TOML |

### 4.6 Dangerous Command Detection

Regex/pattern-based detection of unsafe commands before execution:

| Repo | Count | Method |
|------|-------|--------|
| claude-code | ~30+ patterns | `DANGEROUS_BASH_PATTERNS` regex array |
| oh-my-pi | 26 patterns | `CRITICAL_BASH_PATTERNS` regex array |
| pi-agent-rust | 10 classes | `DangerousCommandClass` enum |
| codex | ~50+ commands | `is_known_safe_command()` + `command_might_be_dangerous()` |

### 4.7 Denial Tracking / Circuit Breaker

When auto mode keeps denying, fall back to interactive prompt:

| Repo | Threshold | Scope |
|------|-----------|-------|
| claude-code | 3 consecutive / 20 total | Per session |
| codex | 3 per turn | Per turn |
| dcg-core | Per-command counter (exists, escalation not yet wired) | Per command hash |

### 4.8 Session-Scoped Approval Caching

Avoid re-prompting the same action within a session:

| Repo | Mechanism |
|------|-----------|
| codex | `ApprovedForSession` decision |
| opencode | Runtime-approved rules in memory |
| claude-code | Tool permission context with cached rules |
| dcg-core | Allow-once codes (6-char hex, SHA-256 derived, 24h TTL) |

### 4.9 Subagent Permission Restriction

Children inherit restricted subset of parent rules:

| Repo | Method |
|------|--------|
| opencode | Derive from parent denies + external_directory rules |
| oh-my-pi | Subagents forced to yolo (parent = auth boundary) |
| oh-my-openagent | Per-agent denylists + team denylist |
| codebuff | Agent template `toolNames[]` + `spawnableAgents[]` |

### 4.10 Mode Cycling (TUI)

Runtime mode switching via keyboard shortcut:

| Repo | Shortcut | Cycle |
|------|----------|-------|
| claude-code | Shift+Tab | default → acceptEdits → plan → auto → bypassPermissions → default |
| codex | Not available | N/A |
| Others | Not available | N/A |

---

## 5. Unique / Novel Patterns (single repo)

| Pattern | Repo | Description |
|---------|------|-------------|
| **YOLO/Auto Classifier** | claude-code | LLM subagent classifies actions as safe/unsafe. Two-stage (fast + thinking). Iron-gate fail-closed. |
| **Guardian Auto-Reviewer** | codex | Separate LLM risk assessment with 90s timeout, fail-closed, circuit breaker (3 denials/turn). |
| **O(1) PolicySnapshot** | pi-agent-rust | Precompiled policy for zero-cost hot-path lookup. Built once at dispatcher creation. |
| **Graduated Rollout** | pi-agent-rust | Shadow → LogOnly → EnforceNew → EnforceAll. Auto-rollback on false-positive rate. |
| **Named Permission Profiles** | codex | TOML profiles with `extends` inheritance. `read-only`, `auto`, `full-access` presets. |
| **Bash Arity Model** | opencode | Command prefix → token count → human-readable matching. `docker compose up` = arity 3. |
| **Write-Before-Read Guard** | oh-my-openagent | LRU tracking prevents blind file overwrites (256 sessions × 1024 paths). |
| **Propose-vs-Execute** | codebuff | Implementor proposes, selector applies. Separation of duty pattern. |
| **Model-Visible Context** | codex | Permission policy injected into LLM system prompt as structured instructions. |
| **MCP Env Allowlist Security** | oh-my-openagent | User-only config boundary prevents project-level injection. |

---

## 6. dcg-core Status: Has vs Needs

### ✅ Already Available in dcg-core v0.6.0-rc.1

| Feature | File | Status |
|---------|------|--------|
| `Mode` enum (6 variants) | `mode.rs` | Complete with `pre_check()` fast path |
| `Effect` enum (7 variants) | `effect.rs` | Complete with `is_read_only()` + `is_subset()` |
| `ToolCall` enum (5 variants) | `tool_call.rs` | Bash/Edit/Write/Read/Network |
| `Decision` tri-state | `decision.rs` | Allow/Prompt{reason,alternatives}/Deny{reason,alternatives} |
| `Engine::evaluate()` | `engine.rs` | Mode → pre_check → protected_paths → fallthrough |
| `EngineConfig` builder | `engine.rs` | working_dir + protected_paths |
| `Session` state | `session.rs` | Allow-once codes (6-char hex, 24h TTL) + per-command deny counter |
| `ProtectedPaths` matcher | `protected_paths.rs` | Prefix matching, `~` expansion, canonicalization |

### 🔨 Needs Building (Phase 2 dcg-core)

| Feature | Priority | Notes |
|---------|----------|-------|
| Dangerous command patterns | P0 | 26-50 regex patterns, severity levels, safer alternatives |
| Safe command whitelist | P0 | Known-safe read-only commands (cat, ls, grep, git status...) |
| Denial escalation logic | P1 | Use existing `deny_counter` → escalate after N denials |
| Session-wide denial budget | P1 | Track total denials across commands, not just per-command |
| Pack rule integration | P2 | Move dcg-cli's 50+ security packs into dcg-core |
| YOLO classifier trait | P2 | Define trait, let consumer inject LLM provider |
| Per-tool user overrides | P2 | TOML config for allow/deny/prompt per tool pattern |
| Network policy | P3 | Host allowlist/denylist for network calls |

### 🏗️ Needs Building in jcode

| Feature | Priority | Notes |
|---------|----------|-------|
| TUI mode cycling (Shift+Tab) | P0 | Cycle through 6 modes at runtime |
| TUI permission dialogs | P0 | Allow/Deny/Always-allow for Prompt decisions |
| CLI `--permission-mode` flag | ✅ Done | Already implemented |
| CLI `--dangerously-skip-permissions` | ✅ Done | Already implemented |
| dcg_bridge wiring | ✅ Done | Already implemented |
| YOLO classifier implementation | P2 | Implement trait from dcg-core, inject active provider |
| MCP permission pipeline | P3 | Unified or separate, TBD |
| Protected paths config | P1 | Default CC paths + user-configurable TOML |

---

## 7. Architecture Recommendation

### 7.1 Layered Pipeline (recommended)

```
CLI flags (--permission-mode, --dangerously-skip-permissions)
    │
    ▼
Config (TOML: default mode, per-tool overrides, protected paths)
    │
    ▼
dcg-core Engine::evaluate(session, tool_call, mode, effects)
    │
    ├─► Mode::pre_check() ─► AllowImmediately / DenyImmediately / Continue
    │
    ├─► Protected paths check ─► Deny if target in protected list
    │
    ├─► [Phase 2] Pack rule evaluation ─► Allow/Deny by pattern
    │
    ├─► Dangerous command detection ─► Escalate severity
    │
    ├─► Safe command whitelist ─► Auto-approve known-safe
    │
    ├─► [Phase 2] YOLO classifier trait ─► LLM auto-approve
    │
    ├─► Denial escalation ─► Prompt after N denials
    │
    └─► Decision: Allow / Prompt / Deny
         │
         ▼
    jcode TUI: Auto-approve (Allow) / Show dialog (Prompt) / Block (Deny)
```

### 7.2 Config Hierarchy

```
CLI flag > JCODE_PERMISSION_MODE env > .jcode/config.toml > ~/.jcode/config.toml > Mode::Default
```

### 7.3 TOML Config Schema (proposed)

```toml
[permissions]
# Default mode when no CLI flag
default_mode = "default"

# Protected paths (always prompt, regardless of mode)
protected_paths = ["~/.ssh", "~/.aws", "~/.config/gh", ".git", ".env"]

# Per-tool overrides (win over mode baseline)
[permissions.tools]
bash = "prompt"          # Always prompt for bash
edit = "allow"           # Always allow edits
read = "allow"           # Always allow reads
webfetch = "prompt"      # Always prompt for network

# Denial tracking
[permissions.denial]
max_consecutive = 3
max_total = 20
```

---

## 8. Open Questions (need further discussion)

1. **YOLO classifier design** — Trait-based in dcg-core vs all in jcode? What LLM provider? How to keep dcg clean?
2. **MCP permission pipeline** — Unified with core tools or separate system?
3. **Sandboxing** — Not in scope for now, but what's the future plan?
4. **Pack rules priority** — When should Phase 2 pack integration happen relative to other work?
5. **Multi-agent/swarm** — How do subagents inherit permissions from parent?

---

## 9. Reference Links

### claude-code
- Permission pipeline: https://github.com/claude-code-best/claude-code/blob/main/src/utils/permissions/permissions.ts
- Mode enum: https://github.com/claude-code-best/claude-code/blob/main/src/types/permissions.ts
- Setup: https://github.com/claude-code-best/claude-code/blob/main/src/utils/permissions/permissionSetup.ts
- Dangerous patterns: https://github.com/claude-code-best/claude-code/blob/main/src/utils/permissions/dangerousPatterns.ts
- Denial tracking: https://github.com/claude-code-best/claude-code/blob/main/src/utils/permissions/denialTracking.ts
- YOLO classifier: https://github.com/claude-code-best/claude-code/blob/main/src/utils/permissions/yoloClassifier.ts
- Sandbox: https://github.com/claude-code-best/claude-code/blob/main/src/utils/sandbox/sandbox-adapter.ts

### codex
- AskForApproval enum: https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs#L787
- SandboxPolicy enum: https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs#L881
- Exec policy: https://github.com/openai/codex/blob/main/codex-rs/core/src/exec_policy.rs
- Guardian: https://github.com/openai/codex/blob/main/codex-rs/core/src/guardian/mod.rs
- Safe commands: https://github.com/openai/codex/blob/main/codex-rs/shell-command/src/command_safety/is_safe_command.rs

### pi-agent-rust
- Policy modes: https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/extensions.rs#L2129
- Policy profiles: https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/extensions.rs#L2051
- Dangerous commands: https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/extensions.rs#L3855
- Rollout phases: https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/extensions.rs#L2229
- Permission store: https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/permissions.rs

### opencode
- Permission service: https://github.com/anomalyco/opencode/blob/main/packages/opencode/src/permission/index.ts
- Evaluation engine: https://github.com/anomalyco/opencode/blob/main/packages/core/src/permission.ts
- Config schema: https://github.com/anomalyco/opencode/blob/main/packages/opencode/src/config/permission.ts
- Subagent permissions: https://github.com/anomalyco/opencode/blob/main/packages/opencode/src/agent/subagent-permissions.ts

### oh-my-pi
- Approval engine: https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/tools/approval.ts
- Critical bash patterns: https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/tools/bash.ts
- Plan mode guard: https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/tools/plan-mode-guard.ts

### oh-my-openagent
- Permission types: https://github.com/code-yeongyu/oh-my-openagent/blob/main/src/config/schema/internal/permission.ts
- CC hooks types: https://github.com/code-yeongyu/oh-my-openagent/blob/main/src/hooks/claude-code-hooks/types.ts
- Write guard: https://github.com/code-yeongyu/oh-my-openagent/blob/main/src/hooks/write-existing-file-guard/hook.ts

### dcg-core (local)
- Engine: /data/projects/destructive_command_guard/crates/dcg-core/src/engine.rs
- Mode: /data/projects/destructive_command_guard/crates/dcg-core/src/mode.rs
- Effect: /data/projects/destructive_command_guard/crates/dcg-core/src/effect.rs
- Decision: /data/projects/destructive_command_guard/crates/dcg-core/src/decision.rs
- Session: /data/projects/destructive_command_guard/crates/dcg-core/src/session.rs
- ToolCall: /data/projects/destructive_command_guard/crates/dcg-core/src/tool_call.rs
- ProtectedPaths: /data/projects/destructive_command_guard/crates/dcg-core/src/protected_paths.rs
