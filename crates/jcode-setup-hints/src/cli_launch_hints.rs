//! Native SessionStart integrations for reminding users about Jcode's global
//! launch shortcut when they open another coding CLI.
//!
//! Claude Code and Codex both expose lifecycle hooks. Using those hooks is more
//! reliable and substantially less invasive than polling the process table or
//! intercepting shell commands. The hook invokes a hidden, fast Jcode command;
//! this module then applies a cooldown and sends a local desktop notification.

use super::{LAUNCH_HOTKEY_LEARNED_USES, SetupHintsState, active_primary_launch_hotkey};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HOOK_COMMAND_MARKER: &str = "setup-hotkey --notify-cli-launch ";
const REMINDER_COOLDOWN_SECS: u64 = 7 * 24 * 60 * 60;
const MAX_REMINDERS_PER_SOURCE: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliSource {
    Claude,
    Codex,
}

impl CliSource {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Self::Claude),
            "codex" | "codex-cli" => Some(Self::Codex),
            _ => None,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

pub(super) fn install_available() -> Result<Vec<String>> {
    let mut installed = Vec::new();

    for source in [CliSource::Claude, CliSource::Codex] {
        let Some(path) = hook_path(source) else {
            continue;
        };
        let config_home_exists = path.parent().is_some_and(Path::exists);
        if !config_home_exists && !binary_on_path(source.binary()) {
            continue;
        }
        match install_hook(&path, source)
            .with_context(|| format!("installing {} SessionStart hook", source.label()))
        {
            Ok(()) => installed.push(source.label().to_string()),
            Err(err) => jcode_logging::warn(&format!(
                "could not install {} launch-shortcut reminder: {err}",
                source.label()
            )),
        }
    }

    Ok(installed)
}

pub(super) fn maybe_notify(source: &str) -> Result<()> {
    let Some(source) = CliSource::parse(source) else {
        return Ok(());
    };
    let Some((canonical, display)) = active_primary_launch_hotkey() else {
        return Ok(());
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut state = SetupHintsState::load();
    if !should_show(&state, source, &canonical, now) {
        return Ok(());
    }

    state
        .cli_launch_hint_last_shown
        .insert(source.id().to_string(), now);
    let shown = state
        .cli_launch_hint_shown_count
        .entry(source.id().to_string())
        .or_default();
    *shown = shown.saturating_add(1);
    state.save()?;

    let body = format!(
        "{} is open. Press {} anytime to launch Jcode.",
        source.label(),
        display
    );
    send_desktop_notification("Jcode shortcut", &body);
    Ok(())
}

fn should_show(state: &SetupHintsState, source: CliSource, primary_chord: &str, now: u64) -> bool {
    if state
        .launch_hotkey_usage
        .get(primary_chord)
        .copied()
        .unwrap_or(0)
        >= LAUNCH_HOTKEY_LEARNED_USES
    {
        return false;
    }

    if state
        .cli_launch_hint_shown_count
        .get(source.id())
        .copied()
        .unwrap_or(0)
        >= MAX_REMINDERS_PER_SOURCE
    {
        return false;
    }

    let last = state
        .cli_launch_hint_last_shown
        .get(source.id())
        .copied()
        .unwrap_or(0);
    last == 0 || now.saturating_sub(last) >= REMINDER_COOLDOWN_SECS
}

fn hook_path(source: CliSource) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    match source {
        CliSource::Claude => Some(
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join("settings.json"),
        ),
        CliSource::Codex => Some(
            std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("hooks.json"),
        ),
    }
}

fn binary_on_path(binary: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        if dir.join(binary).is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            return dir.join(format!("{binary}.exe")).is_file()
                || dir.join(format!("{binary}.cmd")).is_file()
                || dir.join(format!("{binary}.bat")).is_file();
        }
        #[cfg(not(windows))]
        false
    })
}

