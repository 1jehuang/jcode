# E-usability.md — jcode: what a real user can actually do today

Read-only audit. No files modified. No `cargo build`/`cargo test` run. Repo: `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode` (KooshaPari fork of `1jehuang/jcode`, latest declared fork release `v0.85.0-k1.0.0` per README L15; working tree at `jcode v0.9.1888-dev (be386f2)` per README L279). Date: 2026-10-16.

---

## 1. INSTALL

### What the user sees in the README

`README.md` (L33-59, L880-934) advertises four real paths:

1. **`curl -fsSL https://jcode.sh/install | bash`** (macOS / Linux / Git-Bash-on-Windows / Termux) — the blessed default.
2. **`irm https://jcode.sh/install.ps1 | iex`** (Windows PowerShell 5.1+).
3. **`brew tap 1jehuang/jcode && brew install jcode`** (macOS via Homebrew).
4. **GitHub Releases** manual download (`https://github.com/KooshaPari/jcode/releases`) — prebuilt `.tar.gz` or single binary per platform (`jcode-linux-x86_64`, `jcode-linux-aarch64`, `jcode-macos-arm64`, `jcode-macos-x86_64`, `jcode-windows-x86_64`, `jcode-windows-aarch64`).
5. **Bun**: `bunx jcode-install`.
6. **Docker**: `ghcr.io/KooshaPari/jcode/jcode-linux-x86_64:<version>`.
7. **From source**: `git clone … && cargo build --release` (README L949-997).

### What the install script actually does

`scripts/install.sh` (567 lines) is a real, production-grade installer. It:

- Detects OS / arch (Linux x86_64+aarch64, macOS arm64+x86_64, Windows x86_64+aarch64 via env, MSYS/MINGW/Cygwin).
- Resolves the latest stable tag from GitHub's redirect URL **and** falls back to a static `https://jcode.sh/releases/latest/version` if GitHub is throttled (install.sh L155-172). Neither path uses the rate-limited unauthenticated GitHub API.
- Downloads the per-platform asset (tries `.tar.gz` then single-binary), verifies against `SHA256SUMS` from `jcode.sh` first then GitHub (L236-254).
- If **no prebuilt asset exists** for the platform, falls back to `git clone --depth 1` + `cargo build --release` (L275-289). This is the "build from source" escape hatch — but it is invoked **automatically** when no binary is found, not as a separate user choice.
- Installs into `~/.local/bin/jcode` (or `$LOCALAPPDATA/jcode/bin` on Windows); backs up `~/.jcode/builds/` and creates `stable` + `current` symlinks.
- Special-cases **macOS**: strips `com.apple.quarantine` / `com.apple.provenance` xattrs (the macOS 26+ taskgated rejection mentioned in comments at L336-343), re-adhoc-codesigns, and runs `jcode setup-launcher` to install a notification broker (L335-375).
- Special-cases **Termux**: requires `glibc` + `patchelf`, rewrites ELF interpreter to Termux's glibc dynamic linker, wraps the launcher to unset `LD_PRELOAD` (L293-313, L322-332).
- Reports a one-shot install funnel telemetry event to `https://telemetry.jcode.sh/v1/event` (gated by `JCODE_NO_TELEMETRY=1` and `DO_NOT_TRACK=1`).

### "From source" cost

The README does **not** publish a build time. Adjacent docs and observations:

- The README's "Performance" section L710 admits: *"An incremental debug cargo build with cache enabled takes about 1 minute on my machine. The goal is 5-20 seconds."* A cold `--release` of ~88 crates is far longer than that; UNVERIFIED exact minutes (build not run in this audit).
- macOS requires either Xcode.app or the Command Line Tools **plus** an `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX<version>.sdk` env var (README L957-1000). This is a real friction point for users who only installed `xcode-select --install`.
- A `scripts/dev_cargo.sh` wrapper exists to use `sccache` + `clang+lld` for Linux x86_64 self-dev (README L1003-1015). Not on the default happy path.
- **Windows from source** is only available with `-BuildFromSource`, requires Git + Rust + **VS 2022 Build Tools with the "Desktop development with C++" workload** (README L925-927). Heavy.

### The `jcode self-dev --build` gotcha (specifically called out in the audit brief)

- `docs/PROJECT_OWNERSHIP_HANDOFF.md` L79: `jcode self-dev --build` *"rebuild AND repoint `~/.jcode/builds/shared-server/jcode`"*. The gotcha is exactly what the same doc warns about at L350: *"**`cargo build` ≠ running your build.** Use `jcode self-dev --build` + a private `--socket`, or you test stale code."* In other words: there are two jcode binaries — the launcher (`~/.local/bin/jcode`) and the selfdev-build root (`~/.jcode/builds/shared-server/jcode`) — and if a developer edits the source tree, running plain `cargo build` produces a binary that is **not** what `jcode` will launch. You must use the wrapper, or you are testing dead code.
- `docs/NATIVE_SSH.md` L168: `self-dev --build` is **rejected in SSH mode**. Developer on a remote host cannot use this workflow at all.
- `docs/SYSTEM_PROMPT_CONFIG.md` L43: editing `system_prompt.md` requires `selfdev build-reload`. So source-tree changes don't propagate without the wrapper.

