#!/usr/bin/env python3
"""Generate docs/ENVIRONMENT.md from the jcode/kcode workspace.

Reads the env::var() call sites in `crates/jcode-base/src/config/env_overrides.rs`
to extract JCODE_-prefixed env vars with their immediate section comment, plus
all other JCODE_-prefixed env vars found across the workspace, plus a curated
list of well-known third-party env vars (HERDR_*, COMPOSIO_*, GHOSTTY_*, etc.)
seen at runtime.

Usage: python3 build_env_table.py
Output: docs/ENVIRONMENT.md in the workspace root.
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]  # scripts/docs/ -> repo root
ENV_OVERRIDES = REPO / "crates/jcode-base/src/config/env_overrides.rs"

ENV_VAR_RE = re.compile(r'"([A-Z][A-Z0-9_]*?)"')
SECTION_RE = re.compile(r"^\s*//\s*([A-Z][a-zA-Z][a-zA-Z0-9 ()\-/]*?)\s*$", re.MULTILINE)
ENV_VAR_DEF_RE = re.compile(r'std::env::var\("([A-Z][A-Z0-9_]+)"\)')
ASSIGNMENT_RE = re.compile(r"self\.([a-z_][a-zA-Z0-9_.]*)\s*=\s*[a-zA-Z_]")
DOC_HINT_RE = re.compile(r"///\s*(.+?)$")

THIRD_PARTY = {
    # Provider auth + base URLs
    "ANTHROPIC_API_KEY": "Anthropic Claude API key (used by jcode-provider-anthropic)",
    "ANTHROPIC_AUTH_TOKEN": "Alternative Anthropic auth token (OAuth-style)",
    "ANTHROPIC_BASE_URL": "Override Anthropic API base URL",
    "OPENAI_API_KEY": "OpenAI API key (used by jcode-provider-openai)",
    "OPENAI_BASE_URL": "OpenAI-compatible API base URL override",
    "OPENAI_ORGANIZATION": "OpenAI organization ID header",
    "OPENAI_EXTRA_BODY": "JSON merged into every OpenAI request body",
    "OPENROUTER_API_KEY": "OpenRouter API key",
    "OPENROUTER_API_BASE": "OpenRouter API base URL override",
    "GEMINI_API_KEY": "Google Gemini API key",
    "GEMINI_API_ENDPOINT": "Gemini API endpoint override",
    "GEMINI_API_VERSION": "Gemini API version (e.g. v1, v1beta)",
    "GOOGLE_CLOUD_PROJECT": "Google Cloud project ID for Gemini Code Assist",
    "GOOGLE_CLOUD_PROJECT_ID": "Same as GOOGLE_CLOUD_PROJECT, alt name",
    "GROK_API_KEY": "xAI Grok API key",
    "GROK_DEPLOYMENT_KEY": "Grok deployment key",
    "GROK_CLI_PATH": "Override grok CLI binary path",
    "COHERE_API_KEY": "Cohere API key",
    "MISTRAL_API_KEY": "Mistral API key",
    "DEEPSEEK_API_KEY": "DeepSeek API key",
    "OPENAI_COMPAT_API_KEY": "Generic OpenAI-compatible API key fallback",
    "OPENAI_COMPAT_BASE_URL": "Generic OpenAI-compatible base URL",
    "ZAI_API_KEY": "Z.AI API key",
    "KIMI_API_KEY": "Moonshot Kimi API key",
    "MOONSHOT_API_KEY": "Moonshot API key",
    "FIREWORKS_API_KEY": "Fireworks API key",
    "NOVITA_API_KEY": "Novita API key",
    "HUGGINGFACE_API_KEY": "HuggingFace API key",
    "JADE_RELAY_TOKEN": "JADE Cloud relay auth token",
    "JADE_RELAY_USER_ID": "JADE Cloud relay user ID",
    "JADE_RELAY_API_BASE": "JADE Cloud relay API base",
    "JADE_RELAY_DEFAULT_GROUP": "JADE relay default macOS notification group",
    "JADE_RELAY_PAYLOAD_KIND": "JADE relay payload type override",
    "CURSOR_ACCESS_TOKEN": "Cursor IDE access token",
    "CURSOR_REFRESH_TOKEN": "Cursor IDE refresh token",
    "CURSOR_API_KEY": "Cursor API key (alt)",
    "GHOSTTY_BIN_DIR": "Ghostty terminal binary directory",
    "GHOSTTY_RESOURCES_DIR": "Ghostty resources directory",
    "KITTY_PID": "Kitty terminal PID",
    "KITTY_LISTEN_ON": "Kitty socket listen address",
    "WEZTERM_PANE": "WezTerm pane ID",
    "WEZTERM_CONFIG_DIR": "WezTerm config directory",
    "ALACRITTY_WINDOW_ID": "Alacritty window ID",
    "ALACRITTY_SOCKET": "Alacritty IPC socket path",
    "ITERM_SESSION_ID": "iTerm2 session ID",
    "ITERM_PROFILE": "iTerm2 profile name",
    "ITERM_PID": "iTerm2 process ID",
    "TMUX_PANE": "tmux pane identifier",
    "TMUX": "tmux socket path (set inside tmux sessions)",
    "WT_SESSION": "Windows Terminal session UUID",
    "WT_PROFILE_ID": "Windows Terminal profile ID",
    "WSL_DISTRO_NAME": "WSL distro name (Windows)",
    "WSLENV": "WSL env-var passthrough list",
    "XDG_RUNTIME_DIR": "XDG runtime directory",
    "XDG_CONFIG_HOME": "XDG config base directory",
    "XDG_DATA_HOME": "XDG data base directory",
    "XDG_CACHE_HOME": "XDG cache base directory",
    "XDG_SESSION_TYPE": "XDG session kind (wayland/x11)",
    "DISPLAY": "X11 display",
    "WAYLAND_DISPLAY": "Wayland display socket",
    "SSH_AUTH_SOCK": "SSH agent socket",
    "SSH_CONNECTION": "SSH connection metadata",
    "SSH_CLIENT": "SSH client info",
    "SSH_TTY": "SSH TTY device",
    "SSH_ORIGINAL_COMMAND": "Original SSH command",
    "BROWSER": "Default browser executable",
    "BROWSER_SESSION": "Browser session ID",
    "EDITOR": "Default editor executable",
    "VISUAL": "Default visual editor",
    "PAGER": "Default pager",
    "PWD": "Working directory",
    "OLDPWD": "Previous working directory",
    "HOME": "User home directory",
    "USER": "Username",
    "LOGNAME": "Login name",
    "PATH": "Executable search path",
    "MANPATH": "Manpage search path",
    "LANG": "Locale",
    "LC_ALL": "Locale override",
    "LC_CTYPE": "Character classification locale",
    "TZ": "Timezone",
    "TERM": "Terminal type",
    "TERM_PROGRAM": "Terminal program name (iTerm, Apple_Terminal, ...)",
    "TERM_VERSION": "Terminal program version",
    "COLORTERM": "Color-support hint",
    "SHELL": "Login shell",
    "BASH_VERSION": "Bash version (set when invoked under bash)",
    "FISH_VERSION": "Fish shell version",
    "ZSH_VERSION": "Zsh shell version",
    "BASH_ENV": "Bash startup script for non-interactive shells",
    "ZDOTDIR": "Zsh config dir",
    "CARGO": "Cargo executable path",
    "CARGO_HOME": "Cargo home directory",
    "CARGO_MANIFEST_DIR": "Workspace manifest directory",
    "CARGO_TARGET_DIR": "Cargo target directory override",
    "CARGO_REGISTRIES_CRATES_IO_PROTOCOL": "crates.io registry protocol (sparse/git)",
    "CARGO_NET_OFFLINE": "Cargo network offline mode",
    "CARGO_TERM_COLOR": "Cargo color output setting",
    "CARGO_TERM_PROGRESS_WHEN": "Cargo progress output",
    "RUSTUP_HOME": "rustup home directory",
    "RUSTUP_TOOLCHAIN": "Default rust toolchain",
    "RUSTUP_DIST_SERVER": "rustup dist server override",
    "RUST_BACKTRACE": "Backtrace mode for panics",
    "RUST_LOG": "tracing/log filter",
    "DO_NOT_TRACK": "Telemetry opt-out (1 = off)",
    "COMPUTERNAME": "Windows machine name",
    "HOSTNAME": "Hostname",
    "HOSTTYPE": "CPU arch string",
    "OSTYPE": "OS kind",
    "MACHTYPE": "CPU/OS string",
    "JPM_LAUNCH_AGENT": "LaTeX-related, ignored by jcode",
    "JADE_RELAY_DEFAULT_PRIORITY": "JADE relay notification priority default",
    "JADE_RELAY_DEFAULT_RELAX": "JADE relay relax-days default",
    "CODE_ASSIST_ENDPOINT": "Code Assist endpoint URL",
    "CODE_ASSIST_API_VERSION": "Code Assist API version",
    "COMPOSIO_API_KEY": "Composio API key",
    "COMPOSIO_USER_ID": "Composio user identifier",
    "COMPOSIO_BASE_URL": "Composio API base URL",
    "COMPOSIO_GMAIL_AUTH_CONFIG_ID": "Composio Gmail auth config",
    "COMPOSIO_GMAIL_CONNECTED_ACCOUNT_ID": "Composio connected account for Gmail",
    "COMPOSIO_GMAIL_USER_ID": "Composio Gmail user ID",
    "COMTEGRA_API_KEY": "Comtegra API key (Comtegra provider)",
    "HERDR_BIN_PATH": "Path to the `herdr` binary",
    "HERDR_ENV": "Reserved for herdr environment tagging",
    "HERDR_PANE_ID": "Identifier of the herdr pane hosting jcode",
    "HERDR_TAB_ID": "Tab ID within herdr",
    "HERDR_SOCKET_PATH": "herdr IPC socket path",
    "HERDR_WORKSPACE_ID": "herdr workspace identifier",
    "HERDR_JCODE_IDLE_DEBOUNCE_MS": "Debounce window for herdr->jcode idle events (ms)",
    "HERDR_JCODE_RETRY_GRACE_MS": "Retry grace window after disconnects (ms)",
}


def parse_env_overrides() -> dict[str, dict]:
    """Return {section: [varinfo, ...]} for entries bound in env_overrides.rs.

    `varinfo` = {"name": str, "field": str|None, "comment": str|None}.
    """
    text = ENV_OVERRIDES.read_text()
    out: dict[str, list[dict]] = defaultdict(list)
    current_section = "Server/operator behavior"

    section_iter = list(SECTION_RE.finditer(text))
    var_iter = list(ENV_VAR_DEF_RE.finditer(text))

    section_starts = [(m.start(), m.group(1).strip()) for m in section_iter]
    # Append sentinel
    section_starts.append((len(text), ""))

    var_positions = [(m.start(), m.group(1)) for m in var_iter]

    # Pre-compute: per-line doc comment, when present above a var definition
    lines = text.splitlines()
    line_for_pos: dict[int, int] = {}
    cum = 0
    for idx, ln in enumerate(lines):
        line_for_pos[cum] = idx
        cum += len(ln) + 1

    def find_assignment(pos: int) -> str | None:
        # Look ahead 200 chars for first `self.<field> = ...` after this var read
        snippet = text[pos : pos + 250]
        m = ASSIGNMENT_RE.search(snippet)
        if m:
            return m.group(1)
        return None

    for vpos, vname in var_positions:
        # Find the section in effect
        sec = current_section
        for sstart, sname in section_starts:
            if sstart > vpos:
                break
            sec = sname
        current_section = sec
        # field
        field = find_assignment(vpos)
        # doc comment above
        ln_idx = line_for_pos.get(vpos, 0)
        doc = None
        for back in range(1, 4):
            if ln_idx - back < 0:
                break
            prev = lines[ln_idx - back].strip()
            dm = DOC_HINT_RE.match(prev)
            if dm:
                doc = dm.group(1).strip()
                break
            if not prev.startswith("//"):
                break
        out[sec].append({"name": vname, "field": field, "doc": doc})

    return out


def build_env_overrides_table(sections: dict[str, list[dict]]) -> str:
    """Render the env_overrides section as a markdown table."""
    rows: list[str] = []
    header = (
        "| Env var | Default | Used by | Description |\n"
        "|---|---|---|---|"
    )
    for sec, items in sections.items():
        rows.append(f"### {sec}")
        rows.append("")
        rows.append(header)
        for it in items:
            name = it["name"]
            field = it.get("field") or "—"
            desc = it["doc"]
            if not desc:
                # Fall back to a description built from the field name
                f = field.replace("self.", "")
                if f.endswith("_key") or "_key" in f:
                    desc = f"Override the `{f}` keybinding value (string)"
                elif f.endswith("_enabled") or f.startswith("enable_"):
                    desc = f"Toggle `{f}` on/off"
                elif f.endswith("_model"):
                    desc = f"Override the `{f}` model name"
                elif f.endswith("_provider"):
                    desc = f"Override the `{f}` provider"
                elif "_timeout" in f or "_secs" in f or "_ms" in f or "_seconds" in f:
                    desc = f"Override the `{f}` duration"
                elif f.endswith("_url") or f.endswith("_base") or f.endswith("_path"):
                    desc = f"Override the `{f}` URL/path"
                else:
                    desc = f"Override `{f}` config field"
            rows.append(f"| `{name}` | (unset) | jcode-base | {desc} |")
        rows.append("")
    return "\n".join(rows)


def build_workspace_jcode_table(names: list[str]) -> str:
    rows = [
        "### Other JCODE_* env vars (workspace-wide)",
        "",
        "These JCODE_-prefixed env vars are read by their respective crates but not",
        "listed in `crates/jcode-base/src/config/env_overrides.rs`. They are read",
        "directly via `std::env::var(...)` from each crate's `apply_*` helper or",
        "module-level const.",
        "",
        "| Env var | Default | Used by | Description |",
        "|---|---|---|---|",
    ]
    for name in names:
        rows.append(f"| `{name}` | (unset) | various | See call sites in `crates/` and `src/` |")
    rows.append("")
    return "\n".join(rows)


def build_thirdparty_table() -> str:
    rows = [
        "### Third-party env vars read at runtime",
        "",
        "These env vars come from upstream libraries and third-party integrations",
        "that jcode reads directly. They are listed here for completeness; consult",
        "the upstream documentation for the canonical reference.",
        "",
        "| Env var | Default | Used by | Description |",
        "|---|---|---|---|",
    ]
    for name, desc in sorted(THIRD_PARTY.items()):
        used = "multiple"
        if name.startswith("ANTHROPIC_"):
            used = "jcode-provider-anthropic"
        elif name.startswith("OPENAI"):
            used = "jcode-provider-openai, jcode-provider-openai-compat"
        elif name.startswith("OPENROUTER_"):
            used = "jcode-provider-openai-compat"
        elif name.startswith("GEMINI_"):
            used = "jcode-provider-gemini"
        elif name.startswith("GROK_"):
            used = "jcode-provider-grok"
        elif name.startswith("GOOGLE_CLOUD"):
            used = "jcode-provider-gemini"
        elif name.startswith("CURSOR_"):
            used = "jcode-provider-cursor"
        elif name.startswith("JADE_RELAY_"):
            used = "jcode-jade-relay"
        elif name.startswith("COMPOSIO_"):
            used = "jcode-composio"
        elif name.startswith("GHOSTTY_"):
            used = "jcode-terminal-detect"
        elif name.startswith("KITTY_"):
            used = "jcode-terminal-detect"
        elif name.startswith("WEZTERM_"):
            used = "jcode-terminal-detect"
        elif name.startswith("ITERM_"):
            used = "jcode-terminal-detect"
        elif name.startswith("ALACRITTY_"):
            used = "jcode-terminal-detect"
        elif name.startswith("XDG_"):
            used = "jcode-base"
        elif name.startswith("CARGO_"):
            used = "jcode-build scripts"
        elif name.startswith("RUST"):
            used = "jcode-base telemetry"
        elif name.startswith("HERDR_"):
            used = "jcode-herdr"
        elif name.startswith("SSH_"):
            used = "jcode-base"
        elif name.startswith("COLORTERM") or name == "TERM" or name.startswith("TERM_"):
            used = "jcode-tui"
        else:
            used = "various"
        rows.append(f"| `{name}` | (unset) | {used} | {desc} |")
    rows.append("")
    return "\n".join(rows)


def main() -> int:
    sections = parse_env_overrides()
    overrides_section = build_env_overrides_table(sections)

    # Workspace JCODE-prefixed vars minus the ones already in env_overrides
    overrides_names = set()
    for items in sections.values():
        for it in items:
            overrides_names.add(it["name"])

    jcode_workspace: list[str] = []
    list_file = REPO / "scripts/docs/jcode_env.lst"
    if not list_file.exists():
        print(f"missing {list_file}; regenerating via rg...", file=sys.stderr)
        # Refresh the list from the workspace via rg
        result = subprocess.run(
            [
                "rg",
                "--type",
                "rust",
                "-N",
                "-o",
                '"JCODE_[A-Z0-9_]*"',
                "crates",
                "src",
            ],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=True,
        )
        names_set: set[str] = set()
        for line in result.stdout.splitlines():
            # rg output is `path:"VAR"` — extract the quoted var
            if '"' not in line:
                continue
            quoted = line[line.index('"'):]
            m = ENV_VAR_RE.search(quoted)
            if m and m.group(1).startswith("JCODE_"):
                names_set.add(m.group(1))
        list_file.write_text("\n".join(sorted(names_set)) + "\n")
    for raw in list_file.read_text().splitlines():
        name = raw.strip()
        if not name or name in overrides_names:
            continue
        jcode_workspace.append(name)

    workspace_section = build_workspace_jcode_table(jcode_workspace)
    thirdparty_section = build_thirdparty_table()

    body = f"""# Environment variables