fn install_hook(path: &Path, source: CliSource) -> Result<()> {
    let mut root = if path.exists() {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {} without modifying it", path.display()))?
    } else {
        json!({})
    };

    let command = hook_command(source)?;
    if !upsert_hook(&mut root, &command)? {
        return Ok(());
    }

    let parent = path
        .parent()
        .context("external CLI hook path has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut bytes = serde_json::to_vec_pretty(&root)?;
    bytes.push(b'\n');
    jcode_storage::write_bytes(path, &bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn upsert_hook(root: &mut Value, command: &str) -> Result<bool> {
    let root_obj = root
        .as_object_mut()
        .context("hook config root must be a JSON object")?;
    let hooks = root_obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .context("hook config `hooks` must be a JSON object")?;
    let groups = hooks_obj.entry("SessionStart").or_insert_with(|| json!([]));
    let groups = groups
        .as_array_mut()
        .context("hook config `hooks.SessionStart` must be an array")?;

    let desired = json!({
        "matcher": "startup|resume",
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 5
        }]
    });

    for group in groups.iter_mut() {
        let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        let Some(index) = handlers.iter().position(|handler| {
            handler
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains(HOOK_COMMAND_MARKER))
        }) else {
            continue;
        };

        // Our installer creates a dedicated matcher group. If a user later adds
        // another handler beside ours, update only our handler so their command
        // is never discarded or silently moved to a different matcher.
        if handlers.len() > 1 {
            let desired_handler = desired["hooks"][0].clone();
            if handlers[index] == desired_handler {
                return Ok(false);
            }
            handlers[index] = desired_handler;
            return Ok(true);
        }

        if *group == desired {
            return Ok(false);
        }
        *group = desired;
        return Ok(true);
    }

    groups.push(desired);
    Ok(true)
}

fn hook_command(source: CliSource) -> Result<String> {
    let executable = trusted_jcode_executable()?;
    Ok(format!(
        "{} {HOOK_COMMAND_MARKER}{}",
        quote_hook_executable(&executable),
        source.id()
    ))
}

/// Prefer the stable launcher path so upgrades keep working, while avoiding a
/// bare `jcode` lookup in the external CLI's project-scoped PATH. Falling back
/// to the current absolute executable is still safer than shell resolution and
/// remains valid for immutable release/self-dev build channels.
fn trusted_jcode_executable() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let launcher = PathBuf::from(local_app_data)
                .join("jcode")
                .join("bin")
                .join("jcode.exe");
            if launcher.is_file() {
                return Ok(launcher);
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(home) = dirs::home_dir() {
            let launcher = home.join(".local").join("bin").join("jcode");
            if launcher.is_file() {
                return Ok(launcher);
            }
        }
    }

    std::env::current_exe().context("resolving the Jcode executable for lifecycle hooks")
}