### Verdict on install

**Distribution:** Real. Prebuilt binaries on GitHub Releases + a working curl installer + brew formula + PowerShell + Bun + Docker. A stranger can install in <60s on macOS/Linux/Windows with a one-liner.

**From-source:** Possible but punished — multi-minute cold release build, macOS SDK env-var dance, Windows needs VS Build Tools.

**Self-dev gotcha:** Real and undocumented in the README. Only mentioned in `PROJECT_OWNERSHIP_HANDOFF.md` and `NATIVE_SSH.md`. A new contributor will be confused.

---

## 2. FIRST RUN

### Onboarding code

- `crates/jcode-tui/src/tui/app/onboarding_flow.rs` (744 lines) defines the phase machine: `OnboardingPhase::{Login { import }, LoginOpenAi { yes_highlighted }, ModelSelect, ContinuePrompt { cli, yes_highlighted, shown_at }, StartChoice { shown_at }, Suggestions, Done}` (L382-425).
- Header comment at L1-19 describes the flow:
  1. `Login` — entered when **no working credentials exist**. **Skipped entirely when credentials already exist.**
  2. `StartChoice` — run a suggested Git-based bug-and-arch review or start a blank session.
  3. `Suggestions` — prompt-suggestion cards, resting state.
- L18-19 explicitly: *"Session history is intentionally excluded from onboarding and remains available later through `/resume`."*
- A 3-way telemetry consent screen exists at L52-112 (`TelemetryLevel::{Everything, NoContent, Nothing}`), persisted as two independent markers (`is_enabled` + `content_sharing_enabled`).
- `onboarding_flow_control.rs` is 1766 lines and `onboarding_graph.rs` is 920 lines — this is a real state machine, not a stub.

### What blocks the user from value

- `Login` phase opens when there are **no working credentials**. Detection scans 5 external CLIs (Codex, Claude Code, Pi, OpenCode, Cursor) per the `ImportReview` struct (L114-150) and the `ExternalCli` enum (L31-50). If nothing importable is found, the user lands on `LoginOpenAi` — a Yes/No prompt asking whether to log in to OpenAI (L400-403).
- The import dialog is **consent-gated**: the default summary screen lists detected external logins read-only with a preselected "Continue (import all)" pill, and a second "Choose what to import" pill lets users opt out per-login. Detected import paths are documented in `docs/AUTH_CREDENTIAL_SOURCES.md` (Codex, Claude, Gemini, Copilot, Cursor, plus OpenCode, pi, OpenClaw, Hermes from `auth/external.rs`).
- The README's detailed install (L849-877) and the `provider-doctor` CLI both document detection for Claude, OpenAI, Gemini, Copilot, Azure, OpenRouter, Fireworks, Novita, Minimax, NVIDIA NIM, Alibaba Coding Plan.

### Sandbox / demo mode that works with no credentials

**There is no demo / no-credential mode for the agent itself.** What exists:

- `scripts/onboarding_sandbox.sh` (per `docs/ONBOARDING_SANDBOX.md`) gives a developer-mode **isolated `JCODE_HOME`** for testing onboarding repeatedly. It is a developer convenience — `scripts/onboarding_sandbox.sh login openai`, `seed-real-logins`, etc. There is no equivalent for an end user.
- `jcode provider-doctor <provider> --tier offline` (`docs/PROVIDER_DOCTOR.md` L18-29, L41) runs `jcode`-side wiring checks with **no key and no spend**. This exercises the catalog reload, picker rendering, fallback labeling, and model-switch routing. It does **not** run the agent. It is purely a structural smoke test for an OpenAI-compatible provider.
- `provider-doctor --tier catalog` (default) requires a key but only fetches `GET /models` (L42). It is the lightest end-to-end test.
- The mobile onboarding simulator (`docs/ONBOARDING_SANDBOX.md` L134-149) is for **developers iterating on the onboarding UI**, not for end users.

**So: a brand-new user with no credentials can launch the TUI, walk through the `Login` / `LoginOpenAi` / telemetry-consent screens, and see the onboarding screens render, but they cannot get the agent to do work.** The agent's first user-visible action (a chat reply) requires a configured provider.

### First-run smoke test

