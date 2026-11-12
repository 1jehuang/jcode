//! Multi-compositor Linux launch-hotkey support.
//!
//! niri has its own dedicated module ([`crate::linux_niri`]) because it splices
//! KDL inside a `binds { }` block. Every other supported environment
//! (Hyprland, as shipped by omarchy, plus sway and i3) uses flat `#`-commented
//! config files where a bind is a single top-level line, so they share this
//! module:
//!
//! * [`detect_compositor_from`] decides which environment the session runs,
//!   from the standard env vars. Pure over an env lookup so it is unit-tested.
//! * The **pure** renderers ([`render_hyprland_block`], [`render_sway_block`])
//!   turn resolved launch hotkeys into the exact bind lines we manage. Instead
//!   of inlining shell one-liners (a quoting minefield across three different
//!   config grammars), each bind simply executes a launch script jcode writes
//!   to `~/.jcode/hotkey/`.
//! * [`splice_flat_managed_block`] replaces/creates the sentinel-delimited
//!   managed region at file scope, leaving every other line untouched.
//!
//! The managed region is delimited by `#` sentinel comments so re-installs are
//! idempotent and a user can hand-remove it cleanly:
//!
//! ```text
//! # >>> jcode launch hotkeys (managed) >>>
//! bind = SUPER, semicolon, exec, '/home/u/.jcode/hotkey/launch_jcode_0_cmd_semicolon.sh'
//! # <<< jcode launch hotkeys (managed) <<<
//! ```

use crate::keymap::KeyChord;

/// Opening sentinel for the managed region in `#`-commented configs.
pub(crate) const HASH_BLOCK_BEGIN: &str = "# >>> jcode launch hotkeys (managed) >>>";
/// Closing sentinel for the managed region in `#`-commented configs.
pub(crate) const HASH_BLOCK_END: &str = "# <<< jcode launch hotkeys (managed) <<<";

/// A Linux desktop environment / compositor jcode can install launch hotkeys
/// into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinuxCompositor {
    Niri,
    /// Hyprland (what omarchy ships).
    Hyprland,
    Sway,
    I3,
    /// bspwm sessions, where hotkeys live in sxhkd's config.
    Bspwm,
    /// GNOME Shell (Ubuntu/Fedora default, also Budgie). Bindings go through
    /// dconf custom keybindings.
    Gnome,
    /// KDE Plasma. Bindings go through desktop files + kglobalshortcutsrc.
    Kde,
    /// Cinnamon (Linux Mint default). dconf, GNOME-style custom keybindings
    /// under /org/cinnamon/.
    Cinnamon,
    /// MATE. dconf custom keybindings under /org/mate/.
    Mate,
    /// XFCE. Bindings go through xfconf's xfce4-keyboard-shortcuts channel.
    Xfce,
}

impl LinuxCompositor {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            LinuxCompositor::Niri => "niri",
            LinuxCompositor::Hyprland => "Hyprland",
            LinuxCompositor::Sway => "sway",
            LinuxCompositor::I3 => "i3",
            LinuxCompositor::Bspwm => "bspwm/sxhkd",
            LinuxCompositor::Gnome => "GNOME",
            LinuxCompositor::Kde => "KDE Plasma",
            LinuxCompositor::Cinnamon => "Cinnamon",
            LinuxCompositor::Mate => "MATE",
            LinuxCompositor::Xfce => "XFCE",
        }
    }
}

/// Detect the running compositor from an env-var lookup. Pure so the detection
/// matrix is unit-testable; production passes `std::env::var`.
///
/// Socket/instance env vars are checked first (they are authoritative for the
/// *current* session), then the XDG desktop names, so a stale
/// `XDG_CURRENT_DESKTOP` inherited across a compositor switch loses to a live
/// socket.
pub(crate) fn detect_compositor_from(
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<LinuxCompositor> {
    let desktop_is = |name: &str| -> bool {
        let matches_var = |var: &str| {
            get(var)
                .map(|v| v.split(':').any(|d| d.eq_ignore_ascii_case(name)))
                .unwrap_or(false)
        };
        matches_var("XDG_CURRENT_DESKTOP") || matches_var("XDG_SESSION_DESKTOP")
    };

    if get("NIRI_SOCKET").is_some() || desktop_is("niri") {
        return Some(LinuxCompositor::Niri);
    }
    if get("HYPRLAND_INSTANCE_SIGNATURE").is_some() || desktop_is("hyprland") {
        return Some(LinuxCompositor::Hyprland);
    }
    if get("SWAYSOCK").is_some() || desktop_is("sway") {
        return Some(LinuxCompositor::Sway);
    }
    if get("I3SOCK").is_some() || desktop_is("i3") {
        return Some(LinuxCompositor::I3);
    }
    if get("BSPWM_SOCKET").is_some() || desktop_is("bspwm") {
        return Some(LinuxCompositor::Bspwm);
    }
    // Desktop-name-only environments last so a live tiling-WM socket always
    // wins over a stale desktop var. Budgie reports "Budgie:GNOME" and uses
    // GNOME's settings daemon, so it lands on the Gnome path.
    if desktop_is("gnome") || desktop_is("ubuntu") {
        return Some(LinuxCompositor::Gnome);
    }
    if desktop_is("kde") || get("KDE_FULL_SESSION").is_some() {
        return Some(LinuxCompositor::Kde);
    }
    if desktop_is("x-cinnamon") || desktop_is("cinnamon") {
        return Some(LinuxCompositor::Cinnamon);
    }
    if desktop_is("mate") {
        return Some(LinuxCompositor::Mate);
    }
    if desktop_is("xfce") {
        return Some(LinuxCompositor::Xfce);
    }
    None
}

/// One launch hotkey resolved down to "this chord runs this script".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScriptBind {
    pub chord: KeyChord,
    /// Absolute path of the executable launch script.
    pub script: String,
    /// Short human label, e.g. the repo's directory name.
    pub label: String,
    pub self_dev: bool,
}

