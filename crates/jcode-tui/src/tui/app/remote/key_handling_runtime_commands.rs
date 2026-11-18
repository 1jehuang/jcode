//! Remote runtime commands.

/// Route runtime commands and their validation/confirmation to the SSH daemon.
pub(super) fn ssh_runtime_command(input: &str) -> Option<(&'static str, Vec<String>)> {
    if let Some(command) = super::super::ssh_host::command(input) {
        return Some(command);
    }
    let mut words = input.split_whitespace();
    let token = words.next()?;
    let command = match token {
        "/poke" => "poke",
        "/improve" => "improve",
        "/refactor" => "refactor",
        "/fix" => "fix",
        "/skills" => "skills",
        "/refresh-model-list" => "refresh_model_list",
        "/context" => "context",
        "/info" => "info",
        "/cache" => "cache",
        "/agents" => "agents",
        "/subagent-model" => "subagent_model",
        "/autoreview" => "autoreview",
        "/autojudge" => "autojudge",
        "/review" => "review",
        "/judge" => "judge",
        "/subagent" => "subagent",
        "/telemetry" => "telemetry",
        "/subscription" => "subscription",
        "/overnight" => "overnight",
        "/productivity" | "/wrapped" => "productivity",
        "/initiatives" | "/goals" => "initiatives",
        "/auth" => "auth",
        "/logout" => "logout",
        "/remote" => "remote",
        "/provider-test-coverage" | "/model-status" => "provider_test_coverage",
        "/z" | "/zz" | "/zzz" | "/zstatus" => "premium",
        _ => return None,
    };
    let mut args = Vec::new();
    match token {
        "/z" => args.push("normal".to_string()),
        "/zz" => args.push("toggle-one".to_string()),
        "/zzz" => args.push("toggle-zero".to_string()),
        "/zstatus" => args.push("status".to_string()),
        _ => {}
    }
    args.extend(words.map(str::to_string));
    Some((command, args))
}
