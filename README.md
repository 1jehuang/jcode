<div align="center">

# jcode

[![Latest Release](https://img.shields.io/github/v/release/1jehuang/jcode?style=flat-square)](https://github.com/1jehuang/jcode/releases)
[![License](https://img.shields.io/github/license/1jehuang/jcode?style=flat-square)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-blue?style=flat-square)](https://github.com/1jehuang/jcode/releases)
[![Commit Activity](https://img.shields.io/github/commit-activity/m/1jehuang/jcode?style=flat-square)](https://github.com/1jehuang/jcode/commits/master)
[![GitHub Stars](https://img.shields.io/github/stars/1jehuang/jcode?style=flat-square)](https://github.com/1jehuang/jcode/stargazers)

A blazing-fast, fully autonomous AI coding agent with a gorgeous TUI,
multi-model support, swarm coordination, persistent memory, and 30+ built-in tools -
all running natively in your terminal.

<br>

<img src="https://github.com/1jehuang/jcode/releases/download/v0.3.1/jcode_demo_jaguar.avif" alt="jcode demo" width="800">

<br>

[Features](#features) · [Install](#installation) · [Quick Start](#quick-start) · [Further Reading](#further-reading)

</div>

---

<div align="center">

## Installation

</div>

```bash
# macOS & Linux
curl -fsSL https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.sh | bash
```

Need Windows, Homebrew, source builds, provider setup, or tell your agent to set it up for you?
[Jump to detailed installation](#detailed-installation).

---


<div align="center">

## Performance & Resource Efficiency

</div>

jcode is built to be as performant and resource efficient as possible. Every metric is optimized to the bone, which is important for scaling multi-session workflows. Here we sample two metrics to show the difference: RAM usage and startup time.

<!-- Add performance demo thumbnail/video link here: spawning many jcode instances and using them in parallel. -->

### Headline numbers

- **jcode uses 7.2× less PSS than pi**
- **jcode uses 22× less PSS than OpenCode**
- **jcode uses 25× less PSS than Claude Code**
- **jcode spawns 44.4× faster than pi**
- **jcode spawns 66.8× faster than OpenCode**
- **jcode spawns 24.3× faster than Claude Code**

### Memory benchmarks: 10 simultaneous sessions

Measured on this Linux machine using real interactive PTY sessions and Linux `/proc` memory stats. For jcode, the number includes both client memory and the incremental memory growth of the shared server, which is the fair comparison for many active sessions.

Versions tested:

- `jcode v0.8.16-dev (161f9fa)`
- `pi-coding-agent 0.62.0` (`pi`)
- `opencode 1.0.203`
- `Claude Code 2.1.86`

### Interactive startup time

Measured as median **time to first terminal output** across 10 PTY launches on this Linux machine.

<div align="center">

| Tool | Median startup | Range |
|---|---:|---:|
| **jcode** | **13.6 ms** | 9.0–16.9 ms |
| **Claude Code** | **331.1 ms** | 274.5–466.1 ms |
| **pi** | **603.9 ms** | 522.3–701.4 ms |
| **opencode** | **908.9 ms** | 785.5–1014.6 ms |

</div>

<div align="center">

| Tool | 1 active session | 10 active sessions | Avg per session at 10 | Architecture |
|---|---:|---:|---:|---|
| **jcode** | **28.2 MB RSS** / **8.9 MB PSS** | **433.3 MB RSS** / **140.7 MB PSS** | **43.3 MB RSS** / **14.1 MB PSS** | shared server + lightweight clients |
| **pi** | **168.4 MB RSS** / **156.2 MB PSS** | **1554.9 MB RSS** / **1010.6 MB PSS** | **155.5 MB RSS** / **101.1 MB PSS** | mostly per-session Node process |
| **opencode** | **377.0 MB RSS** / **372.6 MB PSS** | **3665.2 MB RSS** / **3135.1 MB PSS** | **366.5 MB RSS** / **313.5 MB PSS** | mostly per-session monolith |
| **Claude Code** | **677.0 MB RSS** / **674.1 MB PSS** | **4880.9 MB RSS** / **3460.2 MB PSS** | **488.1 MB RSS** / **346.0 MB PSS** | mostly per-session monolith |

</div>

### Additional memory per added session

<div align="center">

| Tool | Extra RSS per added session | Extra PSS per added session |
|---|---:|---:|
| **jcode** | **~45.0 MB** | **~14.6 MB** |
| **pi** | **~154.1 MB** | **~94.9 MB** |
| **opencode** | **~365.4 MB** | **~306.9 MB** |
| **Claude Code** | **~467.1 MB** | **~309.6 MB** |

</div>

---

## Memory (Agent memory)

Jcode embedds each turn/response as a semantic vector. Every turn does queries a graph of memories to efficiently find related memory entries via a cosine similarity check. The embedding hits are fed into the conversation, or optionally uses a memory sideagent which verifies the memories are relevant, and potentially does more work for information retreival before injecting into the conversation. This results in a human like memory system which allows the agent to automatically recall relevant information to the conversation without actively calling memory tools or being a token burner. 
ot 
To have memories which are retrieved, they must also be extracted and stored. Every so often (semantic drift, K turns since last extraction, session end, etc), memories are extracted via a memory sideagent, and put into the memory graph. 

The harness also provides explicit memory tools to allow the agent to actively search or store the memory without relying on a passive background process. The harness also provides session search for traditional RAG on previous sessions. 

Memories are automatically consolidated every so often via the ambient mode. This reorganizes, checks for stalenss, etc

<!-- Add memory demo thumbnail/video and fuller writeup here. -->

---

## UI: Side panels, Diagrams, Info Widgets, rendering, scrolling, alignment

The side panel is a place for auxiliary information. Tell your jcode agent to load a file into the side panel and see it update in real time, or tell your agent to write directly to the side panel, or use it as a diff viewer. The side panel (and chat) is able to render mermaid diagrams inline. 
<img width="2877" height="1762" alt="image" src="https://github.com/user-attachments/assets/6c7bec81-ef3f-434d-8a7b-d55f8a54e5cf" />

To make this possible, I created a new mermaid rendering library to render diagrams 1800x faster. It has no browser or Typescript dependency. See https://github.com/1jehuang/mermaid-rs-renderer

To show you important information without taking space away from the screen that could be used for responses, I developed info widgets. Info widgets will only ever take up the negative space on the screen to show you information, and will get out of the way if there isn't any. 

Jcode can render at over a thousand fps. Your monitor will not have the refresh rate to show you, but this means you will not have silly flicker problems. 

The custom scrollback implementation of jcode allows it to do much more than a native scrollback. However, it is a terminal-level limitation that I cannot have smooth, partial line scrolling with a custom scrollback. To fix this, I made my own terminal. Handterm https://github.com/1jehuang/handterm implements a native scroll api, and also happens to be very effiecent. This is a work in progress. Scrolling is still well implemented for normal terminals.

Yes, you can change the alignment to be left-aligned. I prefer the centered mode, but to each their own. You can change this with the `Alt, C` hotkey, or with the /aligment command, or in the config.

---

## Swarm

Spawn two or more agents in the same repo, and they will automatically be managed by the server to allow native collaboration. When agent A edits a file that agent B has read (code shifting under its feet), the server notifies agent B. Agent B can ignore it if it is not relevant, or it can check the diff to make sure that it doesn't conflict. Each agent has messaging abilities, capable of DMing just one agent, broadcasting to all other agents hosted by the server, or just agents working in that repo. This allows you to spawn multiple sessions in the same repo, and have all conflicts automatically resolved. 

https://github.com/user-attachments/assets/0b69a309-e4c2-4721-8e4d-b5a93d8865ed

The above is a video of me manually managing a swarm of 20 agents. Some working on the jcode codebase, and some not. This is only possible with the performance/resource efficiency noted in the above section on client/session memory usage. 

Agents are also able to spawn their own swarms autonomously. They have a swarm tool which allows them to spawn in their own teamates to accomplish tasks in parallel. Doing so turns the main agent into a coordinator and the spawned agents into workers. Groups of agents, their messaging channels, their completion statuses, etc are all automatically managed. This can be done headlessly or headed. 

---

## OAuth and Providers

jcode works with subscription-backed OAuth flows and many provider integrations, so you can use the models you already pay for and still fall back to direct API providers when needed.

### Supported built-in login flows

- **Claude** (`jcode login --provider claude`)
- **OpenAI / ChatGPT / Codex** (`jcode login --provider openai`)
- **Google Gemini** (`jcode login --provider gemini`)
- **Google / Gmail tools** (`jcode login --provider google`)
- **GitHub Copilot** (`jcode login --provider copilot`)
- **Azure OpenAI** (`jcode login --provider azure`)
- **Alibaba Cloud Coding Plan** (`jcode login --provider alibaba-coding-plan`)
- **Fireworks** (`jcode login --provider fireworks`)
- **MiniMax** (`jcode login --provider minimax`)

<img width="2877" height="1762" alt="Screenshot from 2026-04-02 14-28-51" src="https://github.com/user-attachments/assets/530684c0-9d12-4363-aa0e-1b39a0d4e1be" />
The above image is the first page of provider logins

### Supported provider

- **Native / first-party style providers:** `jcode`, `claude`, `openai`, `copilot`, `gemini`, `azure`, `alibaba-coding-plan`
- **Aggregator / compatibility providers:** `openrouter`, `openai-compatible`
- **Additional provider integrations:** `opencode`, `opencode-go`, `zai` / `kimi`, `302ai`, `baseten`, `cortecs`, `deepseek`, `firmware`, `huggingface`, `moonshotai`, `nebius`, `scaleway`, `stackit`, `groq`, `mistral`, `perplexity`, `togetherai`, `deepinfra`, `fireworks`, `minimax`, `xai`, `lmstudio`, `ollama`, `chutes`, `cerebras`, `cursor`, `antigravity`, `google`

Jcode also supports easy multi-account switching. Ran out of tokens on your first ChatGPT Pro subscription? /account and quickly switch to your second. 

---

## Customizability / Self-Dev

Jcode is inventing a new form of customizability. One that doesn't limit you to what a plugin or extension can do. Tell your jcode agent to enter self dev mode, and it will start modifying its own source code. Jcode is optimized to iterate on itself. There is significant infrastructure around self developement, which allows it to edit, build, and test its own source code, then reload its own binary and continue work in your (potentially many) sessions, fully automatically. 

<!-- Add self-dev demo thumbnail/video and fuller writeup here. -->

---

## Misc.

The devil is in the details. There are many undocumented optimizations and niceties that jcode implements. Some examples: 

Anthropic's Claude cache goes cold after 5 minutes. If you initiate Claude after these 5 minutes, you have a cache miss, potentially costing you lots of tokens. The ui warns you when the cache went cold, and notfies you if there was an unexpected cache miss. 

jcode comes with instructions on how to set up Firefox Agent Bridge. Ask you agent to set it up, and then you will have browser automation in jcode as well. 

Agent grep is a grep tool I made for the jcode agent. It adds file strucuture information (ie the list of functions, their displacement, etc) to the grep return, so that the agent can infer more of what the file doesn without actually reading the file. It also implements a harness-level integration that adaptively truncates returns based on what the agent has already seen. This saves on context a lot. 

Inputs are by default interleaved with the working agent. It sends the input as soon as it safely can without breaking the KV cache. Submit with shift enter instead, and it will send a queue send, and wait for the agent to fully finish its turn before sending.

---

## iOS Application for Native OpenClaw

A native iOS implementation of OpenClaw is coming soon.

<!-- Add iOS / native OpenClaw preview and fuller writeup here. -->

---

<div align="center">

## Quick Start

</div>

```bash
# Launch the TUI
jcode

# Run a single command non-interactively
jcode run "say hello"

# Resume a previous session by memorable name
jcode --resume fox

# Run as a persistent background server, then attach more clients
jcode serve
jcode connect

# Send voice input from your configured STT command
jcode dictate
```

jcode supports interactive TUI use, non-interactive runs, persistent server/client workflows,
and hotkey-friendly dictation without requiring a bundled speech-to-text stack.

---

## Further Reading

- [Ambient Mode / OpenClaw](docs/AMBIENT_MODE.md)
- [Memory Architecture](docs/MEMORY_ARCHITECTURE.md)
- [Swarm Architecture](docs/SWARM_ARCHITECTURE.md)
- [Server Architecture](docs/SERVER_ARCHITECTURE.md)
- [iOS Client Notes](docs/IOS_CLIENT.md)
- [Safety System](docs/SAFETY_SYSTEM.md)
- [Windows Notes](docs/WINDOWS.md)
- [Wrappers and Shell Integration](docs/WRAPPERS.md)
- [Refactoring Notes](docs/REFACTORING.md)

---

## Detailed Installation

### Setup

If you want another agent to set up jcode for you, give it this prompt:

```text
Set up jcode on this machine for me.

1. Detect the operating system, available package managers, and shell environment, then install jcode using the best matching command below instead of referring me somewhere else:

   - macOS with Homebrew available:
     brew tap 1jehuang/jcode
     brew install jcode

   - macOS or Linux via install script:
     curl -fsSL https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.sh | bash

   - Windows PowerShell:
     irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.ps1 | iex

   - From source if the above paths are not appropriate:
     git clone https://github.com/1jehuang/jcode.git
     cd jcode
     cargo build --release
     scripts/install_release.sh

   - For local self-dev / refactor work on Linux x86_64, prefer:
     scripts/dev_cargo.sh build --release -p jcode --bin jcode
     scripts/dev_cargo.sh --print-setup
     scripts/install_release.sh

2. Verify that `jcode` is on my `PATH`.
3. Launch `jcode` once in a new terminal window/session to confirm it starts successfully.
4. Before attempting any interactive login flow, assess which providers are already available non-interactively and prefer those first. Check existing local credentials, config files, CLI sessions, and environment variables such as:
   - Claude: `~/.jcode/auth.json`, `~/.claude/.credentials.json`, `~/.local/share/opencode/auth.json`, `ANTHROPIC_API_KEY`
   - OpenAI: `~/.jcode/openai-auth.json`, `~/.codex/auth.json`, `OPENAI_API_KEY`
   - Gemini: `~/.jcode/gemini_oauth.json`, `~/.gemini/oauth_creds.json`
   - GitHub Copilot: existing auth under `~/.config/github-copilot/`
   - Azure OpenAI: `~/.config/jcode/azure-openai.env`, `AZURE_OPENAI_*`, or an existing `az login`
   - OpenRouter: `OPENROUTER_API_KEY`
   - Fireworks: `~/.config/jcode/fireworks.env`, `FIREWORKS_API_KEY`
   - MiniMax: `~/.config/jcode/minimax.env`, `MINIMAX_API_KEY`
   - Alibaba Cloud Coding Plan: existing jcode config/env if present
5. Prefer whichever provider is already configured and verify it with `jcode auth-test --all-configured` or a provider-specific auth test when appropriate.
6. Only if no usable provider is already configured, guide me through the minimal manual step needed:
   - Claude: `jcode login --provider claude`
   - GitHub Copilot: `jcode login --provider copilot`
   - OpenAI: `jcode login --provider openai`
   - Gemini: `jcode login --provider gemini`
   - Azure OpenAI: `jcode login --provider azure`
   - Fireworks: `jcode login --provider fireworks`
   - MiniMax: `jcode login --provider minimax`
   - Alibaba Cloud Coding Plan: `jcode login --provider alibaba-coding-plan`
   - OpenRouter: help me set `OPENROUTER_API_KEY`
   - Anthropic direct API: help me set `ANTHROPIC_API_KEY`
7. After setup, run a simple smoke test with `jcode run "say hello"` and confirm it works.
8. Explain any manual step that still needs me, especially browser OAuth, device login, or API key entry.
```

This is intended to be a copy-paste bootstrap prompt for jcode itself or any other coding agent.

### Quick Install

```bash
# macOS & Linux
curl -fsSL https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.sh | bash
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.ps1 | iex
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

For local self-dev / refactor work on Linux x86_64, prefer:

```bash
scripts/dev_cargo.sh build --release -p jcode --bin jcode
scripts/dev_cargo.sh --print-setup
```

That wrapper automatically uses `sccache` when available, prefers a fast
working local linker setup (`clang + lld`) instead of assuming every machine's
`mold` configuration is valid, and can print the active linker/cache setup via
`--print-setup` so slow-path builds are easier to diagnose.

Then symlink to your PATH:

```bash
scripts/install_release.sh
```

### Platform Support

| Platform | Status |
|---|---|
| **Linux** x86_64 / aarch64 | Fully supported |
| **macOS** Apple Silicon & Intel | Supported |
| **Windows** x86_64 | Supported (native + WSL2) |

</div>
