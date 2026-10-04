//! Settings, instance resolution and the exact `claude` argv/env builder.

use std::path::PathBuf;

pub use jcode_base::config::{
    ClaudeCodeConfig as ClaudeCodeSettings, ClaudeCodeInstanceConfig as ClaudeCodeInstance,
};

/// Permission modes the CLI understands. Anything else falls back to `default`.
pub const PERMISSION_MODES: &[&str] = &[
    "default",
    "acceptEdits",
    "bypassPermissions",
    "plan",
    "auto",
    "dontAsk",
];

/// Effort levels accepted by `claude --effort`.
pub const CLAUDE_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Settings JSON that disables user hooks for probe and one-shot children.
pub const DISABLE_HOOKS_SETTINGS: &str = r#"{"disableAllHooks":true}"#;

/// `--mcp-config` value that declares the in-process `jcode` SDK MCP server.
pub const JCODE_SDK_MCP_CONFIG: &str = r#"{"mcpServers":{"jcode":{"type":"sdk","name":"jcode"}}}"#;

/// Normalize a configured permission mode (case-insensitive match).
pub fn normalize_permission_mode(raw: &str) -> &'static str {
    let raw = raw.trim();
    PERMISSION_MODES
        .iter()
        .find(|mode| mode.eq_ignore_ascii_case(raw))
        .copied()
        .unwrap_or("default")
}

/// Map a jcode reasoning effort onto a `--effort` level. `None` means "do not
/// pass the flag" (CLI default).
pub fn map_effort(effort: &str) -> Option<&'static str> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "none" | "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        _ => None,
    }
}

/// Strip jcode route prefixes from a model id.
pub fn normalize_model_id(model: &str) -> String {
    let model = model.trim();
    let model = model.strip_prefix("claude-code:").unwrap_or(model);
    model.trim().to_string()
}

/// How the child should open its Claude session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStart {
    /// Brand new session with a host-chosen UUID (`--session-id=<uuid>`).
    New(String),
    /// Continue an existing Claude session (`--resume=<id>`).
    Resume(String),
}

impl SessionStart {
    pub fn session_id(&self) -> &str {
        match self {
            Self::New(id) | Self::Resume(id) => id,
        }
    }
}

/// Everything needed to build the persistent child's argv.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub model: String,
    pub effort: Option<String>,
    pub permission_mode: String,
    pub setting_sources: Vec<String>,
    pub session: SessionStart,
    pub expose_mcp: bool,
    pub launch_args: Vec<String>,
}

/// Build the exact argv (without the binary) for the persistent child.
pub fn build_argv(spec: &SpawnSpec) -> Vec<String> {
    let mut argv: Vec<String> = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let sources: Vec<&str> = spec
        .setting_sources
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    argv.push(format!("--setting-sources={}", sources.join(",")));

    argv.push("--model".into());
    argv.push(spec.model.clone());

    if let Some(effort) = spec.effort.as_deref().and_then(map_effort) {
        argv.push("--effort".into());
        argv.push(effort.into());
    }

    let user_sets_permission = spec.launch_args.iter().any(|arg| {
        arg == "--permission-mode"
            || arg.starts_with("--permission-mode=")
            || arg == "--dangerously-skip-permissions"
    });
    let mode = normalize_permission_mode(&spec.permission_mode);
    if !user_sets_permission && mode != "default" {
        argv.push("--permission-mode".into());
        argv.push(mode.into());
        if mode == "bypassPermissions" {
            argv.push("--allow-dangerously-skip-permissions".into());
        }
    }

    match &spec.session {
        SessionStart::Resume(id) => argv.push(format!("--resume={id}")),
        SessionStart::New(id) => argv.push(format!("--session-id={id}")),
    }

    if spec.expose_mcp {
        argv.push("--mcp-config".into());
        argv.push(JCODE_SDK_MCP_CONFIG.into());
    }

    argv.extend(spec.launch_args.iter().cloned());
    argv
}

/// Argv for the init-only identity probe (no prompt is ever written).
pub fn build_probe_argv(setting_sources: &[String]) -> Vec<String> {
    let sources: Vec<&str> = setting_sources
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    vec![
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
        format!("--setting-sources={}", sources.join(",")),
        "--settings".into(),
        DISABLE_HOOKS_SETTINGS.into(),
        "--no-session-persistence".into(),
        "--strict-mcp-config".into(),
    ]
}

