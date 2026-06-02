/// Claude Code OAuth beta headers used by the Anthropic transport.
pub const ANTHROPIC_OAUTH_BETA_HEADERS: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,advanced-tool-use-2025-11-20,effort-2025-11-24";

/// Claude Code OAuth beta headers with Anthropic's explicit 1M context beta.
pub const ANTHROPIC_OAUTH_BETA_HEADERS_1M: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,advanced-tool-use-2025-11-20,effort-2025-11-24,context-1m-2025-08-07";

/// Check if a model name explicitly requests 1M context via suffix
/// (for example `claude-opus-4-6[1m]`).
pub fn anthropic_is_1m_model(model: &str) -> bool {
    model.ends_with("[1m]")
}

/// Check if a model explicitly requests 1M context via the `[1m]` suffix.
pub fn anthropic_effectively_1m(model: &str) -> bool {
    anthropic_is_1m_model(model)
}

/// Strip the `[1m]` suffix to get the actual API model name.
pub fn anthropic_strip_1m_suffix(model: &str) -> &str {
    model.strip_suffix("[1m]").unwrap_or(model)
}

/// Get the OAuth beta header value appropriate for the model.
pub fn anthropic_oauth_beta_headers(model: &str) -> &'static str {
    if anthropic_is_1m_model(model) {
        ANTHROPIC_OAUTH_BETA_HEADERS_1M
    } else {
        ANTHROPIC_OAUTH_BETA_HEADERS
    }
}

pub fn anthropic_map_tool_name_for_oauth(name: &str) -> String {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "subagent" => "Agent",
        "schedule" => "ScheduleWakeup",
        "skill_manage" => "Skill",
        _ => name,
    }
    .to_string()
}

pub fn anthropic_map_tool_name_from_oauth(name: &str) -> String {
    match name {
        "Bash" => "bash",
        "Read" => "read",
        "Write" => "write",
        "Edit" => "edit",
        "Glob" => "glob",
        "Grep" => "grep",
        "Agent" => "subagent",
        "ScheduleWakeup" => "schedule",
        "Skill" => "skill_manage",
        _ => name,
    }
    .to_string()
}

pub fn anthropic_stainless_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn anthropic_stainless_os() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "MacOS",
        "windows" => "Windows",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_suffix_helpers_require_explicit_1m_suffix() {
        assert!(!anthropic_effectively_1m("claude-opus-4-6"));
        assert!(anthropic_effectively_1m("claude-opus-4-6[1m]"));
        assert_eq!(
            anthropic_strip_1m_suffix("claude-opus-4-6[1m]"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn oauth_beta_headers_follow_1m_suffix() {
        assert_eq!(
            anthropic_oauth_beta_headers("claude-opus-4-6"),
            ANTHROPIC_OAUTH_BETA_HEADERS
        );
        assert_eq!(
            anthropic_oauth_beta_headers("claude-opus-4-6[1m]"),
            ANTHROPIC_OAUTH_BETA_HEADERS_1M
        );
    }

    #[test]
    fn oauth_tool_name_mapping_is_reversible_for_known_tools() {
        for (local, oauth) in [
            ("bash", "Bash"),
            ("read", "Read"),
            ("subagent", "Agent"),
            ("schedule", "ScheduleWakeup"),
            ("skill_manage", "Skill"),
        ] {
            assert_eq!(anthropic_map_tool_name_for_oauth(local), oauth);
            assert_eq!(anthropic_map_tool_name_from_oauth(oauth), local);
        }
        assert_eq!(anthropic_map_tool_name_for_oauth("custom"), "custom");
    }

    #[test]
    fn stainless_labels_are_non_empty() {
        assert!(!anthropic_stainless_arch().is_empty());
        assert!(!anthropic_stainless_os().is_empty());
    }
}