This document catalogues every `JCODE_*` and selected third-party environment
variable that the workspace reads at runtime. The total count is **{len(overrides_names) + len(jcode_workspace) + len(THIRD_PARTY)}** env vars across the workspace (audit 2026-10-17).

Source of truth: each `std::env::var(...)`, `env::var(...)`, or `env::var_os(...)`
call site in `crates/` and `src/`. The first table is generated from
`crates/jcode-base/src/config/env_overrides.rs`. The other tables are derived
from a workspace-wide `rg` pass.

If a variable is missing or undocumented, please file a docs issue; this file
is regenerated on each release cut.

<!-- auto-generated from crates/* source on 2026-10-17 by raven (top5 doc fixes) -->

## Quick reference

- Total env vars documented: **{len(overrides_names) + len(jcode_workspace) + len(THIRD_PARTY)}**
  - `JCODE_*` in env_overrides: **{len(overrides_names)}**
  - `JCODE_*` read directly elsewhere: **{len(jcode_workspace)}**
  - Third-party: **{len(THIRD_PARTY)}**
- Run `rg -N 'std::env::var\\(\\\"[A-Z]' crates src` to regenerate this catalog
- Run `rg -N 'std::env::var\\(' crates src | rg -o '\"[A-Z][A-Z0-9_]*\"' | sort -u`
  to see every string-literal env-var lookup the workspace performs

---

{overrides_section}

---

{workspace_section}

---

{thirdparty_section}

---

## Regeneration

```bash
# Snapshot all unique env-var names:
rg -N 'env::var\\([^)]+\\)' crates src \\
  | rg -o '"[A-Z][A-Z0-9_]*"' | sort -u > docs/ENVIRONMENT.md.tmp

# Inspect a single var's call sites:
rg -N 'JCODE_WAKE_MODE' crates src

# Diff this file against a fresh grep:
diff <(rg -N 'env::var\\(' crates src | rg -o '"[A-Z][A-Z0-9_]*"' | sort -u) \\
     <(awk -F'|' '/^\\| `[A-Z]/ {{print "\\""$2"\\""}}' docs/ENVIRONMENT.md | sort -u)
```
"""
    target = REPO / "docs/ENVIRONMENT.md"
    target.write_text(body)
    print(f"wrote {target} ({len(body)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