- `jcode run "say hello"` is the README's recommended smoke test (L874). This requires a working provider.
- `jcode auth status --json` (`docs/AUTH_CREDENTIAL_SOURCES.md` L39-50) is the canonical way to ask *"is provider X authenticated?"*. The output reports `status`, `auth_kind`, `credential_source`, and `method` per provider.
- `jcode provider-test-coverage` (`docs/PROVIDER_DOCTOR.md` L139-165) shows last-tested freshness per (provider, model) pair.

---

## 3. AUTH — provider reality

### The dual-auth pattern for the two big providers

From `docs/AUTH_CREDENTIAL_SOURCES.md` L13-21:

| Provider | OAuth login id | API-key login id | Credential source (OAuth) | Credential source (API key) |
|---|---|---|---|---|
| Anthropic / Claude | `claude` (OAuth/sub) | `anthropic-api` (API key) | `~/.jcode/auth.json` → `anthropic_accounts[].access` (`sk-ant-oat…`) | `ANTHROPIC_API_KEY` env **or** `~/.config/jcode/anthropic.env` |
| OpenAI / ChatGPT / Codex | `openai` (OAuth) | `openai-api` (API key) | `~/.jcode/openai-auth.json` | `OPENAI_API_KEY` env **or** `~/.config/jcode/openai.env` |

Crucial: `claude` ≠ `anthropic-api` and `openai` ≠ `openai-api`. Grepping for `sk-ant-api03` will miss an OAuth-only setup. The doc is explicit (L23-35).

### Provider surface (per README L660-664 + `AUTH_CREDENTIAL_SOURCES.md`)

| Login id | Auth kind | Implementation in repo | Status |
|---|---|---|---|
| `claude` | OAuth subscription | `crates/jcode-provider-anthropic-runtime/src/lib.rs` (2662 lines), `crates/jcode-provider-anthropic/src/lib.rs` (1189 lines) | **REAL**, heavy-weight runtime with SSE streaming, OAuth refresh, anthropic fallback model, tool schema repair |
| `anthropic-api` | API key | same crates | **REAL** (no special API-key crate; same Anthropic Messages runtime) |
| `openai` | OAuth (ChatGPT/Codex) | `crates/jcode-provider-openai-runtime/src/lib.rs` (1447 lines) + `openai/` + `chatgpt_web.rs` + `openai_provider_impl.rs` + `openai_stream_runtime.rs` + `openai_usage_recording.rs` | **REAL**, Responses WebSocket v2 (README L384-386, `docs/OPENAI_WEBSOCKET.md`) with HTTPS fallback |
| `openai-api` | API key | same | **REAL** |
| `gemini` | OAuth + API-key | `crates/jcode-provider-gemini-runtime/src/lib.rs` (1139 lines) + `crates/jcode-provider-gemini/src/lib.rs` (804 lines) | **REAL** |
| `copilot` | OAuth (device flow + standard) | `crates/jcode-provider-copilot-runtime/src/lib.rs` (1195 lines) + `crates/jcode-provider-copilot/src/lib.rs` (313 lines) | **REAL**, README L641 documents `--print-auth-url` / `--complete` flow |
| `azure` | OAuth / API | `crates/jcode-azure-auth/` | **REAL** (referenced from README L659 + `docs/AUTH_CREDENTIAL_SOURCES.md`) |
| `alibaba-coding-plan` | OAuth / API | (referenced README L373) | UNVERIFIED crate layout, but listed in the README's login surface |
| `openrouter` | API key (named profile) | `crates/jcode-provider-openrouter-runtime/src/lib.rs` (2842 lines) + `openai_compat` integration | **REAL**, with live catalog merge |
| `orcarouter` | API key | listed in README L404 | UNVERIFIED, mentioned as a built-in profile but no dedicated crate line — likely an `openai-compatible` named profile (READMEL405) |
| `opencode` | OpenCode Zen | (README L405-406) | UNVERIFIED — listed as a named profile but I'd need to check `jcode-provider-metadata` to confirm the route. README L662 also lists `opencode-go` |
| `opencode-go` | External (opencode-go keys) | (README L662) | UNVERIFIED — README lumps into "Additional provider integrations" |
| `zai` / `kimi` | API | (README L662) | UNVERIFIED |
| `302ai`, `baseten`, `cortecs`, `deepseek`, `firmware`, `huggingface`, `moonshotai`, `nebius`, `scaleway`, `stackit`, `groq`, `mistral`, `perplexity`, `togetherai`, `deepinfra`, `fireworks`, `novita`, `minimax`, `xai`, `lmstudio`, `ollama`, `chutes`, `cerebras`, `antigravity`, `google`, `cursor` | mostly API key named profiles via `openai-compatible` | README L662 lists all of them | **REAL via the OpenAI-compat harness** — `docs/PROVIDER_DOCTOR.md` L11-13 explicitly says provider-doctor works with `openai-compatible` providers (cerebras, fpt, nvidia-nim, comtegra, deepseek, groq, openrouter, …). The OpenAI-compat runtime is the same `openrouter-runtime`/openai-runtime stack plus named profiles in `jcode-provider-metadata/` |
| `bedrock` | AWS sigv4 | `crates/jcode-provider-bedrock/src/lib.rs` (1981 lines) + `docs/AWS_BEDROCK_PROVIDER.md` | **REAL** for AWS users |
| `claude-cli-runtime` | reuses Claude Code's binary | `crates/jcode-provider-claude-cli-runtime/src/lib.rs` (1142 lines) | **REAL**, a shim that drives the Claude Code CLI as a backend |
| `cursor-runtime` | Cursor's local daemon | `crates/jcode-provider-cursor-runtime/src/lib.rs` (430 lines) | **REAL** |
| `antigravity-runtime` | Antigravity CLI | `crates/jcode-provider-antigravity-runtime/src/lib.rs` (915 lines) + `jcode-provider-antigravity/src/lib.rs` (580 lines) | **REAL**, provider-doctor has a dedicated `antigravity` tier per `live_provider_probes.rs:1226` |
| `grok-build-runtime` | Grok CLI binary | `crates/jcode-provider-grok-build-runtime/src/lib.rs` (834 lines) | **REAL** (binary-shim style) |
| `forgecode-runtime` | Forge binary | `crates/jcode-provider-forgecode-runtime/src/lib.rs` (610 lines) | **REAL** |

