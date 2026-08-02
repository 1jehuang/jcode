//! Render and install global "launch a new jcode" hotkeys on Linux/niri.
//!
//! Unlike macOS, a Wayland client cannot grab system-wide hotkeys: the
//! `global-hotkey` crate only works on X11/macOS. The portable, correct
//! mechanism on Wayland is to ask the **compositor** to bind the key, so on niri
//! we generate `bind` lines and splice them into the user's
//! `~/.config/niri/config.kdl` `binds { }` block. niri watches its config and
//! hot-reloads on save, so the bindings take effect without a restart.
//!
//! Two layers, mirroring the macOS module:
//!
//! * The **pure** renderer ([`render_niri_block`], [`chord_to_niri_bind`]) turns
//!   resolved launch hotkeys into the exact KDL text we manage. This is what the
//!   unit tests assert, so the bindings the user sees are exactly what we can
//!   check without touching their machine.
//! * The **install** glue ([`splice_managed_block`]) replaces our marked region
//!   inside the existing `binds { }` block (or inserts one), leaving every other
//!   line untouched.
//!
//! The managed region is delimited by sentinel comments so re-installs are
//! idempotent and a user can hand-remove it cleanly:
//!
//! ```text
//!     // >>> jcode launch hotkeys (managed) >>>
//!     Alt+Semicolon hotkey-overlay-title="jcode: home" { spawn "sh" "-c" "..."; }
//!     // <<< jcode launch hotkeys (managed) <<<
//! ```

use crate::keymap::KeyChord;

/// Opening sentinel for the managed bind region inside `binds { }`.
pub(crate) const NIRI_BLOCK_BEGIN: &str = "// >>> jcode launch hotkeys (managed) >>>";
/// Closing sentinel for the managed bind region inside `binds { }`.
pub(crate) const NIRI_BLOCK_END: &str = "// <<< jcode launch hotkeys (managed) <<<";

/// One resolved hotkey ready to render as a niri bind: the chord, the target
/// directory, a human label, and whether it is a self-dev session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NiriHotkey {
    pub chord: KeyChord,
    /// Concrete directory to launch jcode in (already resolved from any
    /// `$HOME`/`$LAST_DIR` sentinel).
    pub dir: String,
    /// Short human label, e.g. the repo's directory name.
    pub label: String,
    /// Pass the `self-dev` subcommand.
    pub self_dev: bool,
}

/// Map a jcode modifier+key chord onto niri's KDL key syntax.
///
/// niri uses `+`-joined modifiers followed by an XKB key name, e.g.
/// `Alt+Semicolon`, `Super+Shift+Apostrophe`. We translate jcode's `cmd`
/// modifier to `Super` (the Wayland super/meta key) since there is no Command
/// key on Linux. Returns `None` for keys niri cannot name.
pub(crate) fn chord_to_niri_bind(chord: &KeyChord) -> Option<String> {
    let key = niri_key_name(&chord.key)?;
    let mut parts: Vec<&str> = Vec::new();
    // jcode `cmd` == macOS Command == Wayland Super.
    if chord.cmd {
        parts.push("Super");
    }
    if chord.ctrl {
        parts.push("Ctrl");
    }
    if chord.alt {
        parts.push("Alt");
    }
    if chord.shift {
        parts.push("Shift");
    }
    let mods = parts.join("+");
    if mods.is_empty() {
        Some(key)
    } else {
        Some(format!("{mods}+{key}"))
    }
}

/// Translate a canonical jcode key token into the XKB key name niri expects.
/// Returns `None` for tokens with no stable niri spelling.
fn niri_key_name(key: &str) -> Option<String> {
    let named = match key {
        ";" => "Semicolon",
        "'" => "Apostrophe",
        "[" => "bracketleft",
        "]" => "bracketright",
        "\\" => "backslash",
        "/" => "slash",
        "," => "comma",
        "." => "period",
        "-" => "minus",
        "=" => "equal",
        "`" => "grave",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "pageup" => "Page_Up",
        "pagedown" => "Page_Down",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        "delete" => "Delete",
        "backspace" => "BackSpace",
        "enter" => "Return",
        "esc" => "Escape",
        "tab" => "Tab",
        "space" => "space",
        other => {
            // Single letters: niri accepts the lowercase XKB name (`a`..`z`).
            if other.len() == 1 && other.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(other.to_string());
            }
            // Function keys f1..f24 -> F1..F24.
            if let Some(rest) = other.strip_prefix('f')
                && !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_digit())
            {
                return Some(format!("F{rest}"));
            }
            return None;
        }
    };
    Some(named.to_string())
}

