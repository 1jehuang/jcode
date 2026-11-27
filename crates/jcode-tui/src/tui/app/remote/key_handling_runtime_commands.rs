//! Remote effort and runtime commands.

use super::*;

pub(super) async fn apply_remote_effort_direction(
    app: &mut App,
    remote: &mut RemoteConnection,
    direction: i8,
) -> Result<()> {
    // Use the best-known provider/model identity (server-reported when
    // available, header hints during the pre-History bootstrap window) and
    // live remote catalog routes so effort cycling works immediately after
    // spawn and reflects discovered sibling models (e.g. Antigravity).
    let efforts = app.remote_available_efforts();
    if efforts.is_empty() {
        app.set_status_notice("Reasoning effort not available for this provider");
        return Ok(());
    }
    let current = app.remote_reasoning_effort_hint();
    let current = current.as_deref();
    let current_index = current
        .and_then(|c| efforts.iter().position(|e| *e == c))
        .unwrap_or(efforts.len() - 1);
    let len = efforts.len();
    let next_index = if direction > 0 {
        if current_index + 1 >= len {
            current_index
        } else {
            current_index + 1
        }
    } else if current_index == 0 {
        0
    } else {
        current_index - 1
    };
    let next_effort = efforts[next_index];
    if Some(next_effort) == current {
        let label = app_mod::effort_display_label(next_effort);
        app.set_status_notice(format!(
            "Effort: {} (already at {})",
            label,
            if direction > 0 { "max" } else { "min" }
        ));
    } else {
        app.remote_reasoning_effort = Some(next_effort.to_string());
        app.invalidate_model_picker_cache();
        app.set_status_notice(format!(
            "Effort: {} (will apply to next request)",
            app_mod::effort_display_label(next_effort)
        ));
        remote.set_reasoning_effort(next_effort).await?;
    }
    Ok(())
}

/// Canonical runtime commands never run the similarly named laptop handler.
/// Keep argument validation and destructive-action confirmation on the daemon.
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
