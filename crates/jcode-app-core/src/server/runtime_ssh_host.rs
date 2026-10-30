//! Host-owned SSH commands. Never launch an editor or opener on the client.
use crate::agent::Agent;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_VIEW: usize = 64 * 1024;

/// Forward through the daemon's normal per-session broadcast, not the laptop bus.
/// Ready means prepared only. A host reload must still use the existing graceful handler.
pub(super) fn maintenance_event(
    status: crate::bus::SessionUpdateStatus,
) -> (String, crate::protocol::ServerEvent) {
    use crate::bus::SessionUpdateStatus;
    let (session_id, command, state, message) = match status {
        SessionUpdateStatus::Status {
            session_id,
            action,
            message,
        } => (session_id, action.noun(), "progress", message),
        SessionUpdateStatus::NoUpdate {
            session_id,
            current,
        } => (
            session_id,
            "update",
            "current",
            format!("Host is already up to date ({current})."),
        ),
        SessionUpdateStatus::ReadyToReload {
            session_id,
            action,
            version,
        } => (
            session_id,
            action.noun(),
            "ready",
            format!(
                "Host {} prepared {version}. Run /reload to adopt it with state-preserving daemon reload.",
                action.noun()
            ),
        ),
        SessionUpdateStatus::Error {
            session_id,
            action,
            message,
        } => (
            session_id,
            action.noun(),
            "error",
            format!("Host {} failed: {}", action.noun(), display_text(&message)),
        ),
    };
    let event = crate::protocol::ServerEvent::RuntimeCommandResult {
        id: 0,
        command: command.into(),
        data: json!({"maintenance": true, "state": state, "host_owned": true}),
        message: display_text(&message),
    };
    (session_id, event)
}

pub(super) fn handles(command: &str) -> bool {
    matches!(
        command,
        "config"
            | "swarm_prompt"
            | "log"
            | "fast_default"
            | "open"
            | "file"
            | "update"
            | "rebuild"
            | "selfdev"
    )
}

/// Reject credential locations before and after canonicalization, including symlink aliases.
fn sensitive_path(path: &Path) -> bool {
    let private_home = path.components().any(|p| {
        matches!(
            p.as_os_str().to_str(),
            Some(".jcode" | ".config" | ".local")
        )
    }) && path.file_name().and_then(|p| p.to_str()) != Some("swarm-prompt.md");
    private_home
        || path.components().any(|part| {
            let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
            matches!(
                name.as_str(),
                ".ssh"
                    | ".aws"
                    | ".azure"
                    | ".gnupg"
                    | ".kube"
                    | ".docker"
                    | "config.toml"
                    | "auth"
                    | "credentials"
                    | "auth.json"
                    | "credentials.json"
                    | "credentials.toml"
                    | "accounts.json"
                    | "accounts"
                    | "secrets"
                    | "token"
                    | "tokens"
                    | ".netrc"
                    | ".npmrc"
                    | ".pypirc"
                    | ".git-credentials"
                    | "id_rsa"
                    | "id_ed25519"
                    | "id_ecdsa"
                    | "shadow"
                    | "gshadow"
                    | "master.passwd"
            ) || name == ".env"
                || name.starts_with(".env.")
                || name.ends_with(".pem")
                || name.ends_with(".key")
                || name.ends_with(".p12")
                || name.contains("credential")
                || name.contains("secret")
                || name.contains("token")
        })
}

fn bounded_read(path: &Path) -> Result<String> {
    let canonical = path.canonicalize().context("Cannot resolve remote file")?;
    if sensitive_path(path) || sensitive_path(&canonical) {
        bail!("Refusing to transmit a credential/private-auth file over remote file viewing");
    }
    if ["/proc", "/sys", "/dev"]
        .iter()
        .any(|root| canonical.starts_with(root))
    {
        bail!("Remote viewing does not expose device or process files");
    }
    if !std::fs::metadata(&canonical)?.is_file() {
        bail!("Remote viewing requires a regular file");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(&canonical)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("Remote viewing requires a regular file");
    }
    if metadata.len() > MAX_VIEW as u64 {
        bail!("Remote file exceeds the 64 KiB viewing limit");
    }
    let mut bytes = Vec::new();
    file.take(MAX_VIEW as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_VIEW {
        bail!("Remote file exceeds the 64 KiB viewing limit");
    }
    let text = String::from_utf8(bytes).context("Remote viewing supports UTF-8 text only")?;
    if text.contains('\0') || text.contains("PRIVATE KEY-----") {
        bail!("Refusing binary or private-key content");
    }
    Ok(text)
}

fn display_text(text: &str) -> String {
    // Terminal escape sequences and bidi controls must not become UI instructions.
    text.chars()
        .map(|c| {
            if (c.is_control() && c != '\n' && c != '\t')
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

fn file_result(path: &Path) -> Result<(Value, String)> {
    let text = display_text(&bounded_read(path)?);
    Ok((
        json!({"path": path.display().to_string(), "contents": text, "host_owned": true}),
        format!("Remote file: {}\n\n{}", path.display(), text),
    ))
}

fn config_path() -> Result<PathBuf> {
    crate::config::Config::path().context("No host config directory")
}

/// Redact sensitive keys recursively, including custom nested auth values.
fn redact(value: &mut toml::Value) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let key = key.to_ascii_lowercase();
                if [
                    "secret",
                    "token",
                    "password",
                    "api_key",
                    "credential",
                    "authorization",
                ]
                .iter()
                .any(|s| key.contains(s))
                {
                    *value = toml::Value::String("[redacted]".into());
                } else {
                    redact(value);
                }
            }
        }
        toml::Value::Array(values) => values.iter_mut().for_each(redact),
        _ => {}
    }
}