### What's *not* built into a dedicated crate

- `zai` / `kimi`, `moonshotai`, `xai`, `302ai`, `baseten`, `cortecs`, `firmware`, `huggingface`, `nebius`, `scaleway`, `stackit`, `groq`, `mistral`, `perplexity`, `togetherai`, `deepinfra`, `fireworks`, `novita`, `minimax`, `lmstudio`, `ollama`, `chutes`, `cerebras`, `chutes` — none of these have a dedicated `jcode-provider-<name>` crate. They are **named profiles** wired through the `openai-compatible` runtime, with their endpoint / headers / `extra_body` table in `jcode-provider-metadata`. The README calls them "built-in named profiles" (L398-406).
- `provider-doctor` exercises them via `live_provider_probes.rs` (L8-13). The README (L398-406) makes the same claim: `jcode login --provider openrouter` fills in the base URL + key env var. **There is no provider-doctor test for any provider that does not match the `openai-compatible` (or Anthropic-Messages-compatible) wire protocol.**

### Subscription-OAuth vs API-key reality

The README's claim that OAuth logins work for ChatGPT Pro / Claude Max / Copilot is corroborated by the runtime crate sizes (`anthropic-runtime` 2662 lines, `openai-runtime` 1447 lines, `copilot-runtime` 1195 lines, `gemini-runtime` 1139 lines) and by `docs/CHATGPT_API_EQUIVALENT_USAGE.md` (L1-25), which documents local-only tracking of OAuth-token usage and a curated OpenAI price table to estimate API-equivalent cost (NOT a charge — explicitly stated at L3).

Rate-limit / quota caveats are mentioned indirectly:

- `jcode-provider-openai-runtime/src/openai_rate_limit_format.rs` exists (per `ls`), suggesting OpenAI-specific rate-limit response handling.
- The fallback model is provider-specific: `jcode-provider-anthropic-runtime/src/lib.rs` has `anthropic_fallback_model` (L2344) which attempts to downgrade to a cheaper model on rate-limit. Same idea for `copilot/src/lib.rs` L17 (`FALLBACK_MODELS`) and `antigravity/src/lib.rs:10` (`DEFAULT_FALLBACK_MODEL = "gemini-3-flash"`).

### Caveats / half-wired items

- The README's "Additional provider integrations" list (L662) groups many real and half-real entries together. From the **code alone**, the only way to know whether a given provider works today is to run `provider-doctor <name> --tier offline`. UNVERIFIED per-provider online status without running.
- `docs/AUTH_CREDENTIAL_SOURCES.md` L127-128 mentions tool-specific importers for Claude Code, Codex, Gemini CLI, GitHub Copilot, Cursor (under `auth/{claude,codex,gemini,copilot,cursor}.rs`). For a user who already has another coding agent installed, **the import flow is the fastest path to value** — but it requires consent (L99-107) and is single-shot (not continuously synced for Codex per README L597-600).

---

## 4. CORE LOOP — does the agent do work?

### Tool registry (always-on built-ins)

From `crates/jcode-app-core/src/tool/mod.rs` `base_tools()` (L320-428):