/// Escape a string for inclusion inside a KDL double-quoted string.
fn kdl_escape(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

/// POSIX-shell single-quote escaping for an argument passed via `sh -c`.
fn sh_single_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

/// Build the `sh -c` command string a bind runs to open jcode in `dir`.
///
/// We `cd` into the directory (falling back to `$HOME` if it has since been
/// removed), then launch jcode via the user's terminal. The terminal is chosen
/// by `terminal` (e.g. `kitty`); we pass it the jcode executable directly.
fn launch_shell_command(
    exe_path: &str,
    terminal: &str,
    dir: &str,
    chord: &str,
    self_dev: bool,
) -> String {
    let dir_q = sh_single_quote(dir);
    let exe_q = sh_single_quote(exe_path);
    let term_q = sh_single_quote(terminal);
    let chord_q = sh_single_quote(chord);
    let subcmd = if self_dev { " self-dev" } else { "" };
    // cd with $HOME fallback, then exec the terminal running jcode.
    format!(
        "if [ -d {dir_q} ]; then cd {dir_q}; else cd \"$HOME\"; fi; exec {term_q} {exe_q} --spawn-hotkey {chord_q}{subcmd}",
        dir_q = dir_q,
        term_q = term_q,
        exe_q = exe_q,
        chord_q = chord_q,
        subcmd = subcmd,
    )
}

/// Render a single niri `bind` line for one hotkey, or `None` if the chord
/// cannot be expressed in niri.
pub(crate) fn render_niri_bind_line(
    hotkey: &NiriHotkey,
    exe_path: &str,
    terminal: &str,
    indent: &str,
) -> Option<String> {
    let bind = chord_to_niri_bind(&hotkey.chord)?;
    let title = if hotkey.self_dev {
        format!("jcode: {} (self-dev)", hotkey.label)
    } else {
        format!("jcode: {}", hotkey.label)
    };
    let shell = launch_shell_command(
        exe_path,
        terminal,
        &hotkey.dir,
        &hotkey.chord.canonical(),
        hotkey.self_dev,
    );
    Some(format!(
        "{indent}{bind} hotkey-overlay-title=\"{title}\" {{ spawn \"sh\" \"-c\" \"{shell}\"; }}",
        indent = indent,
        bind = bind,
        title = kdl_escape(&title),
        shell = kdl_escape(&shell),
    ))
}

/// Render the full managed block (sentinels + one bind per hotkey), indented to
/// sit inside `binds { }`. Hotkeys niri cannot express are skipped. Returns
/// `None` when no hotkey could be rendered.
pub(crate) fn render_niri_block(
    hotkeys: &[NiriHotkey],
    exe_path: &str,
    terminal: &str,
    indent: &str,
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for hk in hotkeys {
        if let Some(line) = render_niri_bind_line(hk, exe_path, terminal, indent) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str(indent);
    out.push_str(NIRI_BLOCK_BEGIN);
    out.push('\n');
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(indent);
    out.push_str(NIRI_BLOCK_END);
    Some(out)
}

/// Result of splicing the managed block into a config: the new text plus whether
/// anything actually changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpliceResult {
    pub text: String,
    pub changed: bool,
}

/// Splice `block` (a fully-rendered managed region, no trailing newline) into
/// `config`'s `binds { }` section.
///
/// Behavior:
/// - If a previous managed region (between the sentinels) exists, replace it in
///   place. This keeps re-installs idempotent and position-stable.
/// - Otherwise, insert the block just inside the opening `binds {` line.
/// - If there is no `binds {` block at all, append a fresh `binds { ... }` at
///   the end of the file.
///
/// Returns `changed = false` (and the original text) when the existing managed
/// region already equals `block`, so callers can skip a no-op write.
pub(crate) fn splice_managed_block(config: &str, block: &str) -> SpliceResult {
    // 1) Replace an existing managed region if present.
    if let (Some(begin_idx), Some(end_line_end)) = find_managed_region(config) {
        // A managed region written by an older version may sit inside a nested
        // `binds` node (see #719). Relocating it is the only way to recover:
        // strip it here and fall through to the normal top-level insert.
        if brace_depth_before(config, begin_idx) > 1 {
            let mut without = String::with_capacity(config.len());
            without.push_str(&config[..begin_idx]);
            without.push_str(&config[end_line_end..]);
            let relocated = splice_managed_block(&without, block);
            return SpliceResult {
                text: relocated.text,
                changed: true,
            };
        }
        // begin_idx is the byte offset of the start of the BEGIN line; the
        // managed region runs through the end of the END line (including its
        // trailing newline). Re-emit `block` plus that newline so the result is
        // byte-identical to a fresh insert (keeps re-installs idempotent).
        let before = &config[..begin_idx];
        let after = &config[end_line_end..];
        let new_text = format!("{before}{block}\n{after}");
        let changed = new_text != config;
        return SpliceResult {
            text: new_text,
            changed,
        };
    }

    // 2) Insert just inside an existing `binds {` block.
    if let Some(insert_at) = binds_block_insert_point(config) {
        let before = &config[..insert_at];
        let after = &config[insert_at..];
        // Terminate the block with a newline so the END sentinel never runs into
        // the following bind line (which would later swallow it on replace).
        let new_text = format!("{before}{block}\n{after}");
        return SpliceResult {
            text: new_text,
            changed: true,
        };
    }

    // 3) No binds block: append a new one.
    let mut new_text = config.to_string();
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    new_text.push_str("\nbinds {\n");
    new_text.push_str(block);
    new_text.push('\n');
    new_text.push_str("}\n");
    SpliceResult {
        text: new_text,
        changed: true,
    }
}

/// Find the byte range of an existing managed region: `(start_of_BEGIN_line,
/// end_of_END_line_including_newline)`. Returns `None` if either sentinel is
/// missing.
fn find_managed_region(config: &str) -> (Option<usize>, Option<usize>) {
    let Some(begin_pos) = config.find(NIRI_BLOCK_BEGIN) else {
        return (None, None);
    };
    // Back up to the start of the BEGIN line (include its indentation).
    let line_start = config[..begin_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);

    let Some(end_pos) = config[begin_pos..].find(NIRI_BLOCK_END) else {
        return (Some(line_start), None);
    };
    let end_abs = begin_pos + end_pos;
    // Extend through the rest of the END line, including its trailing newline.
    let line_end = match config[end_abs..].find('\n') {
        Some(nl) => end_abs + nl + 1,
        None => config.len(),
    };
    (Some(line_start), Some(line_end))
}

/// Byte offset just after the first `binds {` opening line's newline, i.e. the
/// point to insert new binds so they land inside the block. Returns `None` if no
/// `binds {` block exists.
fn binds_block_insert_point(config: &str) -> Option<usize> {
    for (idx, line) in line_offsets(config) {
        // Only a *top-level* `binds { }` node holds global launch bindings.
        // A nested one (e.g. `recent-windows { binds { ... } }`) accepts a
        // different, much smaller action set, and niri rejects the whole
        // config when a `spawn` action lands there (see #719).
        if brace_depth_before(config, idx) != 0 {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("binds") && trimmed.trim_end().ends_with('{') {
            // Insert right after this line's terminating newline.
            let after_line = idx + line.len();
            // line includes no newline; advance past it if present.
            return Some(if config[after_line..].starts_with('\n') {
                after_line + 1
            } else {
                after_line
            });
        }
    }
    None
}

/// KDL brace nesting depth at byte offset `upto`, ignoring braces inside
/// double-quoted strings, `//` line comments, and `/* */` block comments.
///
/// Block comments matter and are easy to miss: they may contain *unbalanced*
/// braces, which real niri accepts. Counting those would make every following
/// node look nested and hide the genuine top-level `binds` node.
fn brace_depth_before(config: &str, upto: usize) -> i32 {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    let mut in_block_comment = false;
    let bytes = config.as_bytes();
    let mut i = 0;
    while i < upto.min(bytes.len()) {
        let c = bytes[i] as char;
        if in_block_comment {
            if c == '*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 1;
            }
        } else if in_comment {
            if c == '\n' {
                in_comment = false;
            }
        } else if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            in_comment = true;
            i += 1;
        } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_block_comment = true;
            i += 1;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
        }
        i += 1;
    }
    depth
}

