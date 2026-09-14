//! Escape-sequence helpers for modern terminal protocols.
//!
//! All functions in this module return raw strings that can be written
//! directly to the terminal's stdout. No formatting or allocation is
//! done unless the sequence requires variable data (in which case a
//! [`String`] is returned).

/// Synchronized update **begin**.  Wrap bulk output in begin/end pairs
/// to prevent tearing on terminals that support DECSET 2026.
pub fn sync_begin() -> &'static str {
    "\x1b[?2026h"
}

/// Synchronized update **end**.
pub fn sync_end() -> &'static str {
    "\x1b[?2026l"
}

// ---------------------------------------------------------------------------
// OSC 133 – FTCS semantic prompt markers
// ---------------------------------------------------------------------------

/// OSC 133 ; A – **prompt start** (before the prompt text).
pub fn prompt_start() -> &'static str {
    "\x1b]133;A\x07"
}

/// OSC 133 ; B – **command start** (after the prompt, user is typing).
pub fn command_start() -> &'static str {
    "\x1b]133;B\x07"
}

/// OSC 133 ; C – **command executed** (command has begun running).
pub fn command_executed() -> &'static str {
    "\x1b]133;C\x07"
}

/// OSC 133 ; D – **command finished** with exit code.
pub fn command_finished(exit_code: i32) -> String {
    format!("\x1b]133;D;{exit_code}\x07")
}

/// OSC 133 ; P – set the command's **working directory**.
pub fn prompt_cwd(path: &str) -> String {
    format!("\x1b]133;P;cwd={path}\x07")
}

// ---------------------------------------------------------------------------
// OSC 7 – CWD reporting (file:// URL)
// ---------------------------------------------------------------------------

/// OSC 7 – report the current working directory as a `file://` URL.
///
/// `host` should be the hostname (or empty for local).  `path` is the
/// absolute directory path.
pub fn cwd_report(host: &str, path: &str) -> String {
    format!("\x1b]7;file://{host}{path}\x07")
}

// ---------------------------------------------------------------------------
// OSC 8 – clickable hyperlinks
// ---------------------------------------------------------------------------

/// Start an OSC 8 hyperlink targeting `url`.
pub fn hyperlink_start(url: &str) -> String {
    format!("\x1b]8;;{url}\x07")
}

/// End an OSC 8 hyperlink.
pub fn hyperlink_end() -> &'static str {
    "\x1b]8;;\x07"
}

// ---------------------------------------------------------------------------
// OSC 52 – clipboard
// ---------------------------------------------------------------------------

/// OSC 52 – write `base64_data` to the system clipboard.
///
/// The clipboard selection is `c` (system/clipboard).  For primary use
/// `p`.
pub fn clipboard_write_b64(base64_data: &str) -> String {
    format!("\x1b]52;c;{base64_data}\x07")
}

/// OSC 52 – request clipboard contents (terminal will respond on stdin).
pub fn clipboard_read() -> &'static str {
    "\x1b]52;c;?\x07"
}

// ---------------------------------------------------------------------------
// OSC 2 / OSC 0 – window & tab titles
// ---------------------------------------------------------------------------

/// Set the window title via OSC 2.
pub fn set_title(title: &str) -> String {
    format!("\x1b]2;{title}\x07")
}

/// Set the icon name / tab title via OSC 0.
pub fn set_icon_name(name: &str) -> String {
    format!("\x1b]0;{name}\x07")
}

// ---------------------------------------------------------------------------
// Kitty keyboard protocol
// ---------------------------------------------------------------------------

/// Enable the Kitty keyboard protocol with all standard flags
/// (disambiguate, report events, report alternate keys, report all keys,
/// report text).
pub fn kitty_keyboard_enable() -> &'static str {
    "\x1b[>31u"
}

/// Disable the Kitty keyboard protocol (restore xterm defaults).
pub fn kitty_keyboard_disable() -> &'static str {
    "\x1b[<u"
}

/// Push current keyboard protocol state onto the stack.
pub fn kitty_keyboard_push() -> &'static str {
    "\x1b[>u"
}

/// Pop keyboard protocol state from the stack.
pub fn kitty_keyboard_pop() -> &'static str {
    "\x1b[<u"
}

// ---------------------------------------------------------------------------
// DECSET/DECRST – mode toggles
// ---------------------------------------------------------------------------

/// Enable **synchronized output** (DECSET 2026).
pub fn mode_synchronized_on() -> &'static str {
    "\x1b[?2026h"
}

/// Disable **synchronized output** (DECRST 2026).
pub fn mode_synchronized_off() -> &'static str {
    "\x1b[?2026l"
}

/// Enable **bracketed paste** (DECSET 2004).
pub fn mode_bracketed_paste_on() -> &'static str {
    "\x1b[?2004h"
}

/// Disable **bracketed paste** (DECRST 2004).
pub fn mode_bracketed_paste_off() -> &'static str {
    "\x1b[?2004l"
}

/// Enable **focus tracking** (DECSET 1004).
pub fn mode_focus_tracking_on() -> &'static str {
    "\x1b[?1004h"
}