/// Translate a canonical jcode key token into an XKB keysym name (the
/// vocabulary Hyprland, sway, and i3 all accept). Returns `None` for tokens
/// with no stable spelling.
pub(crate) fn xkb_key_name(key: &str) -> Option<String> {
    let named = match key {
        ";" => "semicolon",
        "'" => "apostrophe",
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
        "pageup" => "Prior",
        "pagedown" => "Next",
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
            if other.len() == 1 && other.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(other.to_string());
            }
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

/// POSIX-shell single-quote escaping for a path embedded in a bind line.
pub(crate) fn sh_single_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

/// Render one Hyprland `bind` line, or `None` if the chord cannot be
/// expressed. jcode's `cmd` modifier maps to `SUPER`.
///
/// Hyprland's `exec` dispatcher passes the remainder of the line to a shell,
/// so the script path is single-quoted to survive spaces.
pub(crate) fn render_hyprland_bind_line(bind: &ScriptBind) -> Option<String> {
    let key = xkb_key_name(&bind.chord.key)?;
    let mods = hyprland_mods(&bind.chord);
    Some(format!(
        "bind = {mods}, {key}, exec, {script}",
        script = sh_single_quote(&bind.script),
    ))
}

/// Hyprland modifier list (space-separated, e.g. `SUPER SHIFT`). jcode `cmd`
/// maps to `SUPER`.
fn hyprland_mods(chord: &KeyChord) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if chord.cmd {
        parts.push("SUPER");
    }
    if chord.ctrl {
        parts.push("CTRL");
    }
    if chord.alt {
        parts.push("ALT");
    }
    if chord.shift {
        parts.push("SHIFT");
    }
    parts.join(" ")
}

/// Render one sway/i3 `bindsym` line, or `None` if the chord cannot be
/// expressed. jcode's `cmd` maps to `Mod4` (super) and `alt` to `Mod1`.
///
/// i3 (and sway, which accepts the same grammar) treats `,` and `;` as command
/// separators inside a bind, so the exec payload is a quoted script path with
/// no shell metacharacters. `--no-startup-id` suppresses i3's startup-
/// notification cursor; sway accepts and ignores it.
pub(crate) fn render_sway_bind_line(bind: &ScriptBind) -> Option<String> {
    let key = xkb_key_name(&bind.chord.key)?;
    let mut parts: Vec<String> = Vec::new();
    if bind.chord.cmd {
        parts.push("Mod4".to_string());
    }
    if bind.chord.ctrl {
        parts.push("Ctrl".to_string());
    }
    if bind.chord.alt {
        parts.push("Mod1".to_string());
    }
    if bind.chord.shift {
        parts.push("Shift".to_string());
    }
    parts.push(key);
    let combo = parts.join("+");
    let script = bind.script.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!("bindsym {combo} exec --no-startup-id \"{script}\""))
}