fn quote_hook_executable(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn send_desktop_notification(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        fn escape(value: &str) -> String {
            value.replace('\\', "\\\\").replace('"', "\\\"")
        }
        let mut script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape(body),
            escape(title)
        );
        // Attribute to the host terminal app so clicking the banner activates
        // the terminal rather than Script Editor.
        let launching_bundle_id = std::env::var_os("__CFBundleIdentifier");
        if let Some(bundle_id) = launching_bundle_id
            .as_deref()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "com.apple.Terminal.osascript")
        {
            script = format!(
                "tell application id \"{}\" to {}",
                escape(bundle_id),
                script
            );
        }
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .arg("--app-name=jcode")
            .arg(title)
            .arg(body)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        fn ps_quote(value: &str) -> String {
            value.replace('`', "``").replace('"', "`\"")
        }
        let script = format!(
            "$title=\"{}\";$body=\"{}\";\
             [Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime]>$null;\
             [Windows.Data.Xml.Dom.XmlDocument,Windows.Data.Xml.Dom,ContentType=WindowsRuntime]>$null;\
             $xml=New-Object Windows.Data.Xml.Dom.XmlDocument;\
             $xml.LoadXml(\"<toast><visual><binding template='ToastGeneric'><text>$title</text><text>$body</text></binding></visual></toast>\");\
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Jcode').Show([Windows.UI.Notifications.ToastNotification]::new($xml))",
            ps_quote(title),
            ps_quote(body)
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    let _ = (title, body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_session_start_hook_without_replacing_existing_hooks() {
        let mut value = json!({
            "hooks": {
                "SessionStart": [{
                    "matcher": "compact",
                    "hooks": [{"type": "command", "command": "echo existing"}]
                }],
                "Stop": [{"hooks": [{"type": "command", "command": "echo stop"}]}]
            }
        });
        let command = "'/trusted/jcode' setup-hotkey --notify-cli-launch claude";
        assert!(upsert_hook(&mut value, command).unwrap());
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["hooks"]["SessionStart"][1]["hooks"][0]["command"],
            command
        );
    }

    #[test]
    fn managed_hook_update_is_idempotent() {
        let mut value = json!({});
        let command = "'/trusted/jcode' setup-hotkey --notify-cli-launch codex";
        assert!(upsert_hook(&mut value, command).unwrap());
        assert!(!upsert_hook(&mut value, command).unwrap());
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn updating_managed_hook_preserves_neighboring_user_handler() {
        let mut value = json!({
            "hooks": {
                "SessionStart": [{
                    "matcher": "startup|resume|clear",
                    "hooks": [
                        {
                            "type": "command",
                            "command": "jcode setup-hotkey --notify-cli-launch old",
                            "timeout": 30
                        },
                        {"type": "command", "command": "echo user-owned"}
                    ]
                }]
            }
        });
        let command = "'/trusted/jcode' setup-hotkey --notify-cli-launch codex";
        assert!(upsert_hook(&mut value, command).unwrap());
        let group = &value["hooks"]["SessionStart"][0];
        assert_eq!(group["matcher"], "startup|resume|clear");
        assert_eq!(group["hooks"].as_array().unwrap().len(), 2);
        assert_eq!(group["hooks"][1]["command"], "echo user-owned");
        assert_eq!(group["hooks"][0]["command"], command);
    }

    #[test]
    fn reminder_policy_spaces_repetitions_and_stops_after_learning() {
        let source = CliSource::Claude;
        let mut state = SetupHintsState::default();
        assert!(should_show(&state, source, "super+;", 1_000));

        state
            .cli_launch_hint_last_shown
            .insert(source.id().to_string(), 1_000);
        state
            .cli_launch_hint_shown_count
            .insert(source.id().to_string(), 1);
        assert!(!should_show(
            &state,
            source,
            "super+;",
            1_000 + REMINDER_COOLDOWN_SECS - 1
        ));
        assert!(should_show(
            &state,
            source,
            "super+;",
            1_000 + REMINDER_COOLDOWN_SECS
        ));

        state
            .launch_hotkey_usage
            .insert("super+;".to_string(), LAUNCH_HOTKEY_LEARNED_USES);
        assert!(!should_show(
            &state,
            source,
            "super+;",
            1_000 + REMINDER_COOLDOWN_SECS
        ));
    }

    #[test]
    fn rejects_malformed_hook_shape_instead_of_clobbering_it() {
        let mut value = json!({"hooks": {"SessionStart": {}}});
        assert!(
            upsert_hook(
                &mut value,
                "'/trusted/jcode' setup-hotkey --notify-cli-launch claude"
            )
            .is_err()
        );
    }

    #[test]
    fn install_hook_preserves_file_content_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
  "theme": "dark",
  "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo done"}]}]}
}
"#,
        )
        .unwrap();

        install_hook(&path, CliSource::Claude).unwrap();
        let first = std::fs::read(&path).unwrap();
        let parsed: Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(parsed["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let command = parsed["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.contains(HOOK_COMMAND_MARKER));
        assert!(!command.starts_with("jcode "));

        install_hook(&path, CliSource::Claude).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_hook_executable_quoting_handles_spaces_and_single_quotes() {
        assert_eq!(
            quote_hook_executable(Path::new("/tmp/Jcode's bin/jcode")),
            "'/tmp/Jcode'\\''s bin/jcode'"
        );
    }
}