/// Disable **focus tracking** (DECRST 1004).
pub fn mode_focus_tracking_off() -> &'static str {
    "\x1b[?1004l"
}

/// Enable **SGR mouse mode** (DECSET 1006).
pub fn mode_sgr_mouse_on() -> &'static str {
    "\x1b[?1006h"
}

/// Disable **SGR mouse mode** (DECRST 1006).
pub fn mode_sgr_mouse_off() -> &'static str {
    "\x1b[?1006l"
}

// ---------------------------------------------------------------------------
// Light / dark mode (DECSET 997)
// ---------------------------------------------------------------------------

/// Query light/dark mode.
pub fn light_dark_query() -> &'static str {
    "\x1b[?997;|$?\x1b\\"
}

/// Report that the application is using a **light** color scheme.
pub fn report_light_mode() -> &'static str {
    "\x1b[997;1l"
}

/// Report that the application is using a **dark** color scheme.
pub fn report_dark_mode() -> &'static str {
    "\x1b[997;0l"
}

// ---------------------------------------------------------------------------
// Kitty graphics protocol – transmit + display
// ---------------------------------------------------------------------------

/// Transmit a base64-encoded image payload via the Kitty graphics
/// protocol.  `format` is `f=32` (PNG), `f=24` (raw RGBA), etc.
///
/// Returns the full DCS sequence including display.
pub fn kitty_graphics_transmit_display(b64_data: &str, format: u8) -> String {
    // a=T transmit + display, f=<format>, s=<width>, v=<height>,
    // t=base64 length, then the data.
    format!(
        "\x1b_Ga=T,f={format},t={len};{b64_data}\x1b\\",
        len = b64_data.len(),
    )
}

/// Clear the current Kitty graphics image.
pub fn kitty_graphics_clear() -> &'static str {
    "\x1b_Ga=d\x1b\\"
}

// ---------------------------------------------------------------------------
// Sixel
// ---------------------------------------------------------------------------

/// Wrap raw sixel data for display.  The caller provides the sixel
/// payload (everything between `DCS` and `ST`).
pub fn sixel_display(sixel_payload: &str) -> String {
    format!("\x1bP{sixel_payload}\x1b\\")
}

// ---------------------------------------------------------------------------
// Cursor shape (DECSCUSR)
// ---------------------------------------------------------------------------

/// Set the cursor style.  `style` values:
/// 1 = blinking block, 2 = steady block, 3 = blinking underline,
/// 4 = steady underline, 5 = blinking bar, 6 = steady bar.
pub fn cursor_shape(style: u8) -> String {
    format!("\x1b[{style} q")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_wrapping() {
        assert_eq!(sync_begin(), "\x1b[?2026h");
        assert_eq!(sync_end(), "\x1b[?2026l");
    }

    #[test]
    fn osc133_markers() {
        assert_eq!(prompt_start(), "\x1b]133;A\x07");
        assert_eq!(command_start(), "\x1b]133;B\x07");
        assert_eq!(command_executed(), "\x1b]133;C\x07");
        assert_eq!(command_finished(0), "\x1b]133;D;0\x07");
        assert_eq!(command_finished(1), "\x1b]133;D;1\x07");
    }

    #[test]
    fn osc8_hyperlinks() {
        let start = hyperlink_start("https://example.com");
        assert_eq!(start, "\x1b]8;;https://example.com\x07");
        assert_eq!(hyperlink_end(), "\x1b]8;;\x07");
    }

    #[test]
    fn osc52_clipboard() {
        let write = clipboard_write_b64("aGVsbG8=");
        assert_eq!(write, "\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(clipboard_read(), "\x1b]52;c;?\x07");
    }

    #[test]
    fn window_title() {
        assert_eq!(set_title("jcode"), "\x1b]2;jcode\x07");
        assert_eq!(set_icon_name("jcode"), "\x1b]0;jcode\x07");
    }

    #[test]
    fn kitty_keyboard() {
        assert_eq!(kitty_keyboard_enable(), "\x1b[>31u");
        assert_eq!(kitty_keyboard_disable(), "\x1b[<u");
    }

    #[test]
    fn mode_toggles() {
        assert_eq!(mode_bracketed_paste_on(), "\x1b[?2004h");
        assert_eq!(mode_focus_tracking_on(), "\x1b[?1004h");
        assert_eq!(mode_sgr_mouse_on(), "\x1b[?1006h");
    }

    #[test]
    fn cursor_shape_styles() {
        assert_eq!(cursor_shape(1), "\x1b[1 q");
        assert_eq!(cursor_shape(6), "\x1b[6 q");
    }

    #[test]
    fn cwd_report_format() {
        let report = cwd_report("myhost", "/home/user/project");
        assert!(report.starts_with("\x1b]7;file://"));
        assert!(report.ends_with("\x07"));
        assert!(report.contains("myhost"));
        assert!(report.contains("/home/user/project"));
    }

    #[test]
    fn kitty_graphics_transmit() {
        let seq = kitty_graphics_transmit_display("AAAA", 32);
        assert!(seq.contains("a=T,f=32,t=4"));
        assert!(seq.contains("AAAA"));
    }
}
