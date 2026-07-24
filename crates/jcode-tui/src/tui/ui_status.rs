use super::*;

/// Extract semantic version for UI display/grouping.
pub(super) fn semver() -> &'static str {
    static SEMVER: OnceLock<String> = OnceLock::new();
    SEMVER.get_or_init(|| format!("v{}", jcode_build_meta::semver()))
}

/// True when this process is running from the stable release binary path.
/// Only matches the explicit ~/.jcode/builds/stable/jcode path, NOT
/// ~/.local/bin/jcode launcher path (which now points to current).
pub(super) fn is_running_stable_release() -> bool {
    static IS_STABLE: OnceLock<bool> = OnceLock::new();
    *IS_STABLE.get_or_init(|| {
        // Use the raw symlink target (read_link), not canonicalize, to
        // check whether we're on the stable channel link.
        let current_exe = match std::env::current_exe().ok() {
            Some(path) => path,
            None => return false,
        };

        // Check if we were launched via the stable symlink
        if let Ok(stable_path) = crate::build::stable_binary_path() {
            // Compare the symlink target (not canonical) to distinguish
            // direct stable-channel execution from launcher/current links.
            let stable_target =
                std::fs::read_link(&stable_path).unwrap_or_else(|_| stable_path.clone());
            let current_target =
                std::fs::read_link(&current_exe).unwrap_or_else(|_| current_exe.clone());
            if stable_target == current_target {
                return true;
            }
            // Also check canonical paths for when launched directly
            if let (Ok(stable_canon), Ok(current_canon)) = (
                std::fs::canonicalize(&stable_path),
                std::fs::canonicalize(&current_exe),
            ) && stable_canon == current_canon
                && !current_exe.to_string_lossy().contains("target/release")
            {
                return true;
            }
        }

        false
    })
}

#[cfg(test)]
pub(crate) fn calculate_input_lines(input: &str, line_width: usize) -> usize {
    use unicode_width::UnicodeWidthChar;

    if line_width == 0 {
        return 1;
    }
    if input.is_empty() {
        return 1;
    }

    let mut total_lines = 0;
    for line in input.split("\n") {
        if line.is_empty() {
            total_lines += 1;
        } else {
            let display_width: usize = line.chars().map(|c| c.width().unwrap_or(0)).sum();
            total_lines += display_width.div_ceil(line_width);
        }
    }
    total_lines.max(1)
}

pub(super) fn shorten_model_name(model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    if model.contains("opus") {
        if model.contains("4-5") || model.contains("4.5") {
            return "claude4.5opus".to_string();
        }
        return "claudeopus".to_string();
    }
    if model.contains("sonnet") {
        if model.contains("3-5") || model.contains("3.5") {
            return "claude3.5sonnet".to_string();
        }
        return "claudesonnet".to_string();
    }
    if model.contains("haiku") {
        return "claudehaiku".to_string();
    }
    if model.starts_with("gpt-5") {
        return model.replace("gpt-", "gpt").replace("-", "");
    }
    if model.starts_with("gpt-4") {
        return model.replace("gpt-", "").replace("-", "");
    }
    if model.starts_with("gpt-3") {
        return "gpt3.5".to_string();
    }
    model.split('-').take(3).collect::<Vec<_>>().join("")
}

pub(super) fn format_status_for_debug(app: &dyn TuiState) -> String {
    match app.status() {
        ProcessingStatus::Idle => {
            if let Some(notice) = app.status_notice() {
                format!("Idle (notice: {})", notice)
            } else if let Some((input, output)) = app.total_session_tokens() {
                format!(
                    "Idle (session: {}k in, {}k out)",
                    input / 1000,
                    output / 1000
                )
            } else if let Some(tip) =
                info_widget::occasional_status_tip(120, app.animation_elapsed() as u64)
            {
                format!("Idle ({})", tip)
            } else {
                "Idle".to_string()
            }
        }
        ProcessingStatus::Sending => "Sending...".to_string(),
        ProcessingStatus::Connecting(ref phase) => format!("{}...", phase),
        ProcessingStatus::Thinking(_start) => {
            let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
            format!("Thinking... ({:.1}s)", elapsed)
        }
        ProcessingStatus::Streaming => {
            let (input, output) = app.streaming_tokens();
            format!("Streaming (↑{} ↓{})", input, output)
        }
        ProcessingStatus::WaitingForNetwork { ref listener } => {
            format!("Waiting for network to retry ({})", listener)
        }
        ProcessingStatus::RunningTool(ref name) => {
            if name == "batch"
                && let Some(progress) = app.batch_progress()
            {
                let completed = progress.completed;
                let total = progress.total;
                let mut status = format!("Running batch: {}/{} done", completed, total);
                if let Some(running) =
                    tools_ui::summarize_batch_running_tools_compact(&progress.running)
                {
                    status.push_str(&format!(", running: {}", running));
                }
                if let Some(last) = progress.last_completed.filter(|_| completed < total) {
                    status.push_str(&format!(", last done: {}", last));
                }
                return status;
            }
            format!("Running tool: {}", name)
        }
    }
}