/// Argv for one-shot text generation (`complete_simple`).
pub fn build_oneshot_argv(model: &str, system: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--tools".into(),
        String::new(),
        "--disable-slash-commands".into(),
        "--strict-mcp-config".into(),
        "--permission-mode".into(),
        "dontAsk".into(),
        "--settings".into(),
        DISABLE_HOOKS_SETTINGS.into(),
        "--no-session-persistence".into(),
        "--model".into(),
        model.into(),
    ];
    if !system.trim().is_empty() {
        argv.push("--system-prompt".into());
        argv.push(system.into());
    }
    argv
}

/// Environment additions for a child of `instance`. `HOME` is never set:
/// overriding it breaks the macOS keychain lookup Claude Code relies on.
pub fn instance_env(instance: &ClaudeCodeInstance) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = instance
        .env
        .iter()
        .filter(|(key, _)| {
            let keep = key.as_str() != "HOME" && key.as_str() != "CLAUDE_CONFIG_DIR";
            if !keep {
                jcode_base::logging::warn(&format!(
                    "Claude Code instance '{}': ignoring env override for {key}",
                    instance.id
                ));
            }
            keep
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(home) = instance.resolved_home() {
        env.push((
            "CLAUDE_CONFIG_DIR".into(),
            home.to_string_lossy().into_owned(),
        ));
    }
    env.push(("CLAUDE_CODE_ENTRYPOINT".into(), "sdk-ts".into()));
    env
}

/// Extra env for the identity probe so it never connects to remote MCP or IDEs.
pub fn probe_env_extras() -> Vec<(String, String)> {
    vec![
        ("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "false".into()),
        ("CLAUDE_CODE_AUTO_CONNECT_IDE".into(), "0".into()),
        ("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL".into(), "1".into()),
    ]
}

/// Shell-like split of user launch args (whitespace, single and double quotes,
/// backslash escapes outside single quotes).
pub fn parse_launch_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    current.push(c);
                }
            }
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(c);
                }
            }
            None => {
                if c.is_whitespace() {
                    if in_token {
                        out.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else if c == '\'' || c == '"' {
                    quote = Some(c);
                    in_token = true;
                } else if c == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    in_token = true;
                } else {
                    current.push(c);
                    in_token = true;
                }
            }
        }
    }
    if in_token {
        out.push(current);
    }
    out
}

/// Look up an instance by id within the settings.
pub fn find_instance(settings: &ClaudeCodeSettings, id: &str) -> Option<ClaudeCodeInstance> {
    settings.instance(id)
}

/// Resolved `CLAUDE_CONFIG_DIR` for an instance id (None = CLI default home).
pub fn resolved_home(settings: &ClaudeCodeSettings, id: &str) -> Option<PathBuf> {
    settings.instance(id).and_then(|i| i.resolved_home())
}

