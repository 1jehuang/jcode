# PWSH+HERDR Input Handling Bug Triage

**Date:** 2026-09-17
**Scope:** JCODE-PWSH-HERDR combination only (JCODE-PWSH alone is fine, JCODE-ZSH-HERDR is fine)

---

## Architecture Summary

The input flow for PWSH+HERDR involves three layers:

1. **pwsh** runs inside a HERDR pane, emitting OSC 133 sequences via `Write-Host -NoNewline`
2. **HERDR** is a terminal multiplexer; jcode reports lifecycle state to it via Unix socket (`crates/jcode-herdr/`)
3. **jcode TUI** (Rust/crossterm/ratatui) runs in raw mode, reading crossterm events from the terminal

The critical path: pwsh writes OSC 133 to stdout -> HERDR's PTY -> jcode's crossterm event loop.

---

## Root Cause Analysis

### Finding 1: OSC 133 Sequence Emission Differs Between Pwsh and Zsh

**Evidence:**

- **Zsh** (`crates/jcode-shell-integration/src/templates/zsh.rs`):
  ```zsh
  printf '\033]133;A\007'   # BEL-terminated, via printf
  printf '\033]133;B\007'
  printf '\033]133;D;%d\007' $exit_code
  ```

- **Pwsh** (`crates/jcode-shell-integration/src/templates/pwsh.rs`):
  ```powershell
  Write-Host -NoNewline "`e]133;A`a"   # BEL-terminated, via Write-Host
  Write-Host -NoNewline "`e]133;B`a"
  Write-Host -NoNewline "`e]133;D;${exitCode}`a"
  ```