| Tool | File | Real? |
|---|---|---|
| `read` | `tool/read.rs` | real |
| `write` | `tool/write.rs` | real |
| `agentgrep` | `tool/agentgrep.rs` | real (README L686 — custom harness-aware grep with adaptive truncation) |
| `side_panel` | `tool/side_panel.rs` | real (README L327-330) |
| `edit` | `tool/edit.rs` | real |
| `multiedit` | `tool/multiedit.rs` | real |
| `patch` | `tool/patch.rs` | real |
| `apply_patch` | `tool/apply_patch.rs` | real |
| `ls` | `tool/ls.rs` | real |
| `bash` | `tool/bash.rs` (+ `bash_destructive_gate.rs`) | real, with a destructive-command gate per `tool/bash_destructive_gate.rs` |
| `browser` | `tool/browser.rs` (+ `browser_tests.rs`) | real, but **requires `jcode browser setup` first** — Firefox Agent Bridge per README L757-792 |
| `open` | `tool/open.rs` | real |
| `macos_computer_use` (macOS only) | `tool/computer/mod.rs` | real, gated to `#[cfg(target_os = "macos")]` |
| `webfetch` | `tool/webfetch.rs` | real (with `webfetch_corpus_tests.rs`) |
| `websearch` | `tool/websearch.rs` | real |
| `invalid` | `tool/invalid.rs` | placeholder (intentional) |
| `maintainer_feedback` | `tool/feedback.rs` | real, telemetry-side |
| `elicitate_mcp` | `tool/elicitation.rs` | real (interop with the `phinbox_mcp` MCP server) |
| `jcode_docs` | `tool/jcode_docs.rs` | real (on-demand docs search) |
| `todo` | `tool/todo.rs` (+ `goal_tests.rs`) | real |
| `initiative` | `tool/goal.rs` (+ `goal_tests.rs`) | **DISABLED**: comment at L412-413 says *"Initiative is temporarily unavailable. Keep its implementation and saved data intact so it can be restored without a migration."* The tool is registered but flagged as unavailable |
| `bg` | `tool/bg.rs` | real (background-task orchestration) |
| `session_search` | `tool/session_search.rs` (+ index) | real |
| `memory` | `tool/memory.rs` | real (README L301-321, semantic-vector memory) |
| `gmail` | `tool/gmail.rs` | real but **requires Composio backend** per `docs/GMAIL_COMPOSIO_BACKEND.md` |
| `schedule` | `tool/ambient.rs` | real (ambient mode scheduling) |
| `selfdev` | `tool/selfdev/mod.rs` (+ `setup.rs`, `build_queue.rs`) | real for self-dev sessions; for normal sessions it is an **on-ramp** with only `enter` / `setup` / `reload` / `find-config` / `status` actions (L486-502) |
| `skill_manage` | `tool/skill.rs` | real, added per session (L432-436) |

### Session-gated tools

- `register_mcp_tools` (L1061-1106) registers `mcp`, `mcp_search`, `mcp_call`. MCP server tools (`mcp__<server>__<tool>`) get added per server when configured. **stdio-only** — README L602 says *"jcode currently supports stdio (command-based) servers only; HTTP/SSE entries are recognized and skipped with a log line."*
- `register_selfdev_tools` (L1320-1337) registers `selfdev` and `debug_socket` only inside canary / self-dev sessions.
- `register_ambient_tools` (L1339-1364) registers `end_ambient_cycle`, `schedule_ambient`, `request_permission`, `send_message` only for ambient sessions.

### Reality check per capability

| Capability | Real? | Notes |
|---|---|---|
| Read / write / edit / patch / apply_patch | YES | all in `tool/{read,write,edit,patch,apply_patch}.rs` |
| Bash with destructive gate | YES | `tool/bash_destructive_gate.rs` |
| `agentgrep` | YES | custom (README L686) |
| Side panel + mermaid render | YES | README L327-336 |
| Web fetch + web search | YES | |
| Memory (semantic-vector + sideagent + graph) | YES | README L301-321; `crates/jcode-session-memory/` |
| Session search (RAG over past sessions) | YES | `tool/session_search.rs` |
| MCP stdio servers | YES | documented limit: stdio only |
| MCP HTTP/SSE servers | **NO** | README L602 explicitly says skipped |
| Browser automation | YES, but setup required | Firefox Agent Bridge, `jcode browser setup` (README L757-792) |
| macOS computer use | YES, macOS only | gated `#[cfg]` |
| Gmail | YES, but requires Composio | `docs/GMAIL_COMPOSIO_BACKEND.md` |
| Swarm (multi-agent) | YES | `crates/jcode-swarm-core/`; README L344-358 |
| Resume sessions across harnesses (Codex, Claude Code, OpenCode, pi) | YES | README L690-694 |
| Ambient mode (auto-consolidate memory, schedule tasks) | YES | `docs/AMBIENT_MODE.md` + `tool/ambient.rs` |
| Voice input (dictate) | YES (configurable STT) | README L737-741 |
| Server / client mode (persistent `jcode serve` + `jcode connect`) | YES | README L733-737 |
| Init flow without a working provider | **NO** | onboarding blocks at `Login` / `LoginOpenAi` (L391-403) |
| Skill semantic-vector injection | YES | README L696 |
| `initiative` tool (long-running autonomous loop) | **DISABLED** | L412-413 |
| iOS native app | **PLANNED** | README L700-704 says "coming soon" |

