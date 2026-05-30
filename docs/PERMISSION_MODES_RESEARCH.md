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
| codebuff | TypeScript | ⭐ Tool whitelist, propose-vs-execute |

---

## 2. Cross-Repo Permission Mode Comparison

### 2.1 Mode Enums

| Repo | Modes | Count |
|------|-------|-------|
| **claude-code** | `plan`, `default`, `acceptEdits`, `dontAsk`, `auto`, `bypassPermissions` | 6 |
| **codex** | `UnlessTrusted`, `OnFailure`, `OnRequest`, `Granular(Config)`, `Never` | 5 |
| **pi-agent-rust** | `Strict`, `Prompt`, `Permissive` (extension policy) | 3 |
| **opencode** | No mode enum; `allow/deny/ask` per-tool rules + `--dangerously-skip-permissions` | N/A |
| **oh-my-pi** | `always-ask`, `write`, `yolo` (approval mode) | 3 |
| **oh-my-openagent** | `default`, `plan`, `acceptEdits`, `bypassPermissions` (CC compat) | 4 |
| **codebuff** | No user-facing mode enum | 0 |

### 2.2 Decision Outcomes

All repos converge on **tri-state decision**:

| Decision | claude-code | codex | pi-agent-rust | opencode | oh-my-pi |
|----------|-------------|-------|---------------|----------|----------|
| Allow | ✅ | ✅ Allow | ✅ | ✅ | ✅ |
| Prompt/Ask | ✅ | ✅ Prompt | ✅ | ✅ | ✅ |
| Deny/Forbidden | ✅ | ✅ Forbidden | ✅ | ✅ | ✅ |

### 2.3 Tool Classification

| Repo | Classification Method | Tiers |
|------|----------------------|-------|
| **claude-code** | Per-tool `checkPermissions()` + effect-based + dangerous patterns | Read/Write/Exec + Destructive |
| **codex** | `is_known_safe_command()` whitelist + `command_might_be_dangerous()` + exec policy rules | Safe/Unsafe/Forbidden |
| **pi-agent-rust** | `DangerousCommandClass` (10 classes) + `ExecRiskTier` (High/Critical) + `Effect` (7 variants) | 10 danger classes |
| **oh-my-pi** | `ToolTier` self-declared: `read/write/exec` + `CRITICAL_BASH_PATTERNS` (26 regex) | Read/Write/Exec + Critical |
| **opencode** | Per-tool permission keys (read, edit, bash, glob...) | Per-tool |
| **oh-my-openagent** | Per-agent denylist + hook-chain guards | Per-agent |

---

## 3. Proven Patterns (consistent across 3+ repos)

### 3.1 Enum-Driven Mode + Pre-Check Fast Path

Every mature system uses an enum to represent the active mode, with a `pre_check()` or equivalent that short-circuits before expensive evaluation:

```
pre_check(mode, effects) → AllowImmediately | DenyImmediately | PromptImmediately | Continue
```

**Sources:** claude-code (`PermissionMode`), codex (`AskForApproval`), pi-agent-rust (`ExtensionPolicyMode`), dcg-core (`Mode::pre_check()`)

### 3.2 Effect Taxonomy

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

### 3.3 ToolCall Abstraction

Normalize agent-specific tool names into a common taxonomy:

| Variant | Maps from |
|---------|-----------|
| `Bash { cmd }` | Shell, terminal, run_terminal_cmd, execute_command |
| `Edit { path }` | MultiEdit, ApplyPatch, str_replace |
| `Write { path }` | write_file, create_or_update_file |
| `Read { path }` | read_file, glob, grep, ls |
| `Network { url }` | webfetch, websearch, browser |

**dcg-core already has this:** `ToolCall` enum with 5 variants.

### 3.4 `--dangerously-skip-permissions` Escape Hatch

Universal across all TypeScript-based agents:

| Repo | Flag |
|------|------|
| claude-code | `--dangerously-skip-permissions` |
| opencode | `--dangerously-skip-permissions` |
| codex | `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`) |
| oh-my-pi | `--yolo` / `--auto-approve` |

**jcode already has this:** `--dangerously-skip-permissions` (added in this branch).

### 3.5 Per-Tool User Overrides

Users can set `allow/deny/prompt` per tool in config, overriding mode baseline:

| Repo | Config location |
|------|----------------|
| claude-code | `permissions.allow/deny/ask` arrays in settings.json |
| opencode | `permission.bash: "allow"` in config |
| oh-my-pi | `tools.approval.<toolName>: allow|deny|prompt` |
| codex | Named permission profiles in TOML |

### 3.6 Dangerous Command Detection

Regex/pattern-based detection of unsafe commands before execution:

| Repo | Count | Method |
|------|-------|--------|
| claude-code | ~30+ patterns | `DANGEROUS_BASH_PATTERNS` regex array |
| oh-my-pi | 26 patterns | `CRITICAL_BASH_PATTERNS` regex array |
| pi-agent-rust | 10 classes | `DangerousCommandClass` enum |
| codex | ~50+ commands | `is_known_safe_command()` + `command_might_be_dangerous()` |

### 3.7 Denial Tracking / Circuit Breaker

When auto mode keeps denying, fall back to interactive prompt:

| Repo | Threshold | Scope |
|------|-----------|-------|
| claude-code | 3 consecutive / 20 total | Per session |
| codex | 3 per turn | Per turn |
| dcg-core | Per-command counter (exists, escalation not yet wired) | Per command hash |

### 3.8 Session-Scoped Approval Caching

Avoid re-prompting the same action within a session:

| Repo | Mechanism |
|------|-----------|
| codex | `ApprovedForSession` decision |
| opencode | Runtime-approved rules in memory |
| claude-code | Tool permission context with cached rules |
| dcg-core | Allow-once codes (6-char hex, SHA-256 derived, 24h TTL) |

### 3.9 Subagent Permission Restriction

Children inherit restricted subset of parent rules:

| Repo | Method |
|------|--------|
| opencode | Derive from parent denies + external_directory rules |
| oh-my-pi | Subagents forced to yolo (parent = auth boundary) |
| oh-my-openagent | Per-agent denylists + team denylist |
| codebuff | Agent template `toolNames[]` + `spawnableAgents[]` |

### 3.10 Mode Cycling (TUI)

Runtime mode switching via keyboard shortcut:

| Repo | Shortcut | Cycle |
|------|----------|-------|
| claude-code | Shift+Tab | default → acceptEdits → plan → auto → bypassPermissions → default |
| codex | Not available | N/A |
| Others | Not available | N/A |

---

## 4. Unique / Novel Patterns (single repo)

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

## 5. dcg-core Status: Has vs Needs

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

## 6. Architecture Recommendation

### 6.1 Layered Pipeline (recommended)

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

### 6.2 Config Hierarchy

```
CLI flag > JCODE_PERMISSION_MODE env > .jcode/config.toml > ~/.jcode/config.toml > Mode::Default
```

### 6.3 TOML Config Schema (proposed)

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

## 7. Open Questions (need further discussion)

1. **YOLO classifier design** — Trait-based in dcg-core vs all in jcode? What LLM provider? How to keep dcg clean?
2. **MCP permission pipeline** — Unified with core tools or separate system?
3. **Sandboxing** — Not in scope for now, but what's the future plan?
4. **Pack rules priority** — When should Phase 2 pack integration happen relative to other work?
5. **Multi-agent/swarm** — How do subagents inherit permissions from parent?

---

## 8. Reference Links

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