Both use BEL (`\x07` / `` `a ``) terminators and the same OSC 133 format. The difference is the emission method:
- `printf` writes raw bytes directly to fd 1
- `Write-Host -NoNewline` goes through PowerShell's output pipeline, which may buffer, transform, or partially flush the escape sequence

**Impact:** In a HERDR PTY, `Write-Host` buffering could cause partial OSC 133 sequences to reach the TUI. If crossterm reads a partial sequence (e.g., `\x1b]133;A` without the BEL), it may parse the trailing bytes as literal input characters, producing "random symbols."

### Finding 2: No PWSH-Specific Handling in Remote Mode

**Evidence:**
- `crates/jcode-tui/src/tui/app/remote/key_handling.rs` has zero references to `pwsh`, `PowerShell`, or shell-specific logic
- `crates/jcode-tui/src/tui/app/remote.rs` has zero references to pwsh
- `crates/jcode-tui/src/tui/app/remote/input_dispatch.rs` has zero references to pwsh
- The `!` command handler (`extract_input_shell_command` in `input.rs:85`) delegates to `build_input_shell_command` which:
  - On **non-Windows**: uses `bash -c <command>` (line 99)
  - On **Windows**: uses `cmd.exe /C <command>` (line 92)

**Impact:** The `!` input shell command always uses `cmd.exe` on Windows, regardless of whether the user's shell is pwsh. This means `!ls` runs `cmd.exe /C ls` instead of pwsh's `Get-ChildItem` or pwsh-native `ls`. The command output is captured and displayed as markdown, but the execution context is wrong.

### Finding 3: HERDR State Reporting is Shell-Agnostic (Not the Direct Cause)

**Evidence:**
- `crates/jcode-herdr/src/reporter.rs`: Reports `AgentState` (Working/Idle/Blocked) via Unix socket to HERDR
- `crates/jcode-tui/src/herdr.rs`: Wraps the reporter as a global singleton; called from `local.rs`, `turn.rs`, `ui_elicit.rs`
- HERDR state reports are fire-and-forget socket writes; they don't interact with the PTY or terminal I/O

**Impact:** HERDR state reporting is not the direct cause of input issues. The problem is in how HERDR manages the PTY and how pwsh's output flows through it.

### Finding 4: PTY Interaction Layer - The Real Collision Point

**Evidence:**
- jcode TUI runs in crossterm raw mode (captures all key events, mouse events, bracketed paste)
- `reapply_terminal_modes_to` (`crates/jcode-tui/src/tui/mod.rs:165`) reasserts: bracketed paste, focus change, mouse capture, kitty keyboard
- pwsh's PSReadLine also manages the terminal: it uses ANSI sequences for syntax highlighting, cursor movement, and input editing
- HERDR's PTY sits between them

**The collision sequence:**
1. pwsh starts, PSReadLine initializes and sets terminal modes
2. pwsh shell integration writes OSC 133 sequences via `Write-Host`
3. In HERDR's PTY, these sequences may be:
   - Buffered and delivered in chunks to jcode's crossterm reader
   - Partially corrupted by HERDR's own terminal management
   - Interleaved with HERDR's own escape sequences (pane boundaries, focus events)
4. jcode's crossterm receives malformed or partial escape sequences
5. Unrecognized bytes appear as "random input symbols" in the input bar

### Finding 5: "Input Bar Becomes New Shell Session" on Message Send

**Evidence:**
- When Enter is pressed in remote mode (`key_handling.rs:956-1000`), the flow is:
  1. `take_prepared_input(app)` captures and clears `app.input`
  2. `submit_prepared_remote_input` sends to server via `begin_remote_send`
  3. Server processes the message, streams response
- `take_prepared_input` (`input.rs:2892`) does: `std::mem::take(&mut app.input)`, sets `cursor_pos = 0`, clears undo history
- After send, `app.is_processing = true` and the TUI waits for server events

**The issue:** After the message is sent, if pwsh emits OSC 133 sequences (precmd/preexec hooks fire on the next prompt render), these leak into the input buffer. The TUI interprets them as new input, creating the appearance of a "new shell session" in the input bar.

**Why ZSH doesn't have this problem:** Zsh's `printf` writes are atomic and complete. The OSC 133 sequences are fully delivered and consumed before crossterm's event loop picks them up. pwsh's `Write-Host` buffering creates a window where partial sequences arrive during the idle tick.

### Finding 6: "Exit" Command Ambiguity

**Evidence:**
- `/quit` in remote mode (`key_handling.rs:1073`): Sets `app.should_quit = true`, closes the client TUI
- There is no `/exit` command handler in remote mode
- If the user types `exit` (without `/`), it's treated as regular input and sent to the remote server
- The server processes `exit` as a normal message to the AI agent (not as a shell command)
- In a HERDR pane, if pwsh exits, HERDR may close the pane

**Impact:** "exit" behavior depends on context:
- In jcode TUI input bar: treated as a chat message, sent to the server
- In pwsh's own shell (if user focused the shell directly): pwsh exits, HERDR closes pane
- The "sometimes closes pane, sometimes goes back to chat" behavior is a race condition between pwsh's exit handling and HERDR's pane management

### Finding 7: `!` Command Uses Wrong Shell on Windows

**Evidence:**
- `build_input_shell_command` (`input.rs:89-103`):
  ```rust
  #[cfg(windows)]
  {
      let mut cmd = Command::new("cmd.exe");
      cmd.arg("/C").arg(command);
      cmd
  }
  ```
- Server-side `build_input_shell_command` (`client_actions.rs:36-50`): Same Windows behavior
- `!pwsh_specific_command` would fail because it runs under cmd.exe, not pwsh

**Fix needed:** On Windows, detect if the user's shell is pwsh and use `pwsh.exe -Command <command>` instead of `cmd.exe /C <command>`.

---

## Summary of Root Causes

| Issue | Root Cause | Severity |
|-------|-----------|----------|
| Random input symbols | OSC 133 sequences from pwsh's `Write-Host` are partially delivered through HERDR's PTY, causing crossterm to parse trailing bytes as literal input | **HIGH** |
| Input bar becomes new shell | After message send, pwsh precmd hooks emit OSC 133 sequences that leak into the idle input buffer | **HIGH** |
| Exit command ambiguity | No `/exit` handler; "exit" is sent as chat message; pwsh-native exit closes HERDR pane | **MEDIUM** |
| `!` command uses wrong shell | Windows `build_input_shell_command` hardcodes `cmd.exe` instead of detecting pwsh | **MEDIUM** |
| Various weird input behaviors | PSReadLine + crossterm raw mode + HERDR PTY triple contention on terminal state | **HIGH** |

---

## Recommended Fixes

### Fix 1: Atomic OSC 133 Emission in Pwsh (HIGH PRIORITY)

Replace `Write-Host -NoNewline` with `[Console]::Write()` or `[System.Console]::Write()` in the pwsh templates to bypass PowerShell's output pipeline:

```powershell
# Before (buffered through Write-Host):
Write-Host -NoNewline "`e]133;A`a"

