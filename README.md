<div align="center">

# jcode

[![CI](https://github.com/1jehuang/jcode/actions/workflows/ci.yml/badge.svg)](https://github.com/1jehuang/jcode/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

A blazing-fast, fully autonomous AI coding agent with a gorgeous TUI,
multi-model support, swarm coordination, persistent memory, and 30+ built-in tools -
all running natively in your terminal.

<br>

<img src="https://github.com/1jehuang/jcode/releases/download/v0.3.1/jcode_demo_jaguar.avif" alt="jcode demo" width="800">

<br>

[Features](#features) · [Install](#installation) · [Usage](#usage) · [Architecture](#architecture) · [Tools](#tools)

</div>

---

## Features

<div align="center">

| Feature | Description |
|---|---|
| **Blazing Fast TUI** | Sub-millisecond rendering at 1,400+ FPS. No flicker. No lag. Ever. |
| **Multi-Provider** | Claude, OpenAI, OpenRouter - 200+ models, switch on the fly |
| **No API Keys Needed** | Works with your Claude Max or ChatGPT Pro subscription via OAuth |
| **Persistent Memory** | Learns about you and your codebase across sessions |
| **Swarm Mode** | Multiple agents coordinate in the same repo with conflict detection |
| **30+ Built-in Tools** | File ops, search, web, shell, memory, sub-agents, parallel execution |
| **MCP Support** | Extend with any Model Context Protocol server |
| **Server / Client** | Daemon mode with multi-client attach, session persistence |
| **Sub-Agents** | Delegate tasks to specialized child agents |
| **Self-Updating** | Built-in self-dev mode with hot-reload and canary deploys |
| **Featherweight** | ~28 MB idle client, single native binary - no runtime, no VM, no Electron |
| **OpenClaw** | Always-on ambient agent — gardens memory, does proactive work, responds via Telegram |

</div>

---

<div align="center">

## Performance & Resource Efficiency

*A single native binary. No Node.js. No Electron. No Python. Just Rust.*

</div>

jcode is engineered to be absurdly efficient. While other coding agents spin up
Electron windows, Node.js runtimes, and multi-hundred-MB processes, jcode runs
as a single compiled binary that sips resources.

<div align="center">

| Metric | jcode | Typical AI IDE / Agent |
|---|---|---|
| **Idle client memory** | **~28 MB** | 300–800 MB |
| **Server memory** | **~40 MB** (base) | N/A (monolithic) |
| **Active session** | **~50–65 MB** | 500 MB+ |
| **Frame render time** | **0.67 ms** (1,400+ FPS) | 16 ms (60 FPS, if lucky) |
| **Startup time** | **Instant** | 3–10 seconds |
| **CPU at idle** | **~0.3%** | 2–5% |
| **Runtime dependencies** | **None** | Node.js, Python, Electron, … |
| **Binary** | **Single 66 MB executable** | Hundreds of MB + package managers |

</div>

> **Real-world proof:** Right now on the dev machine there are **10+ jcode sessions**
> running simultaneously - clients, servers, sub-agents - all totaling less memory
> than a single Electron app window.

The secret is Rust. No garbage collector pausing your UI. No JS event loop
bottleneck. No interpreted overhead. Just zero-cost abstractions compiled
to native code with `jemalloc` for memory-efficient long-running sessions.

---

<div align="center">

## Installation

### Quick Install (macOS & Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.sh | bash
```

### macOS via Homebrew

```bash
brew tap 1jehuang/jcode
brew install jcode
```

### From Source (all platforms)

```bash
git clone https://github.com/1jehuang/jcode.git
cd jcode
cargo build --release
```

Then symlink to your PATH:

```bash
# Linux
ln -sf $(pwd)/target/release/jcode ~/.local/bin/jcode

# macOS
ln -sf $(pwd)/target/release/jcode /usr/local/bin/jcode
```

### Prerequisites

You need at least one of:

| Provider | Setup |
|---|---|
| **Claude** (recommended) | Install [Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code), run `claude login` |
| **OpenAI / Codex** | Run `codex login` to authenticate |
| **OpenRouter** | Set `OPENROUTER_API_KEY=sk-or-v1-...` |
| **Direct API Key** | Set `ANTHROPIC_API_KEY=sk-ant-...` |

### Platform Support

| Platform | Status |
|---|---|
| **Linux** x86_64 / aarch64 | Fully supported |
| **macOS** Apple Silicon & Intel | Supported |
| **Windows** (WSL2) | Experimental |

</div>

---

<div align="center">

## Usage

</div>

```bash
# Launch the TUI (default - connects to server or starts one)
jcode

# Run a single command non-interactively
jcode run "Create a hello world program in Python"

# Start as background server
jcode serve

# Connect additional clients to the running server
jcode connect

# Specify provider
jcode --provider claude
jcode --provider openai
jcode --provider openrouter

# Change working directory
jcode -C /path/to/project

# Resume a previous session by memorable name
jcode --resume fox
```

---

<div align="center">

## Tools

30+ tools available out of the box - and extensible via MCP.

| Category | Tools | Description |
|---|---|---|
| **File Ops** | `read` `write` `edit` `multiedit` `patch` `apply_patch` | Read, write, and surgically edit files |
| **Search** | `glob` `grep` `ls` `codesearch` | Find files, search contents, navigate code |
| **Execution** | `bash` `task` `batch` `bg` | Shell commands, sub-agents, parallel & background execution |
| **Web** | `webfetch` `websearch` | Fetch URLs, search the web via DuckDuckGo |
| **Memory** | `memory` `remember` `session_search` `conversation_search` | Persistent cross-session memory and RAG retrieval |
| **Coordination** | `communicate` `todo_read` `todo_write` | Inter-agent messaging, task tracking |
| **Meta** | `mcp` `skill` `selfdev` | MCP servers, skill loading, self-development |

</div>

---

<div align="center">

## Architecture

</div>

<details>
<summary><strong>High-Level Overview</strong></summary>

<br>

```mermaid
graph TB
    CLI["CLI (main.rs)<br><i>jcode [serve|connect|run|...]</i>"]

    CLI --> TUI["TUI<br>app.rs / ui.rs"]
    CLI --> Server["Server<br>Unix Socket"]
    CLI --> Standalone["Standalone<br>Agent Loop"]

    Server --> Agent["Agent<br>agent.rs"]
    TUI <-->|events| Server

    Agent --> Provider["Provider<br>Claude / OpenAI / OpenRouter"]
    Agent --> Registry["Tool Registry<br>30+ tools"]
    Agent --> Session["Session<br>Persistence"]

    style CLI fill:#f97316,color:#fff
    style Agent fill:#8b5cf6,color:#fff
    style Provider fill:#3b82f6,color:#fff
    style Registry fill:#10b981,color:#fff
    style TUI fill:#ec4899,color:#fff
    style Server fill:#6366f1,color:#fff
```

**Data Flow:**
1. User input enters via TUI or CLI
2. Server routes requests to the appropriate Agent session
3. Agent sends messages to Provider, receives streaming response
4. Tool calls are executed via the Registry
5. Session state is persisted to `~/.jcode/sessions/`

</details>

<details>
<summary><strong>Provider System</strong></summary>

<br>

```mermaid
graph TB
    MP["MultiProvider<br><i>Detects credentials, allows runtime switching</i>"]

    MP --> Claude["ClaudeProvider<br>provider/claude.rs"]
    MP --> OpenAI["OpenAIProvider<br>provider/openai.rs"]
    MP --> OR["OpenRouterProvider<br>provider/openrouter.rs"]

    Claude --> ClaudeCreds["~/.claude/.credentials.json<br><i>OAuth (Claude Max)</i>"]
    Claude --> APIKey["ANTHROPIC_API_KEY<br><i>Direct API</i>"]
    OpenAI --> CodexCreds["~/.codex/auth.json<br><i>OAuth (ChatGPT Pro)</i>"]
    OR --> ORKey["OPENROUTER_API_KEY<br><i>200+ models</i>"]

    style MP fill:#8b5cf6,color:#fff
    style Claude fill:#d97706,color:#fff
    style OpenAI fill:#10b981,color:#fff
    style OR fill:#3b82f6,color:#fff
```

**Key Design:**
- `MultiProvider` detects available credentials at startup
- Seamless runtime switching between providers with `/model` command
- Claude direct API with OAuth - no API key needed with a subscription
- OpenRouter gives access to 200+ models from all major providers

</details>

<details>
<summary><strong>Tool System</strong></summary>

<br>

```mermaid
graph TB
    Registry["Tool Registry<br><i>Arc&lt;RwLock&lt;HashMap&lt;String, Arc&lt;dyn Tool&gt;&gt;&gt;&gt;</i>"]

    Registry --> FileTools["File Tools<br>read · write · edit<br>multiedit · patch"]
    Registry --> SearchTools["Search & Nav<br>glob · grep · ls<br>codesearch"]
    Registry --> ExecTools["Execution<br>bash · task · batch · bg"]
    Registry --> WebTools["Web<br>webfetch · websearch"]
    Registry --> MemTools["Memory & RAG<br>remember · session_search<br>conversation_search"]
    Registry --> MetaTools["Meta & Control<br>todo · skill · communicate<br>mcp · selfdev"]
    Registry --> MCPTools["MCP Tools<br><i>Dynamically registered<br>from external servers</i>"]

    style Registry fill:#10b981,color:#fff
    style FileTools fill:#3b82f6,color:#fff
    style SearchTools fill:#6366f1,color:#fff
    style ExecTools fill:#f97316,color:#fff
    style WebTools fill:#ec4899,color:#fff
    style MemTools fill:#8b5cf6,color:#fff
    style MetaTools fill:#d97706,color:#fff
    style MCPTools fill:#64748b,color:#fff
```

**Tool Trait:**
```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput>;
}
```

</details>

<details>
<summary><strong>Server & Swarm Coordination</strong></summary>

<br>

```mermaid
graph TB
    Server["Server<br>/run/user/{uid}/jcode.sock"]

    Server --> C1["Client 1<br>TUI"]
    Server --> C2["Client 2<br>TUI"]
    Server --> C3["Client 3<br>External"]
    Server --> Debug["Debug Socket<br>Headless testing"]

    subgraph Swarm["Swarm - Same Working Directory"]
        Fox["fox<br>(agent)"]
        Oak["oak<br>(agent)"]
        River["river<br>(agent)"]

        Fox <--> Coord["Conflict Detection<br>File Touch Events<br>Shared Context"]
        Oak <--> Coord
        River <--> Coord
    end

    Server --> Swarm

    style Server fill:#6366f1,color:#fff
    style Debug fill:#64748b,color:#fff
    style Coord fill:#ef4444,color:#fff
    style Fox fill:#f97316,color:#fff
    style Oak fill:#10b981,color:#fff
    style River fill:#3b82f6,color:#fff
```

**Protocol (newline-delimited JSON over Unix socket):**
- **Requests:** Message, Cancel, Subscribe, ResumeSession, CycleModel, SetModel, CommShare, CommMessage, ...
- **Events:** TextDelta, ToolStart, ToolResult, TurnComplete, TokenUsage, Notification, SwarmStatus, ...

</details>

<details>
<summary><strong>TUI Rendering</strong></summary>

<br>

```mermaid
graph LR
    Frame["render_frame()"]

    Frame --> Layout["Layout Calculation<br>header · messages · input · status"]
    Layout --> MD["Markdown Parsing<br>parse_markdown() → Vec&lt;Block&gt;"]
    MD --> Syntax["Syntax Highlighting<br>50+ languages"]
    Syntax --> Wrap["Text Wrapping<br>terminal width"]
    Wrap --> Render["Render to Terminal<br>crossterm backend"]

    style Frame fill:#ec4899,color:#fff
    style Syntax fill:#8b5cf6,color:#fff
    style Render fill:#10b981,color:#fff
```

**Rendering Performance:**

| Mode | Avg Frame Time | FPS | Memory |
|---|---|---|---|
| Idle (200 turns) | 0.68 ms | 1,475 | 18 MB |
| Streaming | 0.67 ms | 1,498 | 18 MB |

*Measured with 200 conversation turns, full markdown + syntax highlighting, 120×40 terminal.*

**Key UI Components:**
- **InfoWidget** - floating panel showing model, context usage, todos, session count
- **Session Picker** - interactive split-pane browser with conversation previews
- **Mermaid Diagrams** - rendered natively as inline images (Sixel/Kitty/iTerm2 protocols)
- **Visual Debug** - frame-by-frame capture for debugging rendering

</details>

<details>
<summary><strong>Session & Memory</strong></summary>

<br>

```mermaid
graph TB
    Agent["Agent"] --> Session["Session<br><i>session_abc123_fox</i>"]
    Agent --> Memory["Memory System"]
    Agent --> Compaction["Compaction Manager"]

    Session --> Storage["~/.jcode/sessions/<br>session_*.json"]

    Memory --> Global["Global Memories<br>~/.jcode/memory/global.json"]
    Memory --> Project["Project Memories<br>~/.jcode/memory/projects/{hash}.json"]

    Compaction --> Summary["Background Summarization<br><i>When context hits 80% of limit</i>"]
    Compaction --> RAG["Full History Kept<br><i>for RAG search</i>"]

    style Agent fill:#8b5cf6,color:#fff
    style Session fill:#3b82f6,color:#fff
    style Memory fill:#10b981,color:#fff
    style Compaction fill:#f97316,color:#fff
```

**Compaction:** When context approaches the token limit, older turns are summarized in the background while recent turns are kept verbatim. Full history is always available for RAG search.

**Memory Categories:** `Fact` · `Preference` · `Entity` · `Correction` - with semantic search, graph traversal, and automatic extraction at session end.

</details>

<details>
<summary><strong>MCP Integration</strong></summary>

<br>

```mermaid
graph LR
    Manager["MCP Manager"] --> Client1["MCP Client<br>JSON-RPC 2.0 / stdio"]
    Manager --> Client2["MCP Client"]
    Manager --> Client3["MCP Client"]

    Client1 --> S1["playwright"]
    Client2 --> S2["filesystem"]
    Client3 --> S3["custom server"]

    style Manager fill:#8b5cf6,color:#fff
    style S1 fill:#3b82f6,color:#fff
    style S2 fill:#10b981,color:#fff
    style S3 fill:#64748b,color:#fff
```

Configure in `.claude/mcp.json` (project) or `~/.claude/mcp.json` (global):

```json
{
  "servers": {
    "playwright": {
      "command": "npx",
      "args": ["@anthropic/mcp-playwright"]
    }
  }
}
```

Tools are auto-registered as `mcp__servername__toolname` and available immediately.

</details>

<details>
<summary><strong>Self-Dev Mode</strong></summary>

<br>

```mermaid
graph TB
    Stable["Stable Binary<br>(promoted)"]

    Stable --> A["Session A<br>stable"]
    Stable --> B["Session B<br>stable"]
    Stable --> C["Session C<br>canary"]

    C --> Reload["selfdev reload<br><i>Hot-restart with new binary</i>"]
    Reload -->|"crash"| Rollback["Auto-Rollback<br>to stable"]
    Reload -->|"success"| Promote["selfdev promote<br><i>Mark as new stable</i>"]

    style Stable fill:#10b981,color:#fff
    style C fill:#f97316,color:#fff
    style Rollback fill:#ef4444,color:#fff
    style Promote fill:#10b981,color:#fff
```

jcode can develop itself - edit code, build, hot-reload, and test in-place. If the canary crashes, it auto-rolls back to the last stable binary and wakes with crash context.

</details>

<details>
<summary><strong>Module Map</strong></summary>

<br>

```mermaid
graph TB
    main["main.rs"] --> tui["tui/"]
    main --> server["server.rs"]
    main --> agent["agent.rs"]

    server --> protocol["protocol.rs"]
    server --> bus["bus.rs"]

    tui --> protocol
    tui --> bus

    agent --> session["session.rs"]
    agent --> compaction["compaction.rs"]
    agent --> provider["provider/"]
    agent --> tools["tool/"]
    agent --> mcp["mcp/"]

    provider --> auth["auth/"]
    tools --> memory["memory.rs"]
    mcp --> skill["skill.rs"]
    auth --> config["config.rs"]
    config --> storage["storage.rs"]
    storage --> id["id.rs"]

    style main fill:#f97316,color:#fff
    style agent fill:#8b5cf6,color:#fff
    style tui fill:#ec4899,color:#fff
    style server fill:#6366f1,color:#fff
    style provider fill:#3b82f6,color:#fff
    style tools fill:#10b981,color:#fff
```

**~92,000 lines of Rust** across 106 source files.

</details>

---

<div align="center">

## OpenClaw — Ambient Mode

</div>

OpenClaw is jcode's always-on autonomous agent. When you're not actively coding, it runs in the background — gardening your memory graph, doing proactive work, and staying reachable via Telegram.

Think of it like a brain consolidating memories during sleep: it merges duplicates, resolves contradictions, verifies stale facts against your codebase, and extracts missed context from crashed sessions.

**Key capabilities:**

- **Memory gardening** — consolidates duplicates, prunes dead memories, discovers new relationships, backfills embeddings
- **Proactive work** — analyzes recent sessions and git history to identify useful tasks you'd appreciate being surprised by
- **Telegram integration** — sends status updates and accepts directives mid-cycle via bot replies
- **Self-scheduling** — the agent decides when to wake next, constrained by adaptive resource limits that never starve interactive sessions
- **Safety-first** — code changes go on worktree branches with permission requests; conservative by default

<details>
<summary><strong>Ambient Cycle Architecture</strong></summary>

<br>

```mermaid
graph TB
    subgraph "Scheduling Layer"
        EV[Event Triggers<br/>session close, crash, git push]
        TM[Timer<br/>agent-scheduled wake]
        RC[Resource Calculator<br/>adaptive interval]
        SQ[(Scheduled Queue<br/>persistent)]
    end

    subgraph "Ambient Agent"
        QC[Check Queue]
        SC[Scout<br/>memories + sessions + git]
        GD[Garden<br/>consolidate + prune + verify]
        WK[Work<br/>proactive tasks]
        SA[schedule_ambient tool<br/>set next wake + context]
    end

    subgraph "Resource Awareness"
        UH[Usage History<br/>rolling window]
        RL[Rate Limits<br/>per provider]
        AU[Ambient Usage<br/>current window]
        AC[Active Sessions<br/>user activity]
    end

    EV -->|wake early| RC
    TM -->|scheduled wake| RC
    RC -->|"gate: safe to run?"| QC
    SQ -->|pending items| QC
    QC --> SC
    SC --> GD
    SC --> WK
    SA -->|next wake + context| SQ
    SA -->|proposed interval| RC

    UH --> RC
    RL --> RC
    AU --> RC
    AC --> RC

    style EV fill:#fff3e0
    style TM fill:#fff3e0
    style RC fill:#ffcdd2
    style SQ fill:#e3f2fd
    style QC fill:#e8f5e9
    style SC fill:#e8f5e9
    style GD fill:#e8f5e9
    style WK fill:#e8f5e9
```

</details>

<details>
<summary><strong>Two-Layer Memory Consolidation</strong></summary>

<br>

Memory consolidation happens at two levels — fast inline checks during sessions, and deep graph-wide passes during ambient cycles:

```mermaid
graph LR
    subgraph "Layer 1: Sidecar (every turn, fast)"
        S1[Memory retrieved<br/>for relevance check]
        S2{New memory<br/>similar to existing?}
        S3[Reinforce existing<br/>+ breadcrumb]
        S4[Create new memory]
        S5[Supersede if<br/>contradicts]
    end

    subgraph "Layer 2: Ambient Garden (background, deep)"
        A1[Full graph scan]
        A2[Cross-session<br/>dedup]
        A3[Fact verification<br/>against codebase]
        A4[Retroactive<br/>session extraction]
        A5[Prune dead<br/>memories]
        A6[Relationship<br/>discovery]
    end

    S1 --> S2
    S2 -->|yes| S3
    S2 -->|no| S4
    S2 -->|contradicts| S5

    A1 --> A2
    A1 --> A3
    A1 --> A4
    A1 --> A5
    A1 --> A6

    style S1 fill:#e8f5e9
    style S2 fill:#e8f5e9
    style S3 fill:#e8f5e9
    style S4 fill:#e8f5e9
    style S5 fill:#e8f5e9
    style A1 fill:#e3f2fd
    style A2 fill:#e3f2fd
    style A3 fill:#e3f2fd
    style A4 fill:#e3f2fd
    style A5 fill:#e3f2fd
    style A6 fill:#e3f2fd
```

</details>

<details>
<summary><strong>Provider Selection & Scheduling</strong></summary>

<br>

OpenClaw prefers subscription-based providers (OAuth) so ambient cycles never burn API credits silently:

```mermaid
graph TD
    START[Ambient Mode Start] --> CHECK1{OpenAI OAuth<br/>available?}
    CHECK1 -->|yes| OAI[Use OpenAI<br/>strongest available]
    CHECK1 -->|no| CHECK2{Anthropic OAuth<br/>available?}
    CHECK2 -->|yes| ANT[Use Anthropic<br/>strongest available]
    CHECK2 -->|no| CHECK3{API key or OpenRouter +<br/>config opt-in?}
    CHECK3 -->|yes| API[Use API/OpenRouter<br/>with budget cap]
    CHECK3 -->|no| DISABLED[Ambient mode disabled<br/>no provider available]

    style OAI fill:#e8f5e9
    style ANT fill:#fff3e0
    style API fill:#ffcdd2
    style DISABLED fill:#f5f5f5
```

The system adapts scheduling based on rate limit headers, user activity, and budget:

| Condition | Behavior |
|-----------|----------|
| User is active | Pause or throttle heavily |
| User idle for hours | Run more frequently |
| Hit a rate limit | Exponential backoff |
| Approaching end of window with budget left | Squeeze in extra cycles |

</details>

---

<div align="center">

## Environment Variables

| Variable | Description |
|---|---|
| `ANTHROPIC_API_KEY` | Direct API key (overrides OAuth) |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `JCODE_ANTHROPIC_MODEL` | Override default Claude model |
| `JCODE_OPENROUTER_MODEL` | Override default OpenRouter model |
| `JCODE_ANTHROPIC_DEBUG` | Log API request payloads |

</div>

---

<div align="center">

## macOS Notes

</div>

jcode runs natively on macOS (Apple Silicon & Intel). Key differences:

- **Sockets** use `$TMPDIR` instead of `$XDG_RUNTIME_DIR` (override with `$JCODE_RUNTIME_DIR`)
- **Clipboard** uses `osascript` / `NSPasteboard` for image paste
- **Terminal spawning** auto-detects Kitty, WezTerm, Alacritty, iTerm2, Terminal.app
- **Mermaid diagrams** rendered via pure-Rust SVG with Core Text font discovery

---

<div align="center">

## Testing

</div>

```bash
cargo test                          # All tests
cargo test --test e2e               # End-to-end only
cargo run --bin jcode-harness       # Tool harness (--include-network for web)
scripts/agent_trace.sh              # Full agent smoke test
```

---

<div align="center">

**Built with Rust** · **MIT License**

[GitHub](https://github.com/1jehuang/jcode) · [Report Bug](https://github.com/1jehuang/jcode/issues) · [Request Feature](https://github.com/1jehuang/jcode/issues)

</div>
