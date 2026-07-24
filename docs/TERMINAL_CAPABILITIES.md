# Terminal Emulator Capabilities for TUI Rendering

> Compiled 2026-03-02. Reflects latest stable releases of each terminal.
> "Yes*" means supported with caveats (see notes). "No" means not supported as of latest release.

## Capability Matrix

| Terminal | Truecolor (24-bit) | 256-color | Unicode/Emoji | Kitty Keyboard Protocol | Bracketed Paste | Mouse Capture | Alt Screen | Notable Quirks |
|---|---|---|---|---|---|---|---|---|
| **macOS Terminal.app** | No (until macOS Tahoe/26) | Yes | Partial (emoji widths wrong, no ligatures) | No | Yes | Yes (basic SGR) | Yes | No truecolor - RGB silently clamped to 256. Emoji often render 1-cell wide instead of 2. TERM=xterm-256color only. |
| **iTerm2** | Yes | Yes | Full (excellent emoji, ligatures) | Yes (3.5+) | Yes | Yes (SGR 1006) | Yes | Slight input latency on complex scenes. Proprietary inline image protocol. Occasionally misreports TERM_PROGRAM version to apps. |
| **Ghostty** | Yes | Yes | Full (grapheme clustering, good emoji) | Yes | Yes | Yes (SGR 1006) | Yes | Very new - occasional edge cases with rare combining sequences. GPU-rendered, minimal legacy quirks. |
| **Handterm** | Yes | Yes | Full (good emoji/Nerd Font handling) | Partial | Yes | Yes (SGR 1006) | Yes | Experimental Wayland-native GPU terminal. Smooth pixel-scroll behavior is terminal-native in its GPU path. Still evolving; some CLI/launcher integrations may lag behind established terminals. |
| **Kitty** | Yes | Yes | Full (grapheme clustering, emoji) | Yes (originator) | Yes | Yes (SGR 1006) | Yes | Strict spec compliance can break apps expecting xterm quirks. Does NOT set TERM=xterm-*; uses xterm-kitty. `ssh` may need terminfo transfer. |
| **Alacritty** | Yes | Yes | Full (good emoji support) | Yes (0.13+) | Yes | Yes (SGR 1006) | Yes | No tabs/splits (by design). No scrollback mouse-scroll passthrough to apps without config. No ligature support. |
| **WezTerm** | Yes | Yes | Full (ligatures, emoji, Nerd Fonts) | Yes | Yes | Yes (SGR 1006) | Yes | Lua config can cause startup delays. Multiplexer mode has rare sync artifacts. Very feature-complete. |
| **Warp** | Yes | Yes | Full (emoji, ligatures) | Yes* (partial, evolving) | Yes* (Warp intercepts paste for its own UI) | Yes* (limited - Warp's block model intercepts raw mouse) | Yes* (Warp overrides alt-screen for its own rendering) | Warp's non-traditional architecture (blocks, AI input) intercepts many escape sequences. TUI apps may render incorrectly because Warp interposes its own shell integration layer. |
| **Windows Terminal** | Yes | Yes | Full (emoji, CJK, good font fallback) | No | Yes | Yes (SGR 1006) | Yes | ConPTY layer can add latency and occasionally drops rapid escape sequences. Background color can bleed 1 cell on resize. Bold = bright color mapping surprises some apps. |
| **VS Code Terminal** | Yes | Yes | Full (inherits VS Code's font rendering) | Yes (xterm.js 5.x+) | Yes | Yes (SGR 1006) | Yes | xterm.js backend: slightly slower than native terminals. Canvas renderer can leave stale cells on rapid redraws. Emoji width depends on editor font. Extension host restarts can kill the PTY. |
| **GNOME Terminal (VTE)** | Yes | Yes | Full (system font emoji, no ligatures) | No | Yes | Yes (SGR 1006) | Yes | VTE rewrites COLORTERM=truecolor. Historically slow with large scrollback. Underline color/style support lagged. No ligatures (VTE limitation). |
| **Konsole** | Yes | Yes | Full (emoji, Nerd Fonts, ligatures) | No* (partial, basic CSI u only) | Yes | Yes (SGR 1006) | Yes | Reflow on resize can cause momentary display corruption. Older versions had SGR background bleed on line wrap. Generally very solid. |
| **tmux** | Yes* (needs `set -g default-terminal "tmux-256color"` + `set -as terminal-features ',*:RGB'`) | Yes | Partial (passes through but wcwidth mismatches with outer terminal) | No (strips kitty keyboard sequences) | Yes (passthrough) | Yes (passthrough) | Yes (own alt-screen layer) | **Major source of rendering bugs.** Interposes its own terminal emulation layer. Strips unknown escapes by default. Truecolor requires explicit config. `passthrough` DCS escape needed for some protocols. Double-width chars can desync between tmux's internal state and the outer terminal. |
| **screen** | No (256-color max without patches) | Yes | Partial (limited multi-byte, no emoji) | No | Yes* (recent versions only) | Yes* (basic, older protocol) | Yes (own alt-screen layer) | **Most limited multiplexer.** No truecolor. Ancient codebase with minimal Unicode support - CJK/emoji characters frequently render as wrong width or garble the line. Escape sequence filtering is aggressive. Largely superseded by tmux. |

## Legend

- **Truecolor**: Supports `\e[38;2;R;G;Bm` / `\e[48;2;R;G;Bm` SGR sequences for 16M colors
- **256-color**: Supports `\e[38;5;Nm` / `\e[48;5;Nm` indexed color
- **Unicode/Emoji**: Full = correct grapheme clustering, proper double-width, emoji ZWJ sequences; Partial = basic multi-byte but broken widths or missing sequences
- **Kitty Keyboard Protocol**: Supports `CSI > flags u` progressive enhancement keyboard protocol
- **Bracketed Paste**: Supports `\e[?2004h` to wrap pasted content in begin/end markers
- **Mouse Capture**: Supports SGR 1006 mouse reporting (`\e[?1006h`)
- **Alt Screen**: Supports `\e[?1049h` alternate screen buffer

---

## Known Rendering Issues That Cause White Blocks or Stale Content

### 1. Background Color Bleeding / "White Blocks"

**Root cause**: When a TUI sets a background color on a cell but the terminal fails to clear or repaint that cell correctly on the next frame, the cell retains its old content or falls back to the default background (often white on light themes).

**Affected terminals and scenarios:**

- **All terminals**: If the app writes `\e[K` (erase to end of line) without first setting the correct background color via SGR, the erased region inherits the terminal's default background, not the app's intended color. This is the #1 cause of white/light blocks in dark-themed TUIs.

- **tmux**: tmux emulates its own screen buffer. If the inner app uses BCE (Background Color Erase) and tmux's `default-terminal` doesn't advertise BCE support correctly, erased regions render with the wrong background. Fix: ensure `tmux-256color` terminfo is used and matches the outer terminal's capabilities.

- **Windows Terminal (ConPTY)**: ConPTY sometimes coalesces rapid SGR+erase sequences incorrectly, causing 1-2 cells at line boundaries to retain old background colors after a resize or rapid redraw.

- **VS Code Terminal (xterm.js)**: The canvas-based renderer can leave "ghost" cells when the terminal rapidly alternates between normal and alternate screen buffers, especially during resize events.

### 2. Emoji / Double-Width Character Misalignment

**Root cause**: The terminal and the application disagree on how many columns a character occupies. The app thinks an emoji is 2 columns wide (per Unicode `East_Asian_Width`), but the terminal renders it as 1 (or vice versa), causing every subsequent cell on that line to be shifted.

**Affected terminals:**

- **macOS Terminal.app**: Particularly bad. Many emoji render at 1-cell width while apps (using libc `wcwidth` or Unicode tables) assume 2. This desynchronizes the entire line, leaving "phantom" cells that appear as blank/white blocks.

- **tmux**: tmux has its own internal `wcwidth` implementation. If it disagrees with the outer terminal about a character's width (common with newer emoji added in recent Unicode versions), cursor positioning breaks and cells appear duplicated or blank.

- **screen**: Even worse than tmux. Its Unicode width tables are years out of date. Most emoji and many CJK characters will corrupt line layout.

- **Alacritty**: Generally good, but Nerd Font glyphs that are PUA (Private Use Area) codepoints default to 1-cell width. If the app assumes 2, misalignment occurs.

### 3. Stale Content After Resize

**Root cause**: When the terminal window is resized, the app receives `SIGWINCH` and must redraw. If the redraw is partial or the terminal's line reflow logic conflicts with the app's assumptions, old content remains visible.

**Affected terminals:**

- **Konsole**: Reflow on resize is aggressive - it reflows soft-wrapped lines, which can conflict with TUI apps that expect each line to be independent. This causes momentary "double rendering" artifacts.

- **tmux**: Resize causes tmux to reflow its own buffer and then relay `SIGWINCH` to the inner app. There's a race condition: if the app redraws before tmux finishes reflowing, old content appears for 1-2 frames.

- **VS Code Terminal**: The xterm.js resize handler can lag behind the actual viewport size, causing the app to draw for the wrong dimensions for 1-2 frames.

### 4. Alternate Screen Buffer Transition Artifacts

**Root cause**: When entering or leaving the alternate screen (`\e[?1049h` / `\e[?1049l`), some terminals don't fully clear the buffer, or they restore the wrong saved state.

**Affected scenarios:**

- **Warp**: Warp's block-based architecture doesn't use a traditional alternate screen. TUI apps that rely on `\e[?1049h` may find their output mixed with Warp's shell integration UI elements.

- **tmux + nested sessions**: Nested tmux sessions (or tmux inside screen) can lose track of which alternate screen buffer is active, leaving the outer multiplexer's status bar overlaid on the inner app's content.

- **macOS Terminal.app**: On older macOS versions (pre-Ventura), restoring from alternate screen occasionally leaves the cursor invisible until the user types.

### 5. Cursor Visibility Issues

**Root cause**: `\e[?25l` (hide cursor) and `\e[?25h` (show cursor) aren't always reliably paired, especially when apps crash or are killed with SIGKILL.

**Affected terminals:**

- **All terminals**: If a TUI app crashes without restoring the cursor, it stays hidden. Most modern terminals (kitty, WezTerm, iTerm2) auto-restore on shell prompt, but GNOME Terminal and Terminal.app may leave cursor hidden until `reset` or `tput cnorm`.

- **tmux**: If the inner pane's app hides the cursor and then the user switches panes, the cursor visibility state can leak between panes (fixed in newer tmux versions but still observed in 3.3 and earlier).

### 6. SGR Reset Scope Issues

**Root cause**: `\e[0m` (SGR reset) should reset all attributes, but some terminals handle it inconsistently with respect to underline style, underline color, or strikethrough.

- **GNOME Terminal (VTE)**: Older VTE versions didn't reset underline color on SGR 0, causing colored underlines to persist across lines.
- **Konsole**: Historical bug where `\e[0m` didn't reset the overline attribute.
- **screen**: `\e[0m` doesn't reliably reset 256-color foreground/background, leaving stale colors on subsequent text.

### 7. Kitty Keyboard Protocol Fallback Issues

**Root cause**: Apps that enable the kitty keyboard protocol but don't properly disable it on exit (or crash) leave the terminal in an enhanced keyboard mode. Subsequent shell input may produce garbled escape sequences.

- **Kitty, Alacritty, WezTerm, Ghostty**: All affected if the app doesn't call `CSI < u` on exit. Kitty itself auto-resets on shell prompt detection. Alacritty and WezTerm do not auto-reset - the user must run `reset`.

### 8. tmux-Specific Passthrough Limitations

tmux is the most common source of rendering issues in TUI apps because it interposes a full VT100 emulation layer:

- **Escape sequence filtering**: tmux strips any escape sequences it doesn't recognize. This breaks kitty keyboard protocol, kitty graphics protocol, iTerm2 inline images, and some extended SGR attributes (e.g., `CSI 4:3 m` curly underline requires tmux 3.4+).
- **Delayed passthrough**: Even with `set -g allow-passthrough on`, DCS passthrough adds latency and can fragment long sequences.
- **TERM mismatch**: If the inner `TERM` doesn't match tmux's advertised capabilities (e.g., app sees `xterm-256color` but tmux only passes `screen-256color`), color/capability negotiation fails silently.
- **Clipboard**: OSC 52 clipboard support works but must be explicitly enabled (`set -g set-clipboard on`).

---

## Recommendations for TUI Developers

1. **Always set BGC before erasing**: Before any `\e[K`, `\e[J`, or `\e[2J`, set the intended background color via SGR. Never assume the terminal's default background matches your theme.

2. **Use `COLORTERM` for truecolor detection**: Check `COLORTERM=truecolor` or `COLORTERM=24bit` rather than parsing terminfo, which is unreliable for RGB support.

3. **Handle emoji width defensively**: Use Unicode 15.1+ width tables and accept that some terminals will disagree. Consider avoiding emoji in grid-aligned TUI layouts, or pad with explicit spaces.

4. **Full redraw on SIGWINCH**: Don't try to incrementally patch the screen on resize. Clear everything and redraw from scratch.

5. **Always restore terminal state on exit**: Use a cleanup handler (even for SIGTERM/SIGINT) that: restores cursor visibility, leaves alternate screen, disables mouse capture, disables bracketed paste, resets kitty keyboard protocol, and issues SGR reset.

6. **Test under tmux**: If your users might run inside tmux, test there explicitly. Many rendering bugs only appear under a multiplexer.

7. **Degrade gracefully for Terminal.app and screen**: These are the lowest-capability terminals still in common use. Detect them (via `TERM_PROGRAM` or `TERM`) and fall back to 256-color mode with ASCII-safe UI elements.