/// Render the full managed block for a flat `#`-commented config. `render_line`
/// is the per-compositor bind renderer. Each bind is preceded by a label
/// comment on its own line (i3 only supports whole-line comments, so labels
/// are never appended to the bind line itself). Hotkeys that cannot be
/// expressed are skipped; returns `None` when nothing could be rendered.
pub(crate) fn render_flat_block(
    binds: &[ScriptBind],
    render_line: impl Fn(&ScriptBind) -> Option<String>,
) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for bind in binds {
        if let Some(line) = render_line(bind) {
            lines.push(format!("# jcode: {label}", label = bind.label));
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str(HASH_BLOCK_BEGIN);
    out.push('\n');
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(HASH_BLOCK_END);
    Some(out)
}

/// Render the managed Hyprland block.
pub(crate) fn render_hyprland_block(binds: &[ScriptBind]) -> Option<String> {
    render_flat_block(binds, render_hyprland_bind_line)
}

/// Render the managed sway/i3 block.
pub(crate) fn render_sway_block(binds: &[ScriptBind]) -> Option<String> {
    render_flat_block(binds, render_sway_bind_line)
}

/// Render one sxhkd chord + command stanza, or `None` if the chord cannot be
/// expressed. sxhkd wants lowercase modifier names joined with ` + ` on one
/// line and the command indented beneath. jcode's `cmd` maps to `super`.
pub(crate) fn render_sxhkd_stanza(bind: &ScriptBind) -> Option<String> {
    let key = xkb_key_name(&bind.chord.key)?;
    let mut parts: Vec<String> = Vec::new();
    if bind.chord.cmd {
        parts.push("super".to_string());
    }
    if bind.chord.ctrl {
        parts.push("ctrl".to_string());
    }
    if bind.chord.alt {
        parts.push("alt".to_string());
    }
    if bind.chord.shift {
        parts.push("shift".to_string());
    }
    parts.push(key);
    Some(format!(
        "{combo}\n    {script}",
        combo = parts.join(" + "),
        script = sh_single_quote(&bind.script),
    ))
}

/// Render the managed sxhkd block (multi-line stanzas inside the same `#`
/// sentinels as other flat configs).
pub(crate) fn render_sxhkd_block(binds: &[ScriptBind]) -> Option<String> {
    render_flat_block(binds, render_sxhkd_stanza)
}

/// Map a chord onto GNOME's gsettings accelerator syntax, e.g.
/// `<Super>semicolon` or `<Super><Shift>apostrophe`. jcode's `cmd` maps to
/// `<Super>`. Returns `None` for keys with no XKB name.
pub(crate) fn gnome_binding(chord: &KeyChord) -> Option<String> {
    let key = xkb_key_name(&chord.key)?;
    let mut out = String::new();
    if chord.cmd {
        out.push_str("<Super>");
    }
    if chord.ctrl {
        out.push_str("<Control>");
    }
    if chord.alt {
        out.push_str("<Alt>");
    }
    if chord.shift {
        out.push_str("<Shift>");
    }
    out.push_str(&key);
    Some(out)
}

/// The dconf path for jcode's Nth GNOME custom keybinding. Slot-stable so
/// re-installs overwrite rather than accumulate.
pub(crate) fn gnome_keybinding_path(index: usize) -> String {
    format!(
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/jcode-launch-{index}/"
    )
}

/// One GNOME custom keybinding ready to apply via gsettings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GnomeKeybinding {
    /// dconf path (`gnome_keybinding_path` output).
    pub path: String,
    /// Human name shown in GNOME Settings.
    pub name: String,
    /// Command executed on press (the launch script).
    pub command: String,
    /// Accelerator string, e.g. `<Super>semicolon`.
    pub binding: String,
}

/// Resolve script binds into GNOME custom keybindings. Unmappable chords are
/// skipped; returns an empty vec when nothing is expressible.
pub(crate) fn gnome_keybindings(binds: &[ScriptBind]) -> Vec<GnomeKeybinding> {
    binds
        .iter()
        .filter_map(|bind| {
            let binding = gnome_binding(&bind.chord)?;
            Some(GnomeKeybinding {
                path: String::new(), // filled below with a stable slot index
                name: format!("jcode: {}", bind.label),
                command: bind.script.clone(),
                binding,
            })
        })
        .enumerate()
        .map(|(index, mut kb)| {
            kb.path = gnome_keybinding_path(index);
            kb
        })
        .collect()
}