### Tool that claims but does not have

- `initiative` is registered (L401-403) but the comment at L412-413 says it is *"temporarily unavailable"*. A new user browsing the tool list will see it; the system prompt should clarify it's disabled. UNVERIFIED whether the system prompt / tool description tells the model this.
- `browser` registers before setup has run. Per the README L757-792, the user must run `jcode browser setup` (which installs Firefox Agent Bridge) before the browser tool actually works. A first-run user who says *"open https://example.com"* without running setup will get a setup-needed error, not a working browser.
- `gmail` registers but requires Composio configuration (`docs/GMAIL_COMPOSIO_BACKEND.md`). A new user who sees it listed will be confused if no setup wizard appears.

---

## 5. CVP / MVP — minimum valuable thing a new user can do TODAY

### The honest CVP

The **minimum valuable end-to-end thing** for a brand-new user is:

1. Install via `curl -fsSL https://jcode.sh/install | bash`.
2. Have an **existing subscription or API key** for one of: Claude (Max / Pro via OAuth or Anthropic API key), ChatGPT Plus/Pro (OAuth or OpenAI API key), Gemini, GitHub Copilot, Azure, or any of the ~20 named profiles (OpenRouter, DeepSeek, Groq, Mistral, etc. — all API-key).
3. Either let the importer pull an external login (Codex, Claude Code, etc.) **or** run `jcode login --provider <x>` to OAuth-in.
4. Run `jcode` to open the TUI. The onboarding flow walks Login → Telemetry consent → StartChoice → Suggestions.
5. Type a prompt like *"fix the bug in src/foo.rs"* or *"create a hello.txt"*. The agent uses `read`, `bash`, `edit` to do the work.

### What blocks a brand-new user with nothing configured

**The agent cannot be exercised without at least one configured provider, and there is no demo/sandbox mode that does work.** The README's "Quick Install" assumes the user has either a subscription or is willing to paste an API key; there is no "try it once" path that does not require credentials. `provider-doctor --tier offline` (L18-29, L41) is the closest thing, but it only validates the wiring, not the agent.

If a user does **not** have any subscription or API key, the experience is:

- Launch `jcode` → onboarding opens at `Login` phase.
- If they have no other coding agent installed (Codex / Claude Code / etc.), the import walkthrough shows nothing to import.
- They land on the `LoginOpenAi` yes/no prompt. If they say "No", onboarding exits to the new-session screen with a system message saying *"run `/login` when ready"* (onboarding_flow.rs L395-397).
- They have a TUI that renders but cannot chat.

### The CVP for a user who already has another coding agent

The CVP becomes **<60 seconds**: install jcode, let the importer pick up Claude Code's OAuth token, and start typing. This is genuinely a feature — the README L692-694 advertises cross-harness session resume. But it depends on the user already being in the Anthropic / OpenAI ecosystem.

---

## 6. TOP 5 ADOPTION GAPS (ranked by how much they block a new user)

### #1 — No "try it" / demo / no-credential mode

**Blocks**: everyone who doesn't already have a Claude / ChatGPT / Copilot subscription or API key.

The onboarding flow ends at `LoginOpenAi` yes/no if nothing is importable. There is no "tour mode" that lets a curious user click around the TUI, send a message, and see the agent do work. Closest is `provider-doctor --tier offline`, which validates wiring but doesn't exercise the agent loop. A "demo" provider that returns canned responses would unblock casual evaluators and conference demos.

### #2 — Installer claims 4 install paths; only one works without per-platform friction

**Blocks**: users on less-common platforms (Termux, Windows ARM64, NixOS).

- The README lists curl/PowerShell/Brew/manual binary/Bun/Docker/from-source.
- The install script auto-falls-back to `cargo build --release` if no prebuilt asset is found (install.sh L275-289). For a user without Rust installed, this fails with `"cargo is required to build from source"` (L278). The error is correct but the diagnosis is poor.
- Windows ARM64: real binary exists (install.sh L134). PowerShell installer supports it.
- Linux x86_64/aarch64 + macOS arm64/x86_64: all real binaries.
- Termux: requires `pkg install glibc patchelf` first.
- From source on Windows needs VS Build Tools + Git + Rust — heavy.

The gap is not "missing binary" but "no preflight check that tells the user exactly what to install before running the installer".

### #3 — Telemetry is on by default, and the privacy story is split across 4 endpoints + 2 env vars + 1 marker file

