<div align="center">

# jcode

[![Latest Release](https://img.shields.io/github/v/release/1jehuang/jcode?style=flat-square)](https://github.com/1jehuang/jcode/releases)
[![License](https://img.shields.io/github/license/1jehuang/jcode?style=flat-square)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-blue?style=flat-square)](https://github.com/1jehuang/jcode/releases)
[![Commit Activity](https://img.shields.io/github/commit-activity/m/1jehuang/jcode?style=flat-square)](https://github.com/1jehuang/jcode/commits/master)
[![GitHub Stars](https://img.shields.io/github/stars/1jehuang/jcode?style=flat-square)](https://github.com/1jehuang/jcode/stargazers)

The next generation coding agent harness to raise the skill ceiling. <br>
Built for multi-session workflows, infinite customizability, and performance. 

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

jcode is built to be as performant and resource efficient as possible. Every metric is optimized to the bone, which is important for scaling multi-session workflows. Here we sample a few metrics to show the difference: RAM usage and boot up.

### RAM comparison

<div align="center">

<table>
  <tr>
    <td valign="top" align="center" width="50%">
      <strong>1 active session</strong>
      <table>
        <thead>
          <tr>
            <th>Tool</th>
            <th>PSS</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td><strong>jcode</strong></td>
            <td align="right"><strong>8.9 MB</strong></td>
          </tr>
          <tr>
            <td><strong>pi</strong></td>
            <td align="right"><strong>156.2 MB</strong></td>
          </tr>
          <tr>
            <td><strong>OpenCode</strong></td>
            <td align="right"><strong>372.6 MB</strong></td>
          </tr>
          <tr>
            <td><strong>Claude Code</strong></td>
            <td align="right"><strong>674.1 MB</strong></td>
          </tr>
        </tbody>
      </table>
    </td>
    <td width="24"></td>
    <td valign="top" align="center" width="50%">
      <strong>10 active sessions</strong>
      <table>
        <thead>
          <tr>
            <th>Tool</th>
            <th>PSS</th>
            <th>jcode improvement multiplier</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td><strong>jcode</strong></td>
            <td align="right"><strong>140.7 MB</strong></td>
            <td align="right">baseline</td>
          </tr>
          <tr>
            <td><strong>pi</strong></td>
            <td align="right"><strong>1010.6 MB</strong></td>
            <td align="right"><strong>7.2× less PSS</strong></td>
          </tr>
          <tr>
            <td><strong>OpenCode</strong></td>
            <td align="right"><strong>3135.1 MB</strong></td>
            <td align="right"><strong>22.2× less PSS</strong></td>
          </tr>
          <tr>
            <td><strong>Claude Code</strong></td>
            <td align="right"><strong>3460.2 MB</strong></td>
            <td align="right"><strong>24.5× less PSS</strong></td>
          </tr>
        </tbody>
      </table>
    </td>
  </tr>
</table>

</div>

Measured on this Linux machine using real interactive PTY sessions and Linux `/proc` memory stats. For jcode, the PSS number includes both client memory and the incremental memory growth of the shared server, which is the fair comparison for many active sessions.

The multiplier column is based on average PSS per session at 10 active sessions, where lower is better.

### Time to first frame

<div align="center">

| Tool | Time to first frame | Range | jcode improvement multiplier |
|---|---:|---:|---:|
| **jcode** | **14.0 ms** | 10.1–19.3 ms | baseline |
| **pi** | **590.7 ms** | 369.6–934.8 ms | **42.2× faster** |
| **Codex CLI** | **882.8 ms** | 742.3–1640.9 ms | **63.1× faster** |
| **OpenCode** | **1035.9 ms** | 922.5–1104.4 ms | **74.0× faster** |
| **GitHub Copilot CLI** | **1518.6 ms** | 1357.4–1826.8 ms | **108.5× faster** |
| **Cursor Agent** | **1949.7 ms** | 1711.0–2104.8 ms | **139.3× faster** |
| **Claude Code** | **3436.9 ms** | 2032.7–8927.2 ms | **245.5× faster** |

</div>

Measured on this Linux machine across 10 interactive PTY launches.

### Time to first input
(time until typed probe text appears on the rendered screen.)
<div align="center">

| Tool | Time to first input | Range | jcode improvement multiplier |
|---|---:|---:|---:|
| **jcode** | **48.7 ms** | 30.3–62.7 ms | baseline |
| **pi** | **596.4 ms** | 373.9–955.2 ms | **12.2× faster** |
| **Codex CLI** | **905.8 ms** | 760.1–1675.7 ms | **18.6× faster** |
| **OpenCode** | **1047.9 ms** | 931.1–1116.9 ms | **21.5× faster** |
| **GitHub Copilot CLI** | **1583.4 ms** | 1422.8–1880.0 ms | **32.5× faster** |
| **Cursor Agent** | **1978.7 ms** | 1727.3–2130.0 ms | **40.6× faster** |
| **Claude Code** | **3512.8 ms** | 2137.4–9002.0 ms | **72.2× faster** |

</div>

Measured on this Linux machine across 10 interactive PTY launches.

### Additional clients / memory scaling

A 10-session snapshot is useful, but the scaling story matters just as much. The table below shows how much memory each additional active session adds, which is where a shared-server architecture compounds its advantage.

<div align="center">

| Tool | Extra RSS per added session | Extra PSS per added session | jcode improvement multiplier |
|---|---:|---:|---:|
| **jcode** | **~45.0 MB** | **~14.6 MB** | baseline |
| **pi** | **~154.1 MB** | **~94.9 MB** | **6.5× less PSS** |
| **OpenCode** | **~365.4 MB** | **~306.9 MB** | **21.0× less PSS** |
| **Claude Code** | **~467.1 MB** | **~309.6 MB** | **21.2× less PSS** |

</div>
versions tested:

- `jcode v0.9.366-dev (dbd480c, dirty)`
- `pi 0.62.0`
- `opencode 1.0.203`
- `codex-cli 0.118.0`
- `Claude Code 2.1.86`
- `Cursor Agent 2026.03.18-f6873f7`
- `GitHub Copilot CLI 0.0.421`

<div align="center">

  <img src="assets/readme/jcode-performance-demonstration.gif" alt="jcode performance demonstration" width="900">

  <p><em>jcode performance demonstration</em></p>

</div>

The above GIF shows jcode spawning 102 sessions.

---

## Memory (Agent memory)

Jcode embeds each turn/response as a semantic vector. Every turn does queries a graph of memories to efficiently find related memory entries via a cosine similarity check. The embedding hits are fed into the conversation, or optionally uses a memory sideagent which verifies the memories are relevant, and potentially does more work for information retreival before injecting into the conversation. This results in a human like memory system which allows the agent to automatically recall relevant information to the conversation without actively calling memory tools or being a token burner. 
ot 
To have memories which are retrieved, they must also be extracted and stored. Every so often (semantic drift, K turns since last extraction, session end, etc), memories are extracted via a memory sideagent, and put into the memory graph. 

The harness also provides explicit memory tools to allow the agent to actively search or store the memory without relying on a passive background process. The harness also provides session search for traditional RAG on previous sessions. 

Memories are automatically consolidated every so often via the ambient mode. This reorganizes, checks for staleness and conflicts, etc

<!-- Add memory demo thumbnail/video and fuller writeup here. -->

---

## UI: Side panels, Diagrams, Info Widgets, rendering, scrolling, alignment

The side panel is a place for auxiliary information. Tell your jcode agent to load a file into the side panel and see it update in real time, or tell your agent to write directly to the side panel, or use it as a diff viewer. The side panel (and chat) is able to render mermaid diagrams inline. 
<img width="2877" height="1762" alt="image" src="https://github.com/user-attachments/assets/6c7bec81-ef3f-434d-8a7b-d55f8a54e5cf" />

To make this possible, I created a new mermaid rendering library to render diagrams 1800x faster. It has no browser or Typescript dependency. See https://github.com/1jehuang/mermaid-rs-renderer

To show you important information without taking space away from the screen that could be used for responses, I developed info widgets. Info widgets will only ever take up the negative space on the screen to show you information, and will get out of the way if there isn't any. 

Jcode can render at over a thousand fps. Your monitor will not have the refresh rate to show you, but this means you will not have silly flicker problems. 

The custom scrollback implementation of jcode allows it to do much more than a native scrollback. However, it is a terminal-level limitation that I cannot have smooth, partial line scrolling with a custom scrollback. To fix this, I made my own terminal. Handterm https://github.com/1jehuang/handterm implements a native scroll api, and also happens to be very effiecent. This is a work in progress. Scrolling is still well implemented for normal terminals.

Yes, you can change the alignment to be left-aligned. I prefer the centered mode, but to each their own. You can change this with the `Alt, C` hotkey, or with the /alignment command, or in the config.

---

## Swarm

Spawn two or more agents in the same repo, and they will automatically be managed by the server to allow native collaboration. When agent A edits a file that agent B has read (code shifting under its feet), the server notifies agent B. Agent B can ignore it if it is not relevant, or it can check the diff to make sure that it doesn't conflict. Each agent has messaging abilities, capable of DMing just one agent, broadcasting to all other agents hosted by the server, or just agents working in that repo. This allows you to spawn multiple sessions in the same repo, and have all conflicts automatically resolved. 

https://github.com/user-attachments/assets/161df3c5-a7b5-43c8-bb47-771a49164aea
The above video is me managing 20 agent sessions

Agents are also able to spawn their own swarms autonomously. They have a swarm tool which allows them to spawn in their own teamates to accomplish tasks in parallel. Doing so turns the main agent into a coordinator and the spawned agents into workers. Groups of agents, their messaging channels, their completion statuses, etc are all automatically managed. This can be done headlessly or headed. 

---

## OAuth and Providers

jcode works with subscription-backed OAuth flows and many provider integrations, so you can use the models you already pay for and still fall back to direct API providers when needed.

### Supported built-in login flows

- **Claude** (`jcode login --provider claude`)
- **OpenAI / ChatGPT / Codex** (`jcode login --provider openai`)
- **Google Gemini** (`jcode login --provider gemini`)
- **GitHub Copilot** (`jcode login --provider copilot`)
- **Azure OpenAI** (`jcode login --provider azure`)
- **Alibaba Cloud Coding Plan** (`jcode login --provider alibaba-coding-plan`)
- **Fireworks** (`jcode login --provider fireworks`)
- **MiniMax** (`jcode login --provider minimax`)
- **LM Studio** (`jcode login --provider lmstudio`)
- **Ollama** (`jcode login --provider ollama`)
- **Custom OpenAI-compatible endpoint** (`jcode login --provider openai-compatible`)

For custom OpenAI-compatible endpoints, jcode now prompts for the API base and supports local localhost servers without requiring an API key.

### Config-file setup for self-hosted endpoints and MCP

If you prefer to configure things by editing files instead of using the login UI, jcode supports both a custom OpenAI-compatible endpoint config and MCP config files.

#### Self-hosted OpenAI-compatible endpoints, including vLLM

The custom OpenAI-compatible provider reads overrides from environment variables or from an env file in jcode's app config directory. On Linux this is usually `~/.config/jcode/`, so the default file is usually:

```text
~/.config/jcode/openai-compatible.env
```

Example for a local or LAN vLLM server:

```bash
JCODE_OPENAI_COMPAT_API_BASE=http://192.168.1.50:8000/v1
JCODE_OPENAI_COMPAT_DEFAULT_MODEL=Qwen/Qwen3-Coder-30B-A3B-Instruct
# Optional if your server expects auth
OPENAI_COMPAT_API_KEY=your-token-here
```

Notes:

- `jcode login --provider openai-compatible` can create or update this for you.
- Plain `http://` is accepted for `localhost` and private LAN IPs. Public remote HTTP is still rejected.
- HTTPS endpoints work as usual.

#### MCP config files

MCP config is separate from `config.toml`.

Primary config files:

- `~/.jcode/mcp.json` for global MCP servers
- `.jcode/mcp.json` for project-local MCP servers

Compatibility fallback:

- `.claude/mcp.json`

Example MCP config:

```json
{
  "servers": {
    "filesystem": {
      "command": "/path/to/mcp-server",
      "args": ["--root", "/workspace"],
      "env": {},
      "shared": true
    }
  }
}
```

On first run, jcode also tries to import MCP servers from `~/.claude/mcp.json` and `~/.codex/config.toml` if `~/.jcode/mcp.json` does not exist yet.

For headless or SSH sessions, OAuth-style providers support `jcode login --provider <provider> --no-browser` (alias: `--headless`) so jcode prints the auth URL/QR and falls back to manual code or callback paste instead of trying to launch a local browser.

For more scriptable remote flows, `claude`, `openai`, `gemini`, and `antigravity` also support a two-step pattern:

```bash
# Step 1: print a resumable auth URL
jcode login --provider openai --print-auth-url --json

# Step 2: complete later with the callback URL or auth code
jcode login --provider openai --callback-url 'http://localhost:1455/auth/callback?...'
jcode login --provider gemini --auth-code '...'
```

Additional scriptable cases:

```bash
# Copilot device flow: print URL + user code, then complete later
jcode login --provider copilot --print-auth-url --json
jcode login --provider copilot --complete

# Gmail/Google OAuth after credentials are already configured
jcode login --provider google --print-auth-url --google-access-tier readonly
jcode login --provider google --callback-url 'http://127.0.0.1:8456?...'
```

Pending scriptable login state is stored under `~/.jcode/pending-login/`, automatically expires, and stale entries are cleaned up when new scriptable logins start or resume.

For the built-in OpenAI login flow, jcode opens a local callback on
`http://localhost:1455/auth/callback` by default.

<img width="2877" height="1762" alt="Screenshot from 2026-04-02 14-28-51" src="https://github.com/user-attachments/assets/530684c0-9d12-4363-aa0e-1b39a0d4e1be" />
The above image is the first page of provider logins

### Supported provider

- **Native / first-party style providers:** `claude`, `openai`, `copilot`, `gemini`, `azure`, `alibaba-coding-plan`
- **Aggregator / compatibility providers:** `openrouter`, `openai-compatible`
- **Additional provider integrations:** `opencode`, `opencode-go`, `zai` / `kimi`, `302ai`, `baseten`, `cortecs`, `deepseek`, `firmware`, `huggingface`, `moonshotai`, `nebius`, `scaleway`, `stackit`, `groq`, `mistral`, `perplexity`, `togetherai`, `deepinfra`, `fireworks`, `minimax`, `xai`, `lmstudio`, `ollama`, `chutes`, `cerebras`, `cursor`, `antigravity`, `google`

Jcode also supports easy multi-account switching. Ran out of tokens on your first ChatGPT Pro subscription? /account and quickly switch to your second. 

---

## Customizability / Self-Dev

Jcode is inventing a new form of customizability. One that doesn't limit you to what a plugin or extension can do. Tell your jcode agent to enter self dev mode, and it will start modifying its own source code. Jcode is optimized to iterate on itself. There is significant infrastructure around self developement, which allows it to edit, build, and test its own source code, then reload its own binary and continue work in your (potentially many) sessions, fully automatically. 

It is reccomended that you use a frontier model for this. The jcode codebase is not a simple one, and weaker models can make subtle, breaking changes. GPT 5.4+ class models work well. 

<!-- Add self-dev demo thumbnail/video and fuller writeup here. -->

---

## Misc.

The devil is in the details. There are many undocumented optimizations and niceties that jcode implements. Some examples: 

Anthropic's Claude cache goes cold after 5 minutes. If you initiate Claude after these 5 minutes, you have a cache miss, potentially costing you lots of tokens. The ui warns you when the cache went cold, and notfies you if there was an unexpected cache miss. 

jcode comes with instructions on how to set up Firefox Agent Bridge. Ask you agent to set it up, and then you will have browser automation in jcode as well. 

Agent grep is a grep tool I made for the jcode agent. It adds file strucuture information (ie the list of functions, their displacement, etc) to the grep return, so that the agent can infer more of what the file doesn without actually reading the file. It also implements a harness-level integration that adaptively truncates returns based on what the agent has already seen. This saves on context a lot. 

Inputs are by default interleaved with the working agent. It sends the input as soon as it safely can without breaking the KV cache. Submit with shift enter instead, and it will send a queue send, and wait for the agent to fully finish its turn before sending.

Resume sessions from different harnesses. Claude code broke on you? Resume the session from jcode and continue where you left off. Session resume is supported for codex, claude code, opencode, and pi. 

<img width="2877" height="1762" alt="Screenshot from 2026-04-11 16-28-52" src="https://github.com/user-attachments/assets/c2b383cf-2531-4217-85ae-6a863354dc97" />
image of /Resume for codex sessions


Skills are not all loaded on startup. The conversation is embedded as a semantic vector, and will automatically inject a skill if there is an embedding hit similar to memories. The agent has a skill tool for you to manually activate a skill at anytime. You may also activate via slash commands. 

---

## iOS Application / Native OpenClaw

A native iOS application version of jcode is coming soon. This will allow you to work with jcode on your personal machine's environment from your phone, via Tailscale. Openclaw like features will be bundled with this iOS application. 

---

## Other planned features

Agents dont like to commit in dirty git state with active changes. Git was clearly not built for multi-agent workflows, and git worktrees is not a good solution. Given this, I believe that is an opporunity for a new git like primitive to be born. 

Build speed improvements: An incremental debug cargo build with cache enabled takes about 1 minute on my machine. The goal is 5-20 seconds. Refactors and crates seams should be able to make this happen. 

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
