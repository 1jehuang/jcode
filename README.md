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

<a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-memory-demo.mp4">
  <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-memory-demo.webp" alt="jcode memory demonstration" width="800">
</a>

<br>

[Features](#features) · [Install](#installation) · [Quick Start](#quick-start) · [Further Reading](#further-reading) · [Contributing](CONTRIBUTING.md)

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
            <th>Comparison</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td><strong>jcode (local embedding off)</strong></td>
            <td align="right"><strong>27.8 MB</strong></td>
            <td align="right">baseline</td>
          </tr>
          <tr>
            <td><strong>jcode</strong></td>
            <td align="right"><strong>167.1 MB</strong></td>
            <td align="right"><strong>6.0× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>pi</strong></td>
            <td align="right"><strong>144.4 MB</strong></td>
            <td align="right"><strong>5.2× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>Codex CLI</strong></td>
            <td align="right"><strong>140.0 MB</strong></td>
            <td align="right"><strong>5.0× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>OpenCode</strong></td>
            <td align="right"><strong>371.5 MB</strong></td>
            <td align="right"><strong>13.4× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>GitHub Copilot CLI</strong></td>
            <td align="right"><strong>333.3 MB</strong></td>
            <td align="right"><strong>12.0× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>Cursor Agent</strong></td>
            <td align="right"><strong>214.9 MB</strong></td>
            <td align="right"><strong>7.7× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>Claude Code</strong></td>
            <td align="right"><strong>386.6 MB</strong></td>
            <td align="right"><strong>13.9× more RAM</strong></td>
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
            <th>Comparison</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td><strong>jcode (local embedding off)</strong></td>
            <td align="right"><strong>117.0 MB</strong></td>
            <td align="right">baseline</td>
          </tr>
          <tr>
            <td><strong>jcode</strong></td>
            <td align="right"><strong>260.8 MB</strong></td>
            <td align="right"><strong>2.2× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>pi</strong></td>
            <td align="right"><strong>833.0 MB</strong></td>
            <td align="right"><strong>7.1× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>Codex CLI</strong></td>
            <td align="right"><strong>334.8 MB</strong></td>
            <td align="right"><strong>2.9× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>OpenCode</strong></td>
            <td align="right"><strong>3237.2 MB</strong></td>
            <td align="right"><strong>27.7× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>GitHub Copilot CLI</strong></td>
            <td align="right"><strong>1756.5 MB</strong></td>
            <td align="right"><strong>15.0× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>Cursor Agent</strong></td>
            <td align="right"><strong>1632.4 MB</strong></td>
            <td align="right"><strong>14.0× more RAM</strong></td>
          </tr>
          <tr>
            <td><strong>Claude Code</strong></td>
            <td align="right"><strong>2300.6 MB</strong></td>
            <td align="right"><strong>19.7× more RAM</strong></td>
          </tr>
        </tbody>
      </table>
    </td>
  </tr>
</table>

</div>

### Time to first frame

<div align="center">

| Tool | Time to first frame | Range | Comparison |
|---|---:|---:|---:|
| **jcode** | **14.0 ms** | 10.1–19.3 ms | baseline |
| **pi** | **590.7 ms** | 369.6–934.8 ms | **42.2× slower** |
| **Codex CLI** | **882.8 ms** | 742.3–1640.9 ms | **63.1× slower** |
| **OpenCode** | **1035.9 ms** | 922.5–1104.4 ms | **74.0× slower** |
| **GitHub Copilot CLI** | **1518.6 ms** | 1357.4–1826.8 ms | **108.5× slower** |
| **Cursor Agent** | **1949.7 ms** | 1711.0–2104.8 ms | **139.3× slower** |
| **Claude Code** | **3436.9 ms** | 2032.7–8927.2 ms | **245.5× slower** |

</div>

Measured on this Linux machine across 10 interactive PTY launches.

### Time to first input
(time until typed probe text appears on the rendered screen.)
<div align="center">

| Tool | Time to first input | Range | Comparison |
|---|---:|---:|---:|
| **jcode** | **48.7 ms** | 30.3–62.7 ms | baseline |
| **pi** | **596.4 ms** | 373.9–955.2 ms | **12.2× slower** |
| **Codex CLI** | **905.8 ms** | 760.1–1675.7 ms | **18.6× slower** |
| **OpenCode** | **1047.9 ms** | 931.1–1116.9 ms | **21.5× slower** |
| **GitHub Copilot CLI** | **1583.4 ms** | 1422.8–1880.0 ms | **32.5× slower** |
| **Cursor Agent** | **1978.7 ms** | 1727.3–2130.0 ms | **40.6× slower** |
| **Claude Code** | **3512.8 ms** | 2137.4–9002.0 ms | **72.2× slower** |

</div>

Measured on this Linux machine across 10 interactive PTY launches.

### Additional clients / memory scaling

<div align="center">

| Tool | Extra PSS per added session | Comparison |
|---|---:|---:|
| **jcode (local embedding off)** | **~9.9 MB** | baseline |
| **jcode** | **~10.4 MB** | **1.1× more RAM** |
| **pi** | **~76.5 MB** | **7.7× more RAM** |
| **Codex CLI** | **~21.6 MB** | **2.2× more RAM** |
| **OpenCode** | **~318.4 MB** | **32.2× more RAM** |
| **GitHub Copilot CLI** | **~158.1 MB** | **16.0× more RAM** |
| **Cursor Agent** | **~157.5 MB** | **15.9× more RAM** |
| **Claude Code** | **~212.7 MB** | **21.5× more RAM** |

</div>
versions tested for this corrected memory rerun:

- `jcode v0.9.1888-dev (be386f2)`
- `pi 0.62.0`
- `codex-cli 0.120.0`
- `opencode 1.0.203`
- `GitHub Copilot CLI 1.0.24` for the 1-session rerun, `GitHub Copilot CLI 1.0.27` for the 10-session rerun
- `Cursor Agent 2026.04.08-a41fba1`
- `Claude Code 2.1.86 (Claude Code)`

<div align="center">

  <a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-performance-demo.mp4">
    <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-performance-demo.webp" alt="jcode performance demonstration" width="900">
  </a>

  <p><em>jcode performance demonstration</em></p>

</div>


---

## Memory (Agent memory)

Jcode embeds each turn/response as a semantic vector. Every turn does queries a graph of memories to efficiently find related memory entries via a cosine similarity check. The embedding hits are fed into the conversation, or optionally uses a memory sideagent which verifies the memories are relevant, and potentially does more work for information retreival before injecting into the conversation. This results in a human like memory system which allows the agent to automatically recall relevant information to the conversation without actively calling memory tools or being a token burner. 
ot 
To have memories which are retrieved, they must also be extracted and stored. Every so often (semantic drift, K turns since last extraction, session end, etc), memories are extracted via a memory sideagent, and put into the memory graph. 

The harness also provides explicit memory tools to allow the agent to actively search or store the memory without relying on a passive background process. The harness also provides session search for traditional RAG on previous sessions. 

Memories are automatically consolidated every so often via the ambient mode. This reorganizes, checks for staleness and conflicts, etc

<div align="center">

  <a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-memory-demo.mp4">
    <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-memory-demo.webp" alt="jcode memory demonstration" width="900">
  </a>

  <p><em>jcode memory demonstration</em></p>

</div>

<!-- Memory demo media is hosted in the readme-assets release. -->

---

## UI: Side panels, Diagrams, Info Widgets, rendering, scrolling, alignment

The side panel is a place for auxiliary information. Tell your jcode agent to load a file into the side panel and see it update in real time, or tell your agent to write directly to the side panel, or use it as a diff viewer. The side panel (and chat) is able to render mermaid diagrams inline. 
<img width="2877" height="1762" alt="image" src="https://github.com/user-attachments/assets/6c7bec81-ef3f-434d-8a7b-d55f8a54e5cf" />

To make this possible, I created a new mermaid rendering library to render diagrams 1800x faster. It has no browser or Typescript dependency. See https://github.com/1jehuang/mermaid-rs-renderer

To show you important information without taking space away from the screen that could be used for responses, I developed info widgets. Info widgets will only ever take up the negative space on the screen to show you information, and will get out of the way if there isn't any. 

Jcode can render at over a thousand fps. Your monitor will not have the refresh rate to show you, but this means you will not have silly flicker problems. 

The custom scrollback implementation of jcode allows it to do much more than a native scrollback. However, it is a terminal-level limitation that I cannot have smooth, partial line scrolling with a custom scrollback. To fix this, I made my own terminal. Handterm https://github.com/1jehuang/handterm implements a native scroll api, and also happens to be very effiecent. This is a work in progress. Scrolling is still well implemented for normal terminals.

Jcode is left-aligned by default. You can switch to centered mode with the `Alt+C` hotkey, with the `/alignment` command, or in the config.

---

## Swarm

Spawn two or more agents in the same repo, and they will automatically be managed by the server to allow native collaboration. When agent A edits a file that agent B has read (code shifting under its feet), the server notifies agent B. Agent B can ignore it if it is not relevant, or it can check the diff to make sure that it doesn't conflict. Each agent has messaging abilities, capable of DMing just one agent, broadcasting to all other agents hosted by the server, or just agents working in that repo. This allows you to spawn multiple sessions in the same repo, and have all conflicts automatically resolved.

<div align="center">

  <a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/swarm-demo.mp4">
    <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-swarm-demonstration.webp" alt="jcode swarm demonstration" width="900">
  </a>

  <p><em>jcode swarm demonstration</em></p>

</div>

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

For agents and scripts, the preferred path is the one-shot provider profile command. It writes a named profile to `~/.jcode/config.toml`, stores secrets in jcode's private app config directory when requested, and prints exact run/validation commands:

```bash
# Secret-safe setup for a hosted OpenAI-compatible API.
printf '%s' "$MY_API_KEY" | jcode provider add my-api \
  --base-url https://llm.example.com/v1 \
  --model my-model-id \
  --api-key-stdin \
  --set-default \
  --json

# Smoke test the profile.
jcode --provider-profile my-api auth-test --prompt 'Reply exactly JCODE_PROVIDER_SETUP_OK'

# Use it directly.
jcode --provider-profile my-api run 'hello'
```

For local servers that do not require auth:

```bash
jcode provider add local-vllm \
  --base-url http://localhost:8000/v1 \
  --model Qwen/Qwen3-Coder-30B-A3B-Instruct \
  --no-api-key \
  --set-default
```

Useful flags:

- `--api-key-env NAME`: reference an existing environment variable instead of storing a key.
- `--api-key-stdin`: read and store a key without putting it in shell history.
- `--context-window TOKENS`: persist the model context window for model selection and routing.
- `--overwrite`: replace an existing profile of the same name.
- `--model-catalog`: use the endpoint's `/models` response in addition to configured models.

The generated profile can also be edited manually in `~/.jcode/config.toml`:

```toml
[provider]
default_provider = "my-api"
default_model = "my-model-id"

[providers.my-api]
type = "openai-compatible"
base_url = "https://llm.example.com/v1"
api_key_env = "JCODE_PROVIDER_MY_API_API_KEY"
env_file = "provider-my-api.env"
default_model = "my-model-id"

[[providers.my-api.models]]
id = "my-model-id"
context_window = 128000
```

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

It is reccomended that you use a frontier model for this. The jcode codebase is not a simple one, and weaker models can make subtle, breaking changes. GPT 5.5 or the latest available frontier model works well.

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

<div align="center">

  <a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/workflow.mp4">
    <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-workflow-demonstration.webp" alt="jcode workflow demonstration" width="900">
  </a>

  <p><em>jcode workflow demonstration</em></p>

</div>

---

## Browser Automation

jcode includes a first-class built-in `browser` tool for browser control inside agent sessions.

Current built-in backend:
- Firefox via Firefox Agent Bridge

Current built-in tool actions include:
- `status`
- `setup`
- `open`
- `snapshot`
- `get_content`
- `interactables`
- `click`
- `type`
- `fill_form`
- `select`
- `wait`
- `screenshot`
- `eval`
- `scroll`
- `upload`
- `press`

Quick setup:

```bash
jcode browser status
jcode browser setup
```

Once setup is complete, the model can use the built-in `browser` tool directly. The UI also summarizes browser tool calls compactly, for example opening a URL, clicking a selector, or typing into a field without echoing sensitive typed text.

Notes:
- the provider/tool architecture is in place for additional backends
- Firefox is the wired built-in backend today
- Chrome bridge / remote debugging style providers can be added on top of the same browser tool later

---

## Further Reading

- [Ambient Mode / OpenClaw](docs/AMBIENT_MODE.md)
- [Browser Provider Protocol](docs/BROWSER_PROVIDER_PROTOCOL.md)
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
8. If I want browser automation, also check `jcode browser status`. If browser automation is not ready, run `jcode browser setup`, verify the built-in `browser` tool works, and explain any remaining manual step.
9. Explain any manual step that still needs me, especially browser OAuth, device login, API key entry, or browser extension approval.
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

### Verifying release artifacts

Each release ships SHA256 manifests alongside the binaries:

- `SHA256SUMS` — Linux + macOS tarballs
- `SHA256SUMS-windows` — Windows tarballs and `.exe` artifacts

Verify before installing:

```bash
VERSION=v0.11.1
ARTIFACT=jcode-linux-x86_64.tar.gz   # or jcode-macos-aarch64.tar.gz

curl -LO "https://github.com/1jehuang/jcode/releases/download/${VERSION}/${ARTIFACT}"
curl -LO "https://github.com/1jehuang/jcode/releases/download/${VERSION}/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
```

Expected output:

```
jcode-linux-x86_64.tar.gz: OK
```

A non-zero exit or `FAILED` line means the download was corrupted or
tampered with — do not extract or install it.

On macOS, `sha256sum` is not in the default toolchain; use `shasum -a 256 -c`
instead, or install `coreutils` (`brew install coreutils` then `gsha256sum`).

On Windows (PowerShell):

```powershell
$VERSION = "v0.11.1"
$ARTIFACT = "jcode-windows-x86_64.tar.gz"

Invoke-WebRequest "https://github.com/1jehuang/jcode/releases/download/$VERSION/$ARTIFACT" -OutFile $ARTIFACT
Invoke-WebRequest "https://github.com/1jehuang/jcode/releases/download/$VERSION/SHA256SUMS-windows" -OutFile SHA256SUMS-windows

$expected = (Select-String -Path SHA256SUMS-windows -Pattern "  $ARTIFACT$").Line.Split(' ')[0]
$actual   = (Get-FileHash $ARTIFACT -Algorithm SHA256).Hash.ToLower()

if ($expected -eq $actual) { "OK: $ARTIFACT" } else { throw "MISMATCH: $ARTIFACT" }
```

Sigstore / cosign signatures and installer-side automatic verification are
tracked separately in [#63](https://github.com/1jehuang/jcode/issues/63).

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
