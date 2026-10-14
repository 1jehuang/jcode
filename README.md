<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/jcode-mark-inverse.svg">
  <img src="assets/brand/jcode-mark.svg" alt="jcode halftone donut logo" width="160">
</picture>

# jcode

[![Last Commit](https://badgen.net/github/last-commit/1jehuang/jcode/master?icon=github)](https://github.com/1jehuang/jcode/commits/master)
[![GitHub Stars](https://badgen.net/github/stars/1jehuang/jcode?icon=github)](https://github.com/1jehuang/jcode/stargazers)
[![GitHub Downloads](https://img.shields.io/github/downloads/1jehuang/jcode/total?style=flat-square)](https://github.com/1jehuang/jcode/releases)
[![Discord](https://img.shields.io/badge/Discord-Join%20Community-5865F2?style=flat-square&logo=discord&logoColor=white)](https://discord.gg/nBe9vGyK9a)

High-performance coding agents. <br>
Run more agents with less RAM.

<a href="https://trendshift.io/repositories/25042?utm_source=repository-badge&amp;utm_medium=badge&amp;utm_campaign=badge-repository-25042" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/repositories/25042" alt="1jehuang/jcode | Trendshift" width="250" height="55"></a>

<a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-yc-launch.mp4">
  <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-yc-launch.webp" alt="jcode YC launch video" width="800">
</a>

<br>

[Website](https://jcode.sh) · [Docs](https://jcode.sh/docs) · [SDK](https://jcode.sh/sdk) · [Benchmarks](https://jcode.sh/bench) · [Install](#installation) · [Quick Start](#quick-start) · [Further Reading](#further-reading) · [Contributing](CONTRIBUTING.md)

</div>

---

<div align="center">

## Installation

</div>

```bash
# macOS & Linux
curl -fsSL https://jcode.sh/install | bash
```

```powershell
# Windows 11 (PowerShell 5.1+)
irm https://jcode.sh/install.ps1 | iex
```

See the [documentation](https://jcode.sh/docs) for installation options and provider configuration.

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
          <tr>
            <td><strong>Antigravity CLI</strong></td>
            <td align="right"><strong>243.7 MB</strong></td>
            <td align="right"><strong>8.8× more RAM</strong></td>
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
          <tr>
            <td><strong>Antigravity CLI</strong></td>
            <td align="right"><strong>1021.2 MB</strong></td>
            <td align="right"><strong>8.7× more RAM</strong></td>
          </tr>
        </tbody>
      </table>
    </td>
  </tr>
</table>

</div>

### Headless sessions (swarm workers)

Swarm workers run headless, so this is the number that matters when you fan out
agents. Each session completed 5 real model turns (file listing, file read, repo
search, summary, reply) with tool calls, then total PSS of every process was
measured. jcode sessions share one daemon; Claude Code runs one
`claude -p --input-format stream-json` process per session. Both used
`claude-sonnet-4-6`.

<div align="center">

| Concurrent headless sessions | jcode | Claude Code | Comparison |
|---:|---:|---:|---:|
| 1 | **32.6 MB** | 261.0 MB | **8.0× less RAM** |
| 5 | **51.0 MB** | 908.6 MB | **17.8× less RAM** |
| 10 | **66.7 MB** | 1749.7 MB | **26.2× less RAM** |
| 20 | **90.6 MB** | 3376.8 MB | **37.3× less RAM** |
| Each additional session | **~3.1 MB** | ~164 MB | **~54× less RAM** |

</div>

Measured 2026-09-29 on Linux with `jcode v0.89.19-dev` (default build, local
embeddings not compiled in) and `Claude Code 2.1.267`. Reproduce with
`python3 scripts/bench_headless_memory.py`.

### Time to first frame

<div align="center">

| Tool | Time to first frame | Range | Comparison |
|---|---:|---:|---:|
| **jcode** | **14.0 ms** | 10.1–19.3 ms | baseline |
| **Antigravity CLI** | **383.5 ms** | 363.1–415.4 ms | **27.4× slower** |
| **pi** | **590.7 ms** | 369.6–934.8 ms | **42.2× slower** |
| **Codex CLI** | **882.8 ms** | 742.3–1640.9 ms | **63.1× slower** |
| **OpenCode** | **1035.9 ms** | 922.5–1104.4 ms | **74.0× slower** |
| **GitHub Copilot CLI** | **1518.6 ms** | 1357.4–1826.8 ms | **108.5× slower** |
| **Cursor Agent** | **1949.7 ms** | 1711.0–2104.8 ms | **139.3× slower** |
| **Claude Code** | **3436.9 ms** | 2032.7–8927.2 ms | **245.5× slower** |

</div>

Measured on this Linux machine across 10 interactive PTY launches.

### Time to first input
(time until typed probe text appears on the rendered screen; Antigravity uses its internal input-ready log marker because the sign-in screen suppresses probe echo.)
<div align="center">

| Tool | Time to first input | Range | Comparison |
|---|---:|---:|---:|
| **jcode** | **48.7 ms** | 30.3–62.7 ms | baseline |
| **Antigravity CLI** | **383.7 ms** | 363.4–415.7 ms | **7.9× slower** |
| **pi** | **596.4 ms** | 373.9–955.2 ms | **12.2× slower** |
| **Codex CLI** | **905.8 ms** | 760.1–1675.7 ms | **18.6× slower** |
| **OpenCode** | **1047.9 ms** | 931.1–1116.9 ms | **21.5× slower** |
| **GitHub Copilot CLI** | **1583.4 ms** | 1422.8–1880.0 ms | **32.5× slower** |
| **Cursor Agent** | **1978.7 ms** | 1727.3–2130.0 ms | **40.6× slower** |
| **Claude Code** | **3512.8 ms** | 2137.4–9002.0 ms | **72.2× slower** |

</div>

Measured on this Linux machine across 10 interactive PTY launches. Antigravity CLI was unauthenticated for this run; its sign-in screen rendered normally and emitted an internal `CLI ready for user input` marker, but did not echo the typed probe.

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
| **Antigravity CLI** | **~86.4 MB** | **8.7× more RAM** |

</div>
versions tested for this corrected memory rerun:

- `jcode v0.9.1888-dev (be386f2)`
- `pi 0.62.0`
- `codex-cli 0.120.0`
- `opencode 1.0.203`
- `GitHub Copilot CLI 1.0.24` for the 1-session rerun, `GitHub Copilot CLI 1.0.27` for the 10-session rerun
- `Cursor Agent 2026.04.08-a41fba1`
- `Claude Code 2.1.86 (Claude Code)`
- `Antigravity CLI 1.0.0`

<div align="center">

  <a href="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-performance-demo.mp4">
    <img src="https://github.com/1jehuang/jcode/releases/download/readme-assets/jcode-performance-demo.webp" alt="jcode performance demonstration" width="900">
  </a>

  <p><em>jcode performance demonstration</em></p>

</div>


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

Swarm modes keep root reasoning separate from worker effort. Configure each mode
in `~/.jcode/config.toml`:

```toml
[agents]
swarm_root_effort = "low"        # /effort swarm
swarm_deep_root_effort = "high"  # /effort swarm-deep
```

Both default to `max`. Accepted levels are `none`, `minimal`, `low`, `medium`,
`high`, `xhigh`, and `max`, mapped to the provider's supported range. The effort
switcher shows the configured root level. These settings do not change worker
`swarm_effort`. Environment overrides are `JCODE_SWARM_ROOT_EFFORT` and
`JCODE_SWARM_DEEP_ROOT_EFFORT`.

### Agent model scope

`/agents` configures worker models for the current session only. Choices persist
when that session resumes and do not change other Jcode instances. Target a role
with `/agents swarm`, `/agents review`, `/agents judge`, `/agents memory`, or
`/agents ambient`. Session ambient choices apply to scheduled workers spawned
from that session, not the daemon's independent ambient loop.

Use `/agents default [role]` or `/agents global [role]` to edit saved global
defaults. **Use global default** removes a session override. **Inherit coordinator**
explicitly bypasses a global pin. An explicit model on a worker spawn still wins.
Changing routing affects future workers, not workers already running. Memory
choices affect extraction only, not Jev recall.

## OAuth and Providers

jcode works with subscription-backed OAuth flows and many provider integrations, so you can use the models you already pay for and still fall back to direct API providers when needed.

### Supported built-in login flows

- **Claude** (`jcode login --provider claude`)
- **Claude Code CLI** (`jcode login --provider claude-code [--account <instance>]`): Claude through the official `claude` binary, side by side with native Claude. See [Claude Code mode](OAUTH.md#claude-code-mode-opt-in-side-by-side-with-native-claude).
- **OpenAI / ChatGPT / Codex** (`jcode login --provider openai`, browser callback on `http://localhost:1455/auth/callback`; see [OAUTH.md](OAUTH.md))
- **Google Gemini** (`jcode login --provider gemini`)
- **GitHub Copilot** (`jcode login --provider copilot`)
- **Azure OpenAI** (`jcode login --provider azure`)
- **Alibaba Cloud Coding Plan** (`jcode login --provider alibaba-coding-plan`)
- **Fireworks** (`jcode login --provider fireworks`)
- **Novita AI** (`jcode login --provider novita`, API key)
- **MiniMax** (`jcode login --provider minimax`)
- **Meta Model API / Muse** (`jcode login --provider meta-muse`)
- **LM Studio** (`jcode login --provider lmstudio`)
- **Ollama** (`jcode login --provider ollama`)
- **Custom OpenAI-compatible endpoint** (`jcode login --provider openai-compatible`)

For custom OpenAI-compatible endpoints, jcode now prompts for the API base and supports local localhost servers without requiring an API key.

Azure OpenAI accepts a resource URL or a base URL ending in `/openai/v1`, a
deployment name, and either an API key or Microsoft Entra ID. Agent turns use
`/openai/v1/responses`, including tool calls with reasoning enabled. The
Responses route carries encrypted reasoning and tool results between turns;
`/chat/completions` cannot combine function tools with reasoning on GPT-5.6
and later Azure deployments. The saved Azure credentials remain in
`~/.config/jcode/azure-openai.env`. Azure omits reasoning effort unless you
set `AZURE_OPENAI_REASONING_EFFORT` for a deployment that supports it; this
avoids sending the global OpenAI default to non-reasoning deployments.
See the [Azure reasoning model guide](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/reasoning#tool-calling-with-reasoning-models).

The native OpenAI providers use Responses WebSocket v2 with opportunistic
background prewarming and HTTPS fallback. See [OpenAI WebSocket transport](docs/OPENAI_WEBSOCKET.md)
for behavior, controls, and verification.

### Supported providers

- **Native / first-party style providers:** `claude`, `openai`, `copilot`, `gemini`, `azure`, `alibaba-coding-plan`
- **Aggregator / compatibility providers:** `openrouter`, `orcarouter`, `yolo-auto`, `openai-compatible`
- **Additional provider integrations:** `opencode`, `opencode-go`, `zai` / `kimi`, `302ai`, `baseten`, `cortecs`, `deepseek`, `firmware`, `huggingface`, `moonshotai`, `nebius`, `scaleway`, `stackit`, `groq`, `mistral`, `perplexity`, `togetherai`, `deepinfra`, `fireworks`, `novita`, `minimax`, `xai`, `lmstudio`, `ollama`, `chutes`, `cerebras`, `cursor`, `antigravity`, `google`

Jcode also supports easy multi-account switching. Ran out of tokens on your first ChatGPT Pro subscription? /account and quickly switch to your second. 

Accounts are per window: `/account switch claude-fox` (or Enter in the `/account` picker) moves only this window, so two windows can use two subscriptions of the same provider at once. `/account default claude-fox` (or `d` in the picker) sets the account new windows start with, `/account unpin` returns this window to the default, and `/account failover on|off|status` controls automatic moves to the next saved account when this window's account runs out. From the command line, `jcode --account claude-fox` and `jcode run --account openai-otter ...` pick the account for that window or run, and `jcode login claude --default` makes a new login the default.

---

## Customizability / Self-Dev

Jcode is inventing a new form of customizability. One that doesn't limit you to what a plugin or extension can do. Tell your jcode agent to enter self dev mode, and it will start modifying its own source code. Jcode is optimized to iterate on itself. There is significant infrastructure around self development, which allows it to edit, build, and test its own source code, then reload its own binary and continue work in your (potentially many) sessions, fully automatically.

It is recommended that you use a frontier model for this. The jcode codebase is not a simple one, and weaker models can make subtle, breaking changes. GPT 5.5 or the latest available frontier model works well.

<!-- Add self-dev demo thumbnail/video and fuller writeup here. -->

---

## Misc.

The devil is in the details. There are many undocumented optimizations and niceties that jcode implements. Some examples: 

Anthropic's Claude cache goes cold after 5 minutes. If you initiate Claude after these 5 minutes, you have a cache miss, potentially costing you lots of tokens. The ui warns you when the cache went cold, and notfies you if there was an unexpected cache miss. 

jcode comes with instructions on how to set up Firefox Agent Bridge. Ask you agent to set it up, and then you will have browser automation in jcode as well. 

Agent grep is a grep tool I made for the jcode agent. It adds file structure information (ie the list of functions, their displacement, etc) to the grep return, so that the agent can infer more of what the file doesn without actually reading the file. It also implements a harness-level integration that adaptively truncates returns based on what the agent has already seen. This saves on context a lot.

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

The built-in `browser` tool lets agents navigate pages, fill forms, upload files,
and capture screenshots through Firefox Agent Bridge. Ask your agent to set it
up and handle browser tasks directly.

On macOS, Brave, Chrome, Opera, and Firefox use Browser Agent Bridge, and Safari
uses a native AppleScript backend. Pick one with `jcode browser setup <browser>`.
Page actions there need an explicit `tab_id`, and file upload and screenshots are
unavailable. See [macOS browser validation](docs/MACOS_BROWSER_VALIDATION.md).

See [Browser Provider Protocol](docs/BROWSER_PROVIDER_PROTOCOL.md) for details.

---

## Further Reading

- [jcode.sh/docs](https://jcode.sh/docs) — install, providers, configuration, keybindings
- [jcode.sh/swarm](https://jcode.sh/swarm) — many coding agents in one repository
- [jcode.sh/sdk](https://jcode.sh/sdk) — TypeScript SDK: drive jcode sessions from your own program
- [jcode.sh/bench](https://jcode.sh/bench) — benchmark methodology and results
- [Ambient Mode / OpenClaw](docs/AMBIENT_MODE.md)
- [Browser Provider Protocol](docs/BROWSER_PROVIDER_PROTOCOL.md)
- [Memory Architecture](docs/MEMORY_ARCHITECTURE.md)
- [Swarm Architecture](docs/SWARM_ARCHITECTURE.md)
- [Server Architecture](docs/SERVER_ARCHITECTURE.md)
- [Safety System](docs/SAFETY_SYSTEM.md)
- [Sponsored Discovery Sponsor Onboarding](docs/SPONSORED_DISCOVERY_SPONSOR_ONBOARDING.md)
- [Windows Notes](docs/WINDOWS.md)
- [Wrappers and Shell Integration](docs/WRAPPERS.md)
- [Refactoring Notes](docs/REFACTORING.md)