fn config_view(path: &Path) -> Result<(Value, String)> {
    let contents = if path.exists() {
        // Config is a deliberate exception to the general viewer, with key redaction.
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() || metadata.len() > MAX_VIEW as u64 {
            bail!("Host config is not a regular file under 64 KiB");
        }
        let mut content = String::new();
        std::fs::File::open(path)?
            .take(MAX_VIEW as u64 + 1)
            .read_to_string(&mut content)?;
        if content.len() > MAX_VIEW {
            bail!("Host config exceeds 64 KiB");
        }
        let mut parsed: toml::Value = content
            .parse()
            .context("Host config is malformed; refusing to expose unredacted contents")?;
        redact(&mut parsed);
        toml::to_string_pretty(&parsed)?
    } else {
        crate::config::Config::default_config_file_contents()
    };
    let contents = display_text(&contents);
    Ok((
        json!({"path": path.display().to_string(), "contents": contents, "host_owned": true}),
        format!(
            "Host config: {}\n\n{}\nUse /config set <dotted.key> <value> to edit on the host. Secret values are redacted.",
            path.display(),
            contents
        ),
    ))
}

fn config_set(path: &Path, key: &str, raw: &str) -> Result<()> {
    let mut schema = serde_json::to_value(crate::config::Config::default())?;
    let parts: Vec<_> = key.split('.').collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
        bail!("Use a known dotted config key such as display.centered");
    }
    let mut target = &mut schema;
    for part in &parts {
        target = target
            .get_mut(*part)
            .with_context(|| format!("Unknown config key: {key}"))?;
    }
    let parsed = format!("value = {raw}")
        .parse::<toml::Value>()
        .ok()
        .and_then(|v| v.get("value").cloned())
        .unwrap_or_else(|| toml::Value::String(raw.into()));
    *target = serde_json::to_value(&parsed)?;
    let _: crate::config::Config =
        serde_json::from_value(schema).context("Invalid config value")?;
    let mut document: toml::Value = if path.exists() {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() || metadata.len() > MAX_VIEW as u64 {
            bail!("Host config must be a regular file under 64 KiB");
        }
        let mut content = String::new();
        std::fs::File::open(path)?
            .take(MAX_VIEW as u64 + 1)
            .read_to_string(&mut content)?;
        if content.len() > MAX_VIEW {
            bail!("Host config exceeds 64 KiB");
        }
        content
            .parse()
            .context("Refusing to overwrite malformed host config")?
    } else {
        toml::Value::Table(Default::default())
    };
    let mut table = document.as_table_mut().context("Expected config table")?;
    for part in &parts[..parts.len() - 1] {
        table = table
            .entry((*part).to_string())
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .context("Config path is not a table")?;
    }
    table.insert(parts[parts.len() - 1].to_string(), parsed);
    let _: crate::config::Config = document.clone().try_into().context("Invalid host config")?;
    atomic_write(path, toml::to_string_pretty(&document)?.as_bytes())?;
    crate::config::Config::invalidate_cache();
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing parent directory")?;
    std::fs::create_dir_all(parent)?;
    // Refuse existing symlinks rather than silently replacing an unexpected destination.
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("Refusing to overwrite a symlink");
    }
    let temporary = parent.join(format!(".ssh-edit-{}", crate::id::new_id("tmp")));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(super) async fn execute(
    agent: &mut Agent,
    command: &str,
    args: &[String],
) -> Result<(Value, String)> {
    match command {
        "config" => {
            let path = config_path()?;
            match args {
                [] => config_view(&path),
                [action] if matches!(action.as_str(), "view" | "show" | "edit" | "open") => {
                    config_view(&path)
                }
                [action] if action == "init" => {
                    // create_new ensures init can never overwrite an existing config.
                    std::fs::create_dir_all(path.parent().context("Missing config directory")?)?;
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    let mut file = options
                        .open(&path)
                        .context("Config already exists or cannot be created")?;
                    file.write_all(
                        crate::config::Config::default_config_file_contents().as_bytes(),
                    )?;
                    crate::config::Config::invalidate_cache();
                    config_view(&path)
                }
                [action, key, value @ ..] if action == "set" && !value.is_empty() => {
                    config_set(&path, key, &value.join(" "))?;
                    Ok((
                        json!({"key": key, "saved": true}),
                        format!(
                            "Saved host config setting {key}. Environment overrides may still take precedence."
                        ),
                    ))
                }
                _ => bail!("Usage: /config [view|init|edit|set <dotted.key> <value>]"),
            }
        }
        "swarm_prompt" => {
            let home = crate::storage::jcode_dir()?;
            let project = agent
                .working_dir()
                .map(|p| Path::new(p).join(".jcode/swarm-prompt.md"));
            let global = home.join("swarm-prompt.md");
            match args {
                [action, scope, content @ ..]
                    if action == "set"
                        && matches!(scope.as_str(), "global" | "project")
                        && !content.is_empty() =>
                {
                    let path = if scope == "project" {
                        project.context("No host working directory")?
                    } else {
                        global
                    };
                    let text = content.join(" ").replace("\\n", "\n");
                    if text.len() > MAX_VIEW {
                        bail!("Swarm prompt exceeds 64 KiB");
                    }
                    atomic_write(&path, text.as_bytes())?;
                    Ok((
                        json!({"saved": true, "path": path.display().to_string()}),
                        format!("Saved host swarm prompt: {}", path.display()),
                    ))
                }
                [] => swarm_view(agent, project, global),
                [action] if matches!(action.as_str(), "view" | "edit" | "open") => {
                    swarm_view(agent, project, global)
                }
                _ => bail!(
                    "Usage: /swarm-prompt [view|edit|set global|project <text>] (\\n inserts a newline)"
                ),
            }
        }
        "log" => {
            if args.first().map(String::as_str) != Some("mark") {
                bail!("Usage: /log mark [note]");
            }
            let note = display_text(&args[1..].join(" "));
            if note.len() > 4096 {
                bail!("Log note exceeds 4096 bytes");
            }
            let id = crate::id::new_id("logmark");
            crate::logging::info(&format!(
                "JCODE_LOG_MARK id={id} session={} note={note:?}",
                agent.session_id()
            ));
            Ok((
                json!({"marker_id": id}),
                format!("Host log marker written: {id}"),
            ))
        }
        "fast_default" => {
            match args {
                [] => {}
                [action] if action == "status" => {}
                [action] if matches!(action.as_str(), "on" | "off") => {
                    crate::config::Config::set_openai_service_tier(Some(if action == "on" {
                        "priority"
                    } else {
                        "off"
                    }))?;
                }
                _ => bail!("Usage: /fast default [on|off|status]"),
            }
            let tier = crate::config::Config::load_strict()?
                .provider
                .openai_service_tier;
            Ok((
                json!({"default_service_tier": tier, "host_owned": true}),
                format!(
                    "Host fast default: {}. Applies to future requests/sessions using the default. Use /fast on or /fast off to change this session.",
                    if tier.as_deref() == Some("priority") {
                        "on"
                    } else {
                        "off"
                    }
                ),
            ))
        }
        "open" | "file" => {
            if args.is_empty() {
                bail!("Usage: /{command} <host text-file path>");
            }
            let input = PathBuf::from(args.join(" "));
            let path = if input.is_absolute() {
                input
            } else {
                Path::new(agent.working_dir().context("No host working directory")?).join(input)
            };
            file_result(&path)
        }
        "update" | "rebuild" => {
            if !args.is_empty() {
                bail!("/{command} accepts no arguments");
            }
            if command == "update" {
                crate::update::spawn_background_session_update(agent.session_id().into());
            } else {
                crate::session_rebuild::spawn_background_session_rebuild(agent.session_id().into());
            }
            Ok((
                json!({"requested": true, "action": command}),
                format!(
                    "Requested host {command} through the daemon maintenance queue. Watch maintenance progress for completion, then /reload to adopt the prepared binary."
                ),
            ))
        }
        "selfdev" => {
            let action = args.first().map(String::as_str).unwrap_or("status");
            if args.len() > 1
                || !matches!(
                    action,
                    "status" | "setup" | "build" | "build-reload" | "reload" | "enter"
                )
            {
                bail!("Usage: /selfdev [status|setup|enter|build|build-reload|reload]");
            }
            let output = agent
                .registry()
                .execute(
                    "selfdev",
                    json!({"action": action}),
                    crate::tool::ToolContext {
                        session_id: agent.session_id().into(),
                        message_id: crate::id::new_id("msg"),
                        tool_call_id: crate::id::new_id("call"),
                        working_dir: agent.working_dir().map(PathBuf::from),
                        stdin_request_tx: None,
                        graceful_shutdown_signal: None,
                        execution_mode: crate::tool::ToolExecutionMode::Direct,
                    },
                )
                .await?;
            Ok((
                output.metadata.unwrap_or_else(|| json!({"action": action})),
                output.output,
            ))
        }
        _ => bail!("Unsupported host command"),
    }
}