/// Iterate `(byte_offset, line_without_newline)` pairs.
fn line_offsets(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        if ch == '\n' {
            out.push((start, &s[start..i]));
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push((start, &s[start..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> KeyChord {
        KeyChord::parse(s).unwrap()
    }

    fn hk(chord_str: &str, dir: &str, label: &str, self_dev: bool) -> NiriHotkey {
        NiriHotkey {
            chord: chord(chord_str),
            dir: dir.to_string(),
            label: label.to_string(),
            self_dev,
        }
    }

    #[test]
    fn maps_common_chords_to_niri_syntax() {
        // cmd maps to Super on Linux.
        assert_eq!(
            chord_to_niri_bind(&chord("cmd+;")).unwrap(),
            "Super+Semicolon"
        );
        assert_eq!(
            chord_to_niri_bind(&chord("cmd+shift+'")).unwrap(),
            "Super+Shift+Apostrophe"
        );
        assert_eq!(
            chord_to_niri_bind(&chord("alt+[")).unwrap(),
            "Alt+bracketleft"
        );
        assert_eq!(
            chord_to_niri_bind(&chord("ctrl+\\")).unwrap(),
            "Ctrl+backslash"
        );
        assert_eq!(chord_to_niri_bind(&chord("alt+b")).unwrap(), "Alt+b");
    }

    #[test]
    fn rejects_unmappable_keys() {
        // An empty/odd token has no niri name.
        assert!(niri_key_name("scrolllock").is_none());
    }

    #[test]
    fn renders_bind_line_with_cd_and_terminal() {
        let line = render_niri_bind_line(
            &hk("alt+;", "/home/jeremy/jcode", "jcode", true),
            "/home/jeremy/.local/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        assert!(line.contains("Alt+Semicolon"));
        assert!(line.contains("self-dev"));
        assert!(line.contains("hotkey-overlay-title=\"jcode: jcode (self-dev)\""));
        assert!(line.contains("spawn \"sh\" \"-c\""));
        assert!(line.contains("/home/jeremy/jcode"));
        assert!(line.starts_with("    "));
    }

    #[test]
    fn render_block_wraps_sentinels() {
        let block = render_niri_block(
            &[
                hk("alt+;", "/home/u", "home", false),
                hk("alt+'", "/home/u/proj", "proj", false),
            ],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        assert!(block.starts_with("    // >>> jcode launch hotkeys (managed) >>>"));
        assert!(
            block
                .trim_end()
                .ends_with("// <<< jcode launch hotkeys (managed) <<<")
        );
        assert_eq!(block.matches("spawn \"sh\"").count(), 2);
    }

    #[test]
    fn splice_inserts_into_existing_binds_block() {
        let cfg = "binds {\n    Alt+Tab { focus-window-previous; }\n}\n";
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let res = splice_managed_block(cfg, &block);
        assert!(res.changed);
        assert!(res.text.contains(NIRI_BLOCK_BEGIN));
        assert!(res.text.contains("Alt+Tab { focus-window-previous; }"));
        // Managed block sits after the binds { line.
        let binds_idx = res.text.find("binds {").unwrap();
        let begin_idx = res.text.find(NIRI_BLOCK_BEGIN).unwrap();
        assert!(begin_idx > binds_idx);
    }

    #[test]
    fn splice_replaces_existing_managed_region_in_place() {
        let block_v1 = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let cfg = format!("binds {{\n{block_v1}\n    Alt+Tab {{ focus-window-previous; }}\n}}\n");

        let block_v2 = render_niri_block(
            &[hk("alt+;", "/home/u/newproj", "newproj", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let res = splice_managed_block(&cfg, &block_v2);
        assert!(res.changed);
        // Only one managed region.
        assert_eq!(res.text.matches(NIRI_BLOCK_BEGIN).count(), 1);
        assert!(res.text.contains("newproj"));
        assert!(!res.text.contains("\"home\""));
        // Untouched bind preserved.
        assert!(res.text.contains("Alt+Tab { focus-window-previous; }"));
    }

    #[test]
    fn splice_is_idempotent() {
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let cfg = "binds {\n    Alt+Tab { focus-window-previous; }\n}\n";
        let first = splice_managed_block(cfg, &block);
        let second = splice_managed_block(&first.text, &block);
        assert!(!second.changed);
        assert_eq!(first.text, second.text);
    }

    #[test]
    fn splice_appends_binds_block_when_missing() {
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let cfg = "// no binds here\noutput \"eDP-1\" {}\n";
        let res = splice_managed_block(cfg, &block);
        assert!(res.changed);
        assert!(res.text.contains("binds {"));
        assert!(res.text.contains(NIRI_BLOCK_BEGIN));
    }

    #[test]
    fn splice_skips_nested_binds_and_appends_a_top_level_block() {
        // Regression for #719: a nested `recent-windows.binds` node only accepts
        // next-window/previous-window, so niri rejects an injected `spawn` there.
        let cfg = "recent-windows {\n    binds {\n        Mod+Tab { next-window; }\n    }\n}\n\ninclude \"binds.kdl\"\n";
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let res = splice_managed_block(cfg, &block);
        assert!(res.changed);

        let begin_idx = res.text.find(NIRI_BLOCK_BEGIN).unwrap();
        let nested_close = res.text.find("Mod+Tab").unwrap();
        assert!(
            begin_idx > nested_close,
            "managed block landed inside recent-windows:\n{}",
            res.text
        );
        assert!(res.text.contains("Mod+Tab { next-window; }"));
        // Depth 0 at the managed block means it is a top-level binds node.
        assert_eq!(brace_depth_before(&res.text, begin_idx), 1);
    }

    #[test]
    fn splice_prefers_a_top_level_binds_block_over_an_earlier_nested_one() {
        let cfg = "recent-windows {\n    binds {\n        Mod+Tab { next-window; }\n    }\n}\n\nbinds {\n    Alt+Tab { focus-window-previous; }\n}\n";
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let res = splice_managed_block(cfg, &block);
        let begin_idx = res.text.find(NIRI_BLOCK_BEGIN).unwrap();
        let top_binds = res.text.find("Alt+Tab").unwrap();
        assert!(begin_idx < top_binds, "{}", res.text);
        assert!(begin_idx > res.text.find("Mod+Tab").unwrap());
    }

    #[test]
    fn splice_relocates_a_managed_block_previously_written_into_a_nested_node() {
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "        ",
        )
        .unwrap();
        // Simulate the broken v1 output: managed region inside recent-windows.
        let broken = format!(
            "recent-windows {{\n    binds {{\n{block}\n        Mod+Tab {{ next-window; }}\n    }}\n}}\n"
        );
        let fixed_block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();
        let res = splice_managed_block(&broken, &fixed_block);
        assert!(res.changed);
        assert_eq!(
            res.text.matches(NIRI_BLOCK_BEGIN).count(),
            1,
            "managed region duplicated:\n{}",
            res.text
        );
        let begin_idx = res.text.find(NIRI_BLOCK_BEGIN).unwrap();
        assert!(
            begin_idx > res.text.find("Mod+Tab").unwrap(),
            "still nested:\n{}",
            res.text
        );
        assert!(res.text.contains("Mod+Tab { next-window; }"));
    }

    /// End-to-end guard for #719: feed the spliced config to the real `niri`
    /// parser. The unit tests above assert *where* the block lands; only niri
    /// itself can confirm the result is a config it will actually load, which
    /// is the property the reporter cared about.
    ///
    /// Skipped when niri is not installed, so this stays a bonus signal in CI
    /// while being a hard check on a niri machine.
    #[test]
    fn spliced_config_is_accepted_by_the_real_niri_parser() {
        if std::process::Command::new("niri")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: niri not installed");
            return;
        }

        let block = render_niri_block(
            &[hk("cmd+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();

        // Each of these is accepted by real niri on its own, and each stresses a
        // different part of the insert-point scan.
        let originals = [
            // Nested compositor binds before the real one (the reported bug).
            "recent-windows {\n    binds {\n        Mod+Tab { next-window; }\n    }\n}\n\nbinds {\n    Mod+Return { spawn \"kitty\"; }\n}\n",
            // Unbalanced brace inside a block comment.
            "/* TODO: revisit the { nesting here */\nbinds {\n    Mod+Return { spawn \"kitty\"; }\n}\n",
            // Brace inside a quoted string.
            "binds {\n    Mod+Return { spawn \"sh\" \"-c\" \"echo { }\"; }\n}\n",
            // Escaped quote then a brace, still inside the string.
            "binds {\n    Mod+Return { spawn \"sh\" \"-c\" \"echo \\\" {\"; }\n}\n",
            // KDL raw string: backslash is not an escape there.
            "binds {\n    Mod+Return { spawn \"sh\" \"-c\" r#\"echo { path\\\"#; }\n}\n",
            // No binds node at all: one must be appended.
            "output \"eDP-1\" {\n    scale 2\n}\n",
        ];

        let dir = std::env::temp_dir().join(format!("jcode-niri-719-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        for (idx, original) in originals.iter().enumerate() {
            let spliced = splice_managed_block(original, &block).text;
            let path = dir.join(format!("config-{idx}.kdl"));
            std::fs::write(&path, &spliced).unwrap();

            let out = std::process::Command::new("niri")
                .arg("validate")
                .arg("--config")
                .arg(&path)
                .output()
                .unwrap();
            let _ = std::fs::remove_file(&path);

            assert!(
                out.status.success(),
                "niri rejected the spliced config (case {idx}):\n{}\n--- config ---\n{spliced}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let _ = std::fs::remove_dir(&dir);
    }

    /// KDL block comments (`/* */`) may contain *unbalanced* braces, and real
    /// niri accepts that. The depth scanner only understood `//` line comments,
    /// so a single stray `{` in a block comment made every following node look
    /// nested, and the genuine top-level `binds` node was skipped.
    ///
    /// Found by probing real niri for constructs the scanner had not been
    /// written against, rather than by re-testing what it already handled.
    #[test]
    fn unbalanced_braces_in_a_block_comment_do_not_hide_the_binds_node() {
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();

        let cfg = "/* TODO: revisit the { nesting here */\nbinds {\n    Mod+Return { spawn \"kitty\"; }\n}\n";
        let res = splice_managed_block(cfg, &block);

        assert_eq!(
            res.text.matches("binds {").count(),
            1,
            "appended a duplicate binds node instead of using the real one:\n{}",
            res.text
        );
        let begin = res.text.find(NIRI_BLOCK_BEGIN).unwrap();
        let binds = res.text.find("binds {").unwrap();
        assert!(
            begin > binds,
            "managed block landed outside the binds node:\n{}",
            res.text
        );
        assert!(
            res.text.contains("Mod+Return"),
            "existing bind was lost:\n{}",
            res.text
        );
    }

    /// Differential fuzz against real niri (refs #719).
    ///
    /// The unit tests above check configs I thought of, which is exactly how the
    /// block-comment bug survived the first fix. This instead uses niri itself as
    /// the oracle: for every generated config that niri accepts, the config must
    /// still be accepted after jcode splices its managed block in, and the block
    /// must land in a real top-level `binds` node.
    ///
    /// The corpus lives beside this file so the test is hermetic and fast; the
    /// generator that produced it (and confirmed each input is niri-valid) is
    /// recorded in the commit that added it.
    #[test]
    fn fuzz_corpus_stays_valid_after_splicing() {
        if std::process::Command::new("niri")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: niri not installed");
            return;
        }
        let corpus = include_str!("linux_niri_fuzz_corpus.txt");
        let block = render_niri_block(
            &[hk("alt+;", "/home/u", "home", false)],
            "/bin/jcode",
            "kitty",
            "    ",
        )
        .unwrap();

        let dir = std::env::temp_dir().join(format!("jcode-niri-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut checked = 0usize;
        for (idx, case) in corpus.split("\u{1}").enumerate() {
            let case = case.trim_matches('\n');
            if case.is_empty() {
                continue;
            }
            let spliced = splice_managed_block(case, &block).text;
            let path = dir.join(format!("fuzz-{idx}.kdl"));
            std::fs::write(&path, &spliced).unwrap();
            let out = std::process::Command::new("niri")
                .arg("validate")
                .arg("--config")
                .arg(&path)
                .output()
                .unwrap();
            let _ = std::fs::remove_file(&path);
            assert!(
                out.status.success(),
                "niri rejected a spliced config that it accepted before (case {idx}):\n{}\n--- original ---\n{case}\n--- spliced ---\n{spliced}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                spliced.contains(NIRI_BLOCK_BEGIN),
                "managed block missing entirely (case {idx}):\n{spliced}"
            );
            checked += 1;
        }
        let _ = std::fs::remove_dir(&dir);
        assert!(
            checked > 100,
            "corpus too small to be meaningful: {checked}"
        );
    }

    #[test]
    fn brace_depth_ignores_braces_in_strings_and_comments() {
        let cfg = "a \"{{{\" // }}}\n{\n";
        assert_eq!(brace_depth_before(cfg, cfg.len()), 1);
    }

    #[test]
    fn shell_command_cds_and_self_devs() {
        let s = launch_shell_command("/bin/jcode", "kitty", "/home/u/proj", "cmd+shift+'", true);
        assert!(s.contains("cd '/home/u/proj'"));
        assert!(s.contains("exec 'kitty' '/bin/jcode' --spawn-hotkey"));
        assert!(s.contains("'cmd+shift+'\\''' self-dev"));
        assert!(s.contains("\"$HOME\""));
    }
}