/// Merge jcode's entries into an existing gsettings/dconf string-array value
/// (e.g. `['/a/', '/b/']` or `@as []`), preserving foreign entries and
/// replacing any previous jcode slots (matched on the `jcode-launch-` marker).
/// Used for GNOME keybinding paths and Cinnamon slot names alike. Pure string
/// surgery on the GVariant text format so it is unit-testable without dconf.
pub(crate) fn merge_gnome_keybinding_list(current: &str, ours: &[String]) -> String {
    let mut entries: Vec<String> = Vec::new();
    // Parse the single-quoted entries out of the list text.
    let mut rest = current;
    while let Some(start) = rest.find('\'') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('\'') else { break };
        let entry = &after[..end];
        if !entry.contains("jcode-launch-") {
            entries.push(entry.to_string());
        }
        rest = &after[end + 1..];
    }
    entries.extend(ours.iter().cloned());
    let quoted: Vec<String> = entries.iter().map(|e| format!("'{e}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Stable dconf slot name for jcode's Nth custom keybinding (Cinnamon/MATE).
pub(crate) fn dconf_slot_name(index: usize) -> String {
    format!("jcode-launch-{index}")
}

/// One dconf-backed custom keybinding (GNOME/Cinnamon/MATE all share the
/// name+command+binding shape, differing only in tree location and value
/// types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DconfKeybinding {
    /// Slot name, e.g. `jcode-launch-0`.
    pub slot: String,
    pub name: String,
    pub command: String,
    /// Accelerator, e.g. `<Super>semicolon`.
    pub binding: String,
}

/// Resolve script binds into dconf keybindings (accelerator format shared by
/// GNOME, Cinnamon, and MATE). Unmappable chords are skipped.
pub(crate) fn dconf_keybindings(binds: &[ScriptBind]) -> Vec<DconfKeybinding> {
    binds
        .iter()
        .filter_map(|bind| {
            let binding = gnome_binding(&bind.chord)?;
            Some(DconfKeybinding {
                slot: String::new(),
                name: format!("jcode: {}", bind.label),
                command: bind.script.clone(),
                binding,
            })
        })
        .enumerate()
        .map(|(index, mut kb)| {
            kb.slot = dconf_slot_name(index);
            kb
        })
        .collect()
}

/// Map a chord onto KDE's QKeySequence syntax, e.g. `Meta+;` or
/// `Meta+Shift+'`. KDE uses literal punctuation characters, not XKB names.
/// jcode's `cmd` maps to `Meta` (the KDE name for Super).
pub(crate) fn kde_shortcut(chord: &KeyChord) -> Option<String> {
    let key: String = match chord.key.as_str() {
        k if k.len() == 1 => k.to_uppercase(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDown".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "insert" => "Ins".into(),
        "delete" => "Del".into(),
        "enter" => "Return".into(),
        "esc" => "Esc".into(),
        "tab" => "Tab".into(),
        "space" => "Space".into(),
        other => {
            if let Some(rest) = other.strip_prefix('f')
                && !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_digit())
            {
                format!("F{rest}")
            } else {
                return None;
            }
        }
    };
    let mut out = String::new();
    if chord.cmd {
        out.push_str("Meta+");
    }
    if chord.ctrl {
        out.push_str("Ctrl+");
    }
    if chord.alt {
        out.push_str("Alt+");
    }
    if chord.shift {
        out.push_str("Shift+");
    }
    out.push_str(&key);
    Some(out)
}

/// Stable desktop-file name for jcode's Nth KDE launch shortcut.
pub(crate) fn kde_desktop_file_name(index: usize) -> String {
    format!("jcode-launch-{index}.desktop")
}

/// One KDE launch shortcut: a hidden desktop file plus its
/// `kglobalshortcutsrc` `[services][...]` binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KdeShortcut {
    pub desktop_file_name: String,
    pub desktop_file_body: String,
    /// The `_launch=` accelerator for `[services][<desktop_file_name>]`.
    pub launch_shortcut: String,
}

/// Resolve script binds into KDE launch shortcuts. Unmappable chords are
/// skipped.
pub(crate) fn kde_shortcuts(binds: &[ScriptBind]) -> Vec<KdeShortcut> {
    binds
        .iter()
        .filter_map(|bind| {
            let shortcut = kde_shortcut(&bind.chord)?;
            Some((bind, shortcut))
        })
        .enumerate()
        .map(|(index, (bind, shortcut))| KdeShortcut {
            desktop_file_name: kde_desktop_file_name(index),
            desktop_file_body: format!(
                "[Desktop Entry]\nType=Application\nName=jcode: {label}\nExec={script}\nNoDisplay=true\nStartupNotify=false\nX-KDE-GlobalAccel-CommandShortcut=true\n",
                label = bind.label,
                script = bind.script,
            ),
            launch_shortcut: shortcut,
        })
        .collect()
}