fn swarm_view(agent: &Agent, project: Option<PathBuf>, global: PathBuf) -> Result<(Value, String)> {
    let path = project.filter(|p| p.is_file()).unwrap_or(global);
    let text = if path.exists() {
        bounded_read(&path)?
    } else {
        crate::prompt::load_swarm_prompt(agent.working_dir().map(Path::new))
    };
    let text = display_text(&text);
    Ok((
        json!({"contents": text, "path": path.display().to_string()}),
        format!(
            "Host swarm prompt: {}\n\n{}\nEdit with /swarm-prompt set global|project <text> (\\n inserts a newline).",
            path.display(),
            text
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_set_preserves_unknown_tables_and_rejects_bad_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[custom]\nkeep = 42\n[display]\ncentered = false\n").unwrap();
        config_set(&path, "display.centered", "true").unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("keep = 42"));
        assert!(saved.contains("centered = true"));
        assert!(config_set(&path, "display.centered", "not-a-bool").is_err());
        assert!(config_set(&path, "display.not_a_setting", "true").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        config_set(&path, "provider.openai_service_tier", "priority").unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("openai_service_tier = \"priority\""));
    }

    #[test]
    fn maintenance_results_are_host_owned_and_do_not_claim_reload() {
        let (session, event) = maintenance_event(crate::bus::SessionUpdateStatus::ReadyToReload {
            session_id: "test-session".into(),
            action: crate::bus::ClientMaintenanceAction::Rebuild,
            version: "future-version".into(),
        });
        assert_eq!(session, "test-session");
        let crate::protocol::ServerEvent::RuntimeCommandResult {
            command,
            data,
            message,
            ..
        } = event
        else {
            panic!("wrong maintenance event");
        };
        assert_eq!(command, "rebuild");
        assert_eq!(data["state"], "ready");
        assert_eq!(data["host_owned"], true);
        assert!(message.contains("Run /reload"));
        assert!(!message.contains("reloaded"));
    }

    #[test]
    fn malformed_config_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[broken").unwrap();
        assert!(config_set(&path, "display.centered", "true").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "[broken");
    }

    #[test]
    fn viewer_bounds_and_rejects_private_content() {
        let dir = tempfile::tempdir().unwrap();
        let normal = dir.path().join("readme.txt");
        std::fs::write(&normal, "hello").unwrap();
        assert_eq!(bounded_read(&normal).unwrap(), "hello");
        std::fs::write(&normal, vec![b'x'; MAX_VIEW + 1]).unwrap();
        assert!(bounded_read(&normal).is_err());
        std::fs::write(&normal, "-----BEGIN PRIVATE KEY-----").unwrap();
        assert!(bounded_read(&normal).is_err());
        let private = dir.path().join(".env");
        std::fs::write(&private, "PASSWORD=test").unwrap();
        assert!(bounded_read(&private).is_err());
        for path in [
            "/home/user/.jcode/openai.json",
            "/home/user/.config/provider/auth.json",
            "/tmp/config.toml",
            "/home/user/.ssh/config",
        ] {
            assert!(sensitive_path(Path::new(path)), "{path}");
        }
        assert!(bounded_read(Path::new("/proc/self/environ")).is_err());
        #[cfg(unix)]
        {
            let alias = dir.path().join("alias.txt");
            std::os::unix::fs::symlink(&private, &alias).unwrap();
            assert!(bounded_read(&alias).is_err());
            assert!(atomic_write(&alias, b"replacement").is_err());
        }
    }

    #[test]
    fn config_redacts_nested_credentials_and_terminal_controls() {
        let mut value: toml::Value = "[custom]\napi_key = 'private'\nname = 'visible'\n"
            .parse()
            .unwrap();
        redact(&mut value);
        assert_eq!(value["custom"]["api_key"].as_str(), Some("[redacted]"));
        assert_eq!(value["custom"]["name"].as_str(), Some("visible"));
        assert_eq!(display_text("a\u{1b}[2J\u{202e}b"), "a�[2J�b");
    }
}