# After (direct console write, atomic):
[Console]::Write("`e]133;A`a")
```

**File:** `crates/jcode-shell-integration/src/templates/pwsh.rs` (constants `PWSH_PRECMD`, `PWSH_PREEXEC`, `PWSH_INIT`)
**File:** `crates/jcode-shell-integration/src/hooks.rs` (function `pwsh_hooks`)

### Fix 2: Shell-Aware `!` Command on Windows (MEDIUM PRIORITY)

Detect the current shell and use the appropriate interpreter:

```rust
#[cfg(windows)]
{
    let shell = crate::shell_integration::detect::Shell::detect();
    match shell {
        Shell::PowerShell => {
            let mut cmd = Command::new("pwsh.exe");
            cmd.arg("-NoProfile").arg("-Command").arg(command);
            cmd
        }
        _ => {
            let mut cmd = Command::new("cmd.exe");
            cmd.arg("/C").arg(command);
            cmd
        }
    }
}
```

**File:** `crates/jcode-tui/src/tui/app/input.rs` (function `build_input_shell_command`)
**File:** `crates/jcode-app-core/src/server/client_actions.rs` (function `build_input_shell_command`)

### Fix 3: Consume leaked OSC sequences in the TUI event loop (HIGH PRIORITY)

Add an OSC 133 sequence filter in the crossterm event processing path. When an unrecognized sequence starting with `\x1b]133;` is detected, consume it silently rather than passing it through as input.

**File:** `crates/jcode-tui/src/tui/app/remote/key_handling.rs` (function `handle_remote_key_event` or the event polling loop)

### Fix 4: Add `/exit` as alias for `/quit` in remote mode (LOW PRIORITY)

```rust
if trimmed == "/quit" || trimmed == "/exit" {
    // ... same quit logic
}
```

**File:** `crates/jcode-tui/src/tui/app/remote/key_handling.rs` (around line 1073)

### Fix 5: Clear input buffer after send in HERDR+PWSH mode (MEDIUM PRIORITY)

After `begin_remote_send`, if running in HERDR+PWSH, drain any pending bytes from the PTY to prevent stale OSC sequences from entering the input buffer.

**File:** `crates/jcode-tui/src/tui/app/remote/input_dispatch.rs` (function `begin_remote_send`)

---

## Testing Notes

- JCODE-PWSH alone works: confirms the shell integration templates are correct for pwsh
- JCODE-ZSH-HERDR works: confirms HERDR integration is correct for well-behaved shells
- The bug is specifically at the intersection: pwsh's output behavior + HERDR's PTY management

**Reproduction steps (estimated):**
1. Launch HERDR on Windows
2. In a HERDR pane, start jcode with pwsh as the shell
3. Type a message and press Enter
4. Observe: input bar may show random characters after send
5. Type `!dir` to test the `!` command (runs cmd.exe, not pwsh)
6. Type `exit` to observe ambiguous behavior