**Blocks**: privacy-conscious users and anyone in a regulated environment.

- `crates/jcode-telemetry-core/src/lib.rs` L34-35: two distinct endpoints (`https://telemetry.jcode.sh/v1/event` for usage, `https://telemetry.jcode.sh/v1/transcript` for content sharing).
- L499: opt-out checks `JCODE_NO_TELEMETRY` env OR `DO_NOT_TRACK` env.
- L480-485: a marker file at `~/.jcode/no_usage_telemetry` persists the opt-out.
- The 3-way `TelemetryLevel` (onboarding_flow.rs L52-112) gives Everything / NoContent / Nothing as a single consent screen during onboarding — that part is good.
- L2468: a stderr message *"To opt out: export JCODE_NO_TELEMETRY=1"*.
- **But the install script (install.sh L44-63) sends an `install_funnel` event with OS/arch/version/stage/outcome to `https://telemetry.jcode.sh/v1/event` *during the install itself*, before any onboarding consent screen.** That event is gated by `JCODE_NO_TELEMETRY=1` and `DO_NOT_TRACK=1` envs (L48-50), but a privacy-conscious user running `curl … | bash` does not know to set those. There is no `--no-telemetry` flag on the install script.

A user reading the README would have to know to grep `TELEMETRY.md` (which is not linked from the README — verified by `rg "TELEMETRY" README.md`, returned 0 hits) to understand what is sent.

### #4 — The "what is configured" surface is fragmented across 4 files, 2 env vars, 1 JSON marker, and a 7-day-stale validation cache

**Blocks**: anyone debugging "why doesn't my provider work?".

`docs/AUTH_CREDENTIAL_SOURCES.md` L1-9 explicitly says this is the most common confusion:

> *"the same confusion keeps recurring: an agent (or a human) greps for `ANTHROPIC_API_KEY` / `sk-ant-api`, finds nothing, reads a stale `auth-validation.json` entry that says 'expired', and wrongly concludes 'there is no working credential'. In reality the credential is present and working; it just lives somewhere the naive search did not look."*

The state lives in: `~/.jcode/auth.json` (OAuth), `~/.jcode/openai-auth.json` (OpenAI OAuth), `~/.config/jcode/<provider>.env` (API keys), `ANTHROPIC_API_KEY` env, `OPENAI_API_KEY` env, external `$HOME/.local/share/opencode/auth.json`, `~/.pi/agent/auth.json`, `~/.openclaw/agent/auth.json`, `~/.hermes/auth.json` — all simultaneously.

The `jcode auth status --json` command is the canonical answer (L39-50) but it is **not** the first thing a new user discovers. The README's "OAuth and Providers" section (L362-682) is 320 lines and does not mention `auth status --json` until deep inside.

### #5 — The `jcode self-dev --build` gotcha is documented but not discoverable

**Blocks**: anyone who tries to contribute / modify / self-host.

`docs/PROJECT_OWNERSHIP_HANDOFF.md` L350: *"**`cargo build` ≠ running your build.** Use `jcode self-dev --build` + a private `--socket`, or you test stale code."* — but this is in a `PROJECT_OWNERSHIP_HANDOFF.md` doc that the README does not link. A first-time contributor running `cargo build --release && target/release/jcode` will silently test a dead binary.

`docs/NATIVE_SSH.md` L168 adds that `self-dev --build` is **rejected in SSH mode** — a developer who SSHes into their build host cannot use this workflow at all.

---

## 7. UX claim vs code reality — discrepancies