/// Upsert `_launch=` entries for jcode's services into `kglobalshortcutsrc`
/// INI text: existing `[services][jcode-launch-*.desktop]` sections are
/// replaced, other sections are preserved, and missing sections are appended.
/// Pure so the INI surgery is unit-testable.
pub(crate) fn upsert_kde_shortcut_sections(current: &str, shortcuts: &[KdeShortcut]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in current.lines() {
        if line.starts_with('[') {
            skipping = line.starts_with("[services][jcode-launch-");
        }
        if !skipping {
            out.push(line.to_string());
        }
    }
    // Drop trailing blank lines so appended sections stay tidy.
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    for sc in shortcuts {
        out.push(String::new());
        out.push(format!("[services][{}]", sc.desktop_file_name));
        out.push(format!("_launch={}", sc.launch_shortcut));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Result of splicing the managed block into a config: the new text plus
/// whether anything actually changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlatSpliceResult {
    pub text: String,
    pub changed: bool,
}

/// Splice `block` (a fully-rendered managed region, no trailing newline) into a
/// flat config file: replace an existing sentinel-delimited region in place, or
/// append the block at the end of the file.
///
/// Returns `changed = false` (and the original text) when the existing managed
/// region already equals `block`, so callers can skip a no-op write.
pub(crate) fn splice_flat_managed_block(config: &str, block: &str) -> FlatSpliceResult {
    if let Some((begin, end)) = find_flat_managed_region(config) {
        let before = &config[..begin];
        let after = &config[end..];
        let new_text = format!("{before}{block}\n{after}");
        let changed = new_text != config;
        return FlatSpliceResult {
            text: new_text,
            changed,
        };
    }

    let mut new_text = config.to_string();
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    if !new_text.is_empty() {
        new_text.push('\n');
    }
    new_text.push_str(block);
    new_text.push('\n');
    FlatSpliceResult {
        text: new_text,
        changed: true,
    }
}

/// Byte range of an existing managed region: `(start_of_BEGIN_line,
/// end_of_END_line_including_newline)`. Returns `None` when either sentinel is
/// missing so a half-deleted region is never mangled.
fn find_flat_managed_region(config: &str) -> Option<(usize, usize)> {
    let begin_pos = config.find(HASH_BLOCK_BEGIN)?;
    let line_start = config[..begin_pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end_pos = config[begin_pos..].find(HASH_BLOCK_END)? + begin_pos;
    let line_end = match config[end_pos..].find('\n') {
        Some(nl) => end_pos + nl + 1,
        None => config.len(),
    };
    Some((line_start, line_end))
}

/// Build the shell command a launch script uses to open jcode in the user's
/// terminal, as a `argv`-quoted string (e.g. `'kitty' '/bin/jcode' 'self-dev'`).
/// Terminals differ in how they accept a command to run.
pub(crate) fn terminal_exec_command(
    terminal: &str,
    exe_path: &str,
    spawn_hotkey: &str,
    self_dev: bool,
) -> String {
    let base = std::path::Path::new(terminal)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| terminal.to_string());
    let mut argv: Vec<String> = match base.as_str() {
        "wezterm" => vec![terminal.to_string(), "start".to_string(), "--".to_string()],
        "alacritty" | "ghostty" | "konsole" | "xterm" => {
            vec![terminal.to_string(), "-e".to_string()]
        }
        // kitty, foot, and most others accept the command as direct argv.
        _ => vec![terminal.to_string()],
    };
    argv.push(exe_path.to_string());
    argv.push("--spawn-hotkey".to_string());
    argv.push(spawn_hotkey.to_string());
    if self_dev {
        argv.push("self-dev".to_string());
    }
    argv.iter()
        .map(|a| sh_single_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> KeyChord {
        KeyChord::parse(s).unwrap()
    }

    fn bind(chord_str: &str, script: &str, label: &str, self_dev: bool) -> ScriptBind {
        ScriptBind {
            chord: chord(chord_str),
            script: script.to_string(),
            label: label.to_string(),
            self_dev,
        }
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn detects_compositors_from_sockets_and_desktop_names() {
        type Case<'a> = (Vec<(&'a str, &'a str)>, Option<LinuxCompositor>);
        let cases: Vec<Case> = vec![
            (
                vec![("NIRI_SOCKET", "/run/niri.sock")],
                Some(LinuxCompositor::Niri),
            ),
            (
                vec![("HYPRLAND_INSTANCE_SIGNATURE", "abc123")],
                Some(LinuxCompositor::Hyprland),
            ),
            // omarchy sets XDG_CURRENT_DESKTOP=Hyprland.
            (
                vec![("XDG_CURRENT_DESKTOP", "Hyprland")],
                Some(LinuxCompositor::Hyprland),
            ),
            (
                vec![("SWAYSOCK", "/run/sway.sock")],
                Some(LinuxCompositor::Sway),
            ),
            (
                vec![("XDG_CURRENT_DESKTOP", "sway")],
                Some(LinuxCompositor::Sway),
            ),
            (vec![("I3SOCK", "/run/i3.sock")], Some(LinuxCompositor::I3)),
            (
                vec![("XDG_SESSION_DESKTOP", "i3")],
                Some(LinuxCompositor::I3),
            ),
            (
                vec![("BSPWM_SOCKET", "/tmp/bspwm-socket")],
                Some(LinuxCompositor::Bspwm),
            ),
            // Ubuntu GNOME sets "ubuntu:GNOME".
            (
                vec![("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")],
                Some(LinuxCompositor::Gnome),
            ),
            (
                vec![("XDG_CURRENT_DESKTOP", "GNOME")],
                Some(LinuxCompositor::Gnome),
            ),
            (
                vec![("XDG_CURRENT_DESKTOP", "KDE")],
                Some(LinuxCompositor::Kde),
            ),
            (
                vec![("KDE_FULL_SESSION", "true")],
                Some(LinuxCompositor::Kde),
            ),
            // Mint Cinnamon reports X-Cinnamon.
            (
                vec![("XDG_CURRENT_DESKTOP", "X-Cinnamon")],
                Some(LinuxCompositor::Cinnamon),
            ),
            (
                vec![("XDG_CURRENT_DESKTOP", "MATE")],
                Some(LinuxCompositor::Mate),
            ),
            (
                vec![("XDG_CURRENT_DESKTOP", "XFCE")],
                Some(LinuxCompositor::Xfce),
            ),
            // Budgie reports "Budgie:GNOME" and uses GNOME's settings daemon.
            (
                vec![("XDG_CURRENT_DESKTOP", "Budgie:GNOME")],
                Some(LinuxCompositor::Gnome),
            ),
            (vec![("XDG_CURRENT_DESKTOP", "LXQt")], None),
            (vec![], None),
        ];
        for (pairs, expected) in cases {
            let got = detect_compositor_from(&env(&pairs));
            assert_eq!(got, expected, "env {pairs:?}");
        }
    }

    #[test]
    fn tiling_socket_beats_gnome_kde_desktop_names() {
        // A Hyprland session nested under / started from a GNOME login must
        // resolve to Hyprland, not GNOME.
        let pairs = vec![
            ("HYPRLAND_INSTANCE_SIGNATURE", "sig"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
        ];
        assert_eq!(
            detect_compositor_from(&env(&pairs)),
            Some(LinuxCompositor::Hyprland)
        );
        let pairs = vec![("SWAYSOCK", "/run/s"), ("KDE_FULL_SESSION", "true")];
        assert_eq!(
            detect_compositor_from(&env(&pairs)),
            Some(LinuxCompositor::Sway)
        );
    }

    #[test]
    fn sxhkd_stanza_renders_super_combo_and_indented_command() {
        let s = render_sxhkd_stanza(&bind("cmd+shift+'", "/s.sh", "x", false)).unwrap();
        assert_eq!(s, "super + shift + apostrophe\n    '/s.sh'");
        let block = render_sxhkd_block(&[bind("cmd+;", "/a.sh", "home", false)]).unwrap();
        assert!(block.starts_with(HASH_BLOCK_BEGIN));
        assert!(block.contains("super + semicolon\n    '/a.sh'"));
    }

    #[test]
    fn gnome_bindings_use_angle_bracket_modifiers_and_stable_slots() {
        assert_eq!(gnome_binding(&chord("cmd+;")).unwrap(), "<Super>semicolon");
        assert_eq!(
            gnome_binding(&chord("cmd+shift+'")).unwrap(),
            "<Super><Shift>apostrophe"
        );
        let kbs = gnome_keybindings(&[
            bind("cmd+;", "/a.sh", "home", false),
            bind("cmd+scrolllock", "/bad.sh", "bad", false),
            bind("cmd+[", "/b.sh", "proj", false),
        ]);
        assert_eq!(kbs.len(), 2);
        assert_eq!(kbs[0].path, gnome_keybinding_path(0));
        assert_eq!(kbs[1].path, gnome_keybinding_path(1));
        assert_eq!(kbs[0].name, "jcode: home");
        assert_eq!(kbs[1].binding, "<Super>bracketleft");
    }

    #[test]
    fn gnome_list_merge_preserves_foreign_entries_and_replaces_ours() {
        let ours = vec![gnome_keybinding_path(0), gnome_keybinding_path(1)];
        // Empty list spellings.
        assert_eq!(
            merge_gnome_keybinding_list("@as []", &ours),
            format!("['{}', '{}']", ours[0], ours[1])
        );
        assert_eq!(
            merge_gnome_keybinding_list("[]", &ours[..1]),
            format!("['{}']", ours[0])
        );
        // Foreign entries stay; stale jcode slots are dropped before re-adding.
        let current = format!(
            "['/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/', '{}', '{}']",
            gnome_keybinding_path(0),
            gnome_keybinding_path(7),
        );
        let merged = merge_gnome_keybinding_list(&current, &ours);
        assert!(merged.contains("custom0"));
        assert!(!merged.contains("jcode-launch-7"));
        assert_eq!(merged.matches("jcode-launch-0").count(), 1);
        assert_eq!(merged.matches("jcode-launch-1").count(), 1);
        // Idempotent.
        assert_eq!(merge_gnome_keybinding_list(&merged, &ours), merged);
    }

    #[test]
    fn dconf_keybindings_share_gnome_accelerators_with_slot_names() {
        let kbs = dconf_keybindings(&[
            bind("cmd+;", "/a.sh", "home", false),
            bind("cmd+scrolllock", "/bad.sh", "bad", false),
            bind("cmd+shift+'", "/b.sh", "self-dev", true),
        ]);
        assert_eq!(kbs.len(), 2);
        assert_eq!(kbs[0].slot, "jcode-launch-0");
        assert_eq!(kbs[1].slot, "jcode-launch-1");
        assert_eq!(kbs[0].binding, "<Super>semicolon");
        assert_eq!(kbs[1].binding, "<Super><Shift>apostrophe");
        assert_eq!(kbs[1].name, "jcode: self-dev");
    }

    #[test]
    fn gnome_list_merge_also_handles_cinnamon_slot_names() {
        // Cinnamon's custom-list holds bare slot names, not dconf paths.
        let ours = vec![dconf_slot_name(0), dconf_slot_name(1)];
        let current = "['custom0', 'jcode-launch-0', 'jcode-launch-5']";
        let merged = merge_gnome_keybinding_list(current, &ours);
        assert!(merged.contains("'custom0'"));
        assert!(!merged.contains("jcode-launch-5"));
        assert_eq!(merged.matches("jcode-launch-0").count(), 1);
        assert!(merged.contains("jcode-launch-1"));
        assert_eq!(merge_gnome_keybinding_list(&merged, &ours), merged);
    }

    #[test]
    fn kde_shortcuts_use_meta_and_literal_keys() {
        assert_eq!(kde_shortcut(&chord("cmd+;")).unwrap(), "Meta+;");
        assert_eq!(kde_shortcut(&chord("cmd+shift+'")).unwrap(), "Meta+Shift+'");
        assert_eq!(kde_shortcut(&chord("ctrl+alt+k")).unwrap(), "Ctrl+Alt+K");
        assert!(kde_shortcut(&chord("cmd+scrolllock")).is_none());

        let scs = kde_shortcuts(&[
            bind("cmd+;", "/a.sh", "home", false),
            bind("cmd+shift+'", "/b.sh", "self-dev", true),
        ]);
        assert_eq!(scs.len(), 2);
        assert_eq!(scs[0].desktop_file_name, "jcode-launch-0.desktop");
        assert!(scs[0].desktop_file_body.contains("Exec=/a.sh"));
        assert!(scs[0].desktop_file_body.contains("NoDisplay=true"));
        assert_eq!(scs[1].launch_shortcut, "Meta+Shift+'");
    }

    #[test]
    fn kde_ini_upsert_replaces_our_sections_and_keeps_others() {
        let scs = kde_shortcuts(&[bind("cmd+;", "/a.sh", "home", false)]);
        let current = "[services][firefox.desktop]\n_launch=Meta+F\n\n[services][jcode-launch-0.desktop]\n_launch=Meta+X\n\n[services][jcode-launch-9.desktop]\n_launch=Meta+Y\n";
        let updated = upsert_kde_shortcut_sections(current, &scs);
        assert!(updated.contains("[services][firefox.desktop]"));
        assert!(updated.contains("_launch=Meta+F"));
        assert_eq!(
            updated
                .matches("[services][jcode-launch-0.desktop]")
                .count(),
            1
        );
        assert!(updated.contains("_launch=Meta+;"));
        assert!(!updated.contains("Meta+X"));
        assert!(!updated.contains("jcode-launch-9"));
        // Idempotent.
        assert_eq!(upsert_kde_shortcut_sections(&updated, &scs), updated);
    }

    #[test]
    fn live_socket_beats_stale_desktop_name() {
        // A user who switched from GNOME to Hyprland keeps a live Hyprland
        // instance signature; the stale desktop var must not win.
        let pairs = vec![
            ("HYPRLAND_INSTANCE_SIGNATURE", "sig"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
        ];
        assert_eq!(
            detect_compositor_from(&env(&pairs)),
            Some(LinuxCompositor::Hyprland)
        );
    }

    #[test]
    fn hyprland_bind_line_maps_cmd_to_super() {
        let line = render_hyprland_bind_line(&bind(
            "cmd+;",
            "/home/u/.jcode/hotkey/launch_jcode_0_cmd_semicolon.sh",
            "jcode",
            false,
        ))
        .unwrap();
        assert_eq!(
            line,
            "bind = SUPER, semicolon, exec, '/home/u/.jcode/hotkey/launch_jcode_0_cmd_semicolon.sh'"
        );

        let shifted = render_hyprland_bind_line(&bind("cmd+shift+'", "/s.sh", "x", true)).unwrap();
        assert!(shifted.starts_with("bind = SUPER SHIFT, apostrophe, exec, "));
    }

    #[test]
    fn sway_bind_line_maps_cmd_to_mod4_and_alt_to_mod1() {
        let line = render_sway_bind_line(&bind("cmd+;", "/s.sh", "jcode", false)).unwrap();
        assert_eq!(
            line,
            "bindsym Mod4+semicolon exec --no-startup-id \"/s.sh\""
        );

        let alt = render_sway_bind_line(&bind("alt+shift+[", "/s.sh", "x", false)).unwrap();
        assert_eq!(
            alt,
            "bindsym Mod1+Shift+bracketleft exec --no-startup-id \"/s.sh\""
        );
    }

    #[test]
    fn unmappable_keys_are_skipped_not_fatal() {
        assert!(xkb_key_name("scrolllock").is_none());
        let block = render_hyprland_block(&[
            bind("cmd+scrolllock", "/a.sh", "a", false),
            bind("cmd+]", "/b.sh", "b", false),
        ])
        .unwrap();
        assert_eq!(block.matches("bind = ").count(), 1);
        assert!(block.contains("bracketright"));
    }

    #[test]
    fn blocks_wrap_sentinels_and_carry_labels() {
        let block = render_hyprland_block(&[
            bind("cmd+;", "/a.sh", "jcode", false),
            bind("cmd+'", "/b.sh", "home", false),
        ])
        .unwrap();
        assert!(block.starts_with(HASH_BLOCK_BEGIN));
        assert!(block.ends_with(HASH_BLOCK_END));
        assert!(block.contains("# jcode: jcode"));
        assert!(block.contains("# jcode: home"));

        let sway = render_sway_block(&[bind("cmd+;", "/a.sh", "jcode", false)]).unwrap();
        assert!(sway.starts_with(HASH_BLOCK_BEGIN));
        assert!(sway.contains("bindsym Mod4+semicolon"));
    }

    #[test]
    fn render_returns_none_when_nothing_renderable() {
        assert!(render_hyprland_block(&[]).is_none());
        assert!(render_hyprland_block(&[bind("cmd+scrolllock", "/a.sh", "a", false)]).is_none());
    }

    #[test]
    fn splice_appends_then_replaces_idempotently() {
        let cfg = "# my config\nbind = SUPER, Q, killactive\n";
        let block_v1 = render_hyprland_block(&[bind("cmd+;", "/a.sh", "a", false)]).unwrap();

        let first = splice_flat_managed_block(cfg, &block_v1);
        assert!(first.changed);
        assert!(first.text.contains(HASH_BLOCK_BEGIN));
        assert!(first.text.contains("bind = SUPER, Q, killactive"));

        // Re-splicing the same block is a no-op.
        let again = splice_flat_managed_block(&first.text, &block_v1);
        assert!(!again.changed);
        assert_eq!(again.text, first.text);

        // A new block replaces the old region in place, exactly once.
        let block_v2 = render_hyprland_block(&[bind("cmd+[", "/b.sh", "b", false)]).unwrap();
        let replaced = splice_flat_managed_block(&first.text, &block_v2);
        assert!(replaced.changed);
        assert_eq!(replaced.text.matches(HASH_BLOCK_BEGIN).count(), 1);
        assert!(replaced.text.contains("/b.sh"));
        assert!(!replaced.text.contains("/a.sh"));
        assert!(replaced.text.contains("bind = SUPER, Q, killactive"));
    }

    #[test]
    fn splice_handles_file_without_trailing_newline() {
        let cfg = "bind = SUPER, Q, killactive";
        let block = render_hyprland_block(&[bind("cmd+;", "/a.sh", "a", false)]).unwrap();
        let res = splice_flat_managed_block(cfg, &block);
        assert!(res.changed);
        assert!(res.text.contains("killactive\n"));
        assert!(res.text.ends_with(&format!("{HASH_BLOCK_END}\n")));
    }

    #[test]
    fn terminal_exec_command_varies_by_terminal() {
        assert_eq!(
            terminal_exec_command("kitty", "/bin/jcode", "cmd+;", false),
            "'kitty' '/bin/jcode' '--spawn-hotkey' 'cmd+;'"
        );
        assert_eq!(
            terminal_exec_command("alacritty", "/bin/jcode", "cmd+shift+'", true),
            r#"'alacritty' '-e' '/bin/jcode' '--spawn-hotkey' 'cmd+shift+'\''' 'self-dev'"#
        );
        assert_eq!(
            terminal_exec_command("wezterm", "/bin/jcode", "cmd+;", false),
            "'wezterm' 'start' '--' '/bin/jcode' '--spawn-hotkey' 'cmd+;'"
        );
        assert_eq!(
            terminal_exec_command("foot", "/bin/jcode", "cmd+;", false),
            "'foot' '/bin/jcode' '--spawn-hotkey' 'cmd+;'"
        );
        // Full paths keep the path but dispatch on the basename.
        assert_eq!(
            terminal_exec_command("/usr/bin/ghostty", "/bin/jcode", "cmd+;", false),
            "'/usr/bin/ghostty' '-e' '/bin/jcode' '--spawn-hotkey' 'cmd+;'"
        );
    }
}