/// User-facing login command for an instance.
pub fn login_command(binary: &str, instance: &ClaudeCodeInstance) -> String {
    match instance.resolved_home() {
        Some(home) => format!("CLAUDE_CONFIG_DIR={} {binary} auth login", home.display()),
        None => format!("{binary} auth login"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn spec(session: SessionStart) -> SpawnSpec {
        SpawnSpec {
            model: "claude-opus-5-5".into(),
            effort: None,
            permission_mode: "default".into(),
            setting_sources: vec!["user".into(), "project".into(), "local".into()],
            session,
            expose_mcp: false,
            launch_args: Vec::new(),
        }
    }

    #[test]
    fn argv_for_new_session_defaults() {
        let argv = build_argv(&spec(SessionStart::New("abc".into())));
        assert_eq!(
            argv,
            vec![
                "--output-format",
                "stream-json",
                "--verbose",
                "--input-format",
                "stream-json",
                "--include-partial-messages",
                "--permission-prompt-tool",
                "stdio",
                "--setting-sources=user,project,local",
                "--model",
                "claude-opus-5-5",
                "--session-id=abc",
            ]
        );
        assert!(!argv.iter().any(|a| a == "-p"));
    }

    #[test]
    fn argv_resume_effort_mode_mcp_and_launch_args() {
        let mut s = spec(SessionStart::Resume("sid-1".into()));
        s.effort = Some("xhigh".into());
        s.permission_mode = "acceptedits".into();
        s.expose_mcp = true;
        s.launch_args = parse_launch_args("--add-dir '/tmp/a b' --fast");
        let argv = build_argv(&s);
        let joined = argv.join(" ");
        assert!(joined.contains("--effort xhigh"));
        assert!(joined.contains("--permission-mode acceptEdits"));
        assert!(argv.contains(&"--resume=sid-1".to_string()));
        assert!(!joined.contains("--session-id"));
        let mcp = argv.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(argv[mcp + 1], JCODE_SDK_MCP_CONFIG);
        assert_eq!(&argv[argv.len() - 3..], ["--add-dir", "/tmp/a b", "--fast"]);
    }

    #[test]
    fn bypass_adds_allow_flag_and_user_override_wins() {
        let mut s = spec(SessionStart::New("x".into()));
        s.permission_mode = "bypassPermissions".into();
        let argv = build_argv(&s);
        assert!(argv.contains(&"--allow-dangerously-skip-permissions".to_string()));

        s.launch_args = vec!["--permission-mode".into(), "plan".into()];
        let argv = build_argv(&s);
        assert_eq!(argv.iter().filter(|a| *a == "--permission-mode").count(), 1);
        assert!(!argv.contains(&"--allow-dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn effort_mapping() {
        assert_eq!(map_effort("none"), Some("low"));
        assert_eq!(map_effort("MEDIUM"), Some("medium"));
        assert_eq!(map_effort("max"), Some("max"));
        assert_eq!(map_effort("swarm"), None);
        let mut s = spec(SessionStart::New("x".into()));
        s.effort = Some("swarm".into());
        assert!(!build_argv(&s).contains(&"--effort".to_string()));
    }

    #[test]
    fn model_prefix_is_stripped() {
        assert_eq!(
            normalize_model_id("claude-code:claude-opus-5-5"),
            "claude-opus-5-5"
        );
        assert_eq!(normalize_model_id(" claude-sonnet-5 "), "claude-sonnet-5");
    }

    #[test]
    fn env_sets_config_dir_only_for_home_and_never_home() {
        let default = ClaudeCodeInstance::implicit_default();
        let env = instance_env(&default);
        assert!(
            env.iter()
                .all(|(k, _)| k != "CLAUDE_CONFIG_DIR" && k != "HOME")
        );
        assert!(env.contains(&("CLAUDE_CODE_ENTRYPOINT".into(), "sdk-ts".into())));

        let mut extra = BTreeMap::new();
        extra.insert("ANTHROPIC_BASE_URL".to_string(), "http://x".to_string());
        extra.insert("HOME".to_string(), "/evil".to_string());
        let personal = ClaudeCodeInstance {
            id: "personal".into(),
            display_name: None,
            home: Some("~/.claude_personal".into()),
            env: extra,
            launch_args: None,
        };
        let env = instance_env(&personal);
        let dir = env
            .iter()
            .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(dir.ends_with(".claude_personal"));
        assert!(!dir.starts_with('~'));
        assert!(env.iter().all(|(k, _)| k != "HOME"));
        assert!(env.contains(&("ANTHROPIC_BASE_URL".into(), "http://x".into())));
    }

    #[test]
    fn empty_home_means_cli_default() {
        let inst = ClaudeCodeInstance {
            id: "a".into(),
            home: Some("  ".into()),
            ..ClaudeCodeInstance::default()
        };
        assert!(
            instance_env(&inst)
                .iter()
                .all(|(k, _)| k != "CLAUDE_CONFIG_DIR")
        );
        assert_eq!(login_command("claude", &inst), "claude auth login");
    }

    #[test]
    fn launch_arg_parsing() {
        assert_eq!(
            parse_launch_args(r#"--a "b c" 'd "e"' f\ g"#),
            vec!["--a", "b c", "d \"e\"", "f g"]
        );
        assert!(parse_launch_args("   ").is_empty());
    }

    #[test]
    fn permission_mode_normalization() {
        assert_eq!(normalize_permission_mode("PLAN"), "plan");
        assert_eq!(normalize_permission_mode("weird"), "default");
    }

    #[test]
    fn oneshot_and_probe_argv() {
        let argv = build_oneshot_argv("haiku", "be brief");
        assert_eq!(argv[0], "-p");
        let tools = argv.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(argv[tools + 1], "");
        assert!(argv.contains(&DISABLE_HOOKS_SETTINGS.to_string()));
        assert!(argv.ends_with(&["--system-prompt".into(), "be brief".into()]));

        let probe = build_probe_argv(&["user".into()]);
        assert!(probe.contains(&"--setting-sources=user".to_string()));
        assert!(probe.contains(&DISABLE_HOOKS_SETTINGS.to_string()));
        assert!(!probe.contains(&"-p".to_string()));
    }
}