| Claim in docs | What the code actually does | Verdict |
|---|---|---|
| README L362-664 lists 40+ providers as supported | The dedicated crate coverage is: anthropic, openai, gemini, copilot, openrouter, bedrock, claude-cli, cursor, antigravity, grok-build, forgecode (11 crates). The other 30 are **named profiles** that reuse the `openai-compatible` runtime. Provider-doctor confirms the harness works against them, but only one wire protocol is supported. | **PARTLY HONORED**. A user picking one of the named profiles gets a working OpenAI-compatible integration, but the README's flat list implies parity that does not exist. |
| README L17 — "The most RAM efficient harness / The most intelligent harness" | The RAM table (L96-218) is real and uses PSS measurements from a specific Linux machine. The "most intelligent" claim is marketing — no benchmark methodology is cited in the README. | **RAM claim honored; "most intelligent" is unverified.** |
| README L301-321 — Memory system with embedding + sideagent + ambient consolidation | `crates/jcode-session-memory/`, `tool/memory.rs`, `tool/ambient.rs`, `docs/MEMORY_ARCHITECTURE.md` all exist. | **HONORED.** |
| README L686 — "Agent grep is a grep tool I made for the jcode agent. It adds file structure information (ie the list of functions, their displacement, etc) to the grep return" | `tool/agentgrep.rs` exists. | **HONORED.** |
| README L700 — iOS app coming soon | No iOS code exists in this checkout (verified `ls ios/` only shows an `ios` directory). | **PLANNED, not delivered.** |
| README L602 — "jcode currently supports stdio (command-based) servers only; HTTP/SSE entries are recognized and skipped with a log line" | Matches `crates/jcode-base/src/mcp/` and tool/mod.rs `register_mcp_tools`. | **HONORED.** |
| `ONBOARDING_STATE_GRAPH.md` L3 — *"Status: partially implemented (steps 1, 2, 5, 6 landed; see §5)"* | The doc is honest about its own incompleteness; the actual phases in `onboarding_flow.rs` are wired but the state-graph formal model is not finished. | **HONORED as a roadmap; not a UX guarantee.** |
| README L824-877 — "Set up jcode on this machine for me" prompt for other agents | This is a copy-paste prompt template, not an automated installer. Useful, but the doc presents it as the canonical first-run path. | **HONEST.** |
| README L344-358 — Swarm spawns multiple agents in the same repo with conflict-aware messaging | `crates/jcode-swarm-core/` exists; `docs/SWARM_ARCHITECTURE.md` is in the docs folder. | **HONORED.** |
| `docs/AUTH_CREDENTIAL_SOURCES.md` L74-88 — "`~/.jcode/auth-validation.json` is a historical record; entries older than 7 days are stale; re-validate with `jcode auth-test`" | `crates/jcode-base/src/auth/validation.rs` `format_record_label` with `VALIDATION_STALE_AFTER_MS` constant. | **HONORED.** |
| README L708-710 — *"Build speed improvements: An incremental debug cargo build with cache enabled takes about 1 minute on my machine. The goal is 5-20 seconds."* | No evidence the goal is met. `scripts/dev_cargo.sh` exists with `sccache + clang + lld`, but the README does not promise a time. | **HONEST (it's an aspiration).** |
| README L757-792 — Browser tool uses Firefox Agent Bridge | `tool/browser.rs` exists, `jcode browser status` / `jcode browser setup` commands exist. | **HONORED — but requires explicit setup.** A user who skips setup will see the tool registered but get a setup-needed error. The README could be louder about this. |
| README L690-694 — "Resume sessions from different harnesses" (Codex, Claude Code, OpenCode, pi) | `crates/jcode-base/src/import.rs` + `import_tests.rs` references external `.claude/`, `.codex/`, etc. The codebase has 600+ lines of import tests (per `import_tests.rs`). | **HONORED.** |

---

## 8. UNVERIFIED items (could not confirm without running)

- Exact per-provider online behavior for the 30 named profiles (cerebras, fireworks, novita, minimax, zai/kimi, moonshotai, xai, chutes, deepinfra, …) — they share the `openai-compatible` runtime, but the README does not show a per-provider "this works today" stamp. `jcode provider-doctor <name> --tier offline` is the cheapest check.
- The exact wall-clock of a cold `cargo build --release` of all 88 crates. The README admits "about 1 minute" for *incremental debug* (L710); a clean release is plausibly 5-15 minutes.
- Whether `provider-doctor claude` / `provider-doctor openai` (the native Anthropic / native OpenAI tiers) pass on a fresh machine — there is a dedicated probe for both (per `live_provider_probes.rs:667` for Claude, `live_provider_probes.rs:1226` for Antigravity), but UNVERIFIED without running.
- Whether the TUI's `Login` phase actually opens a browser or just prints an auth URL when run headless. The README L625-636 mentions `--no-browser` for headless SSH use, so the headless path exists; UNVERIFIED for the default TUI launch.
- Whether `initiative` is silently invisible in the system prompt or just registered and called.
- Whether the install script's `install_funnel` telemetry POST happens before or after the first-run onboarding telemetry consent screen (which would surprise a privacy-conscious user either way).
- The README's quickstart (L722-738) does not list `jcode auth status --json` as a recommended first-run smoke test, despite `AUTH_CREDENTIAL_SOURCES.md` saying it is the canonical way to verify auth. The README's "Detailed Installation" only mentions `jcode auth-test --all-configured` (L861) without showing the JSON shape.

---

## TL;DR for the operator

A brand-new user who already has a Claude / ChatGPT / Copilot subscription or any major-provider API key can install jcode in <60s and be productive in the next minute. A user with no credentials gets a beautiful onboarding flow that stops at a login prompt and cannot proceed. The README's 40-provider list conflates "wired through openai-compatible" with "native first-class integration"; the underlying reality is 11 dedicated provider crates and a strong `openai-compatible` harness. The biggest adoption gaps are: no demo mode, fragmented auth-state locations, telemetry-on-by-default during install, and the `cargo build` ≠ `jcode self-dev --build` gotcha for contributors.