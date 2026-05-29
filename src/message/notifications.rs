use super::*;

fn sanitize_fenced_block(text: &str) -> String {
    text.replace("```", "``\u{200b}`")
}

pub fn format_input_shell_result_markdown(shell: &InputShellResult) -> String {
    let status = if shell.failed_to_start {
        "✗ failed to start".to_string()
    } else if shell.exit_code == Some(0) {
        "✓ exit 0".to_string()
    } else if let Some(code) = shell.exit_code {
        format!("✗ exit {}", code)
    } else {
        "✗ terminated".to_string()
    };

    let mut meta = vec![status, Message::format_duration(shell.duration_ms)];
    if let Some(cwd) = shell.cwd.as_deref() {
        meta.push(format!("cwd `{}`", cwd));
    }
    if shell.truncated {
        meta.push("truncated".to_string());
    }

    let mut message = format!(
        "**Shell command** · {}\n\n```bash\n{}\n```",
        meta.join(" · "),
        sanitize_fenced_block(&shell.command)
    );

    if shell.output.trim().is_empty() {
        message.push_str("\n\n_No output._");
    } else {
        message.push_str(&format!(
            "\n\n```text\n{}\n```",
            sanitize_fenced_block(shell.output.trim_end())
        ));
    }

    message
}

pub fn input_shell_status_notice(shell: &InputShellResult) -> String {
    if shell.failed_to_start {
        "Shell command failed to start".to_string()
    } else if shell.exit_code == Some(0) {
        "Shell command completed".to_string()
    } else if let Some(code) = shell.exit_code {
        format!("Shell command failed (exit {})", code)
    } else {
        "Shell command terminated".to_string()
    }
}

fn format_background_task_status(status: &BackgroundTaskStatus) -> &'static str {
    match status {
        BackgroundTaskStatus::Completed => "✓ completed",
        BackgroundTaskStatus::Superseded => "↻ superseded",
        BackgroundTaskStatus::Failed => "✗ failed",
        BackgroundTaskStatus::Running => "running",
    }
}

fn normalize_background_task_preview(preview: &str) -> Option<String> {
    let normalized = preview.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim_end();
    if trimmed.trim().is_empty() {
        None
    } else {
        Some(sanitize_fenced_block(trimmed))
    }
}

fn sanitize_background_task_label(text: &str) -> String {
    text.replace('`', "'")
}

fn background_task_display_name<'a>(
    tool_name: &'a str,
    display_name: Option<&'a str>,
) -> Option<&'a str> {
    display_name
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != tool_name)
}

fn background_task_header_label(tool_name: &str, display_name: Option<&str>) -> String {
    if let Some(display_name) = background_task_display_name(tool_name, display_name) {
        format!(
            "`{}` (`{}`)",
            sanitize_background_task_label(display_name),
            sanitize_background_task_label(tool_name)
        )
    } else {
        format!("`{}`", sanitize_background_task_label(tool_name))
    }
}

pub fn background_task_display_label(tool_name: &str, display_name: Option<&str>) -> String {
    background_task_display_name(tool_name, display_name)
        .unwrap_or(tool_name)
        .to_string()
}

fn parse_background_task_header_label(label: &str) -> (String, Option<String>) {
    static NAMED_RE: OnceLock<Option<Regex>> = OnceLock::new();
    static TOOL_RE: OnceLock<Option<Regex>> = OnceLock::new();

    let named_re = NAMED_RE
        .get_or_init(|| {
            compile_static_regex(r"^`(?P<display_name>[^`]+)` \(`(?P<tool_name>[^`]+)`\)$")
        })
        .as_ref();
    if let Some(captures) = named_re.and_then(|re| re.captures(label)) {
        return (
            captures["tool_name"].to_string(),
            Some(captures["display_name"].to_string()),
        );
    }

    let tool_re = TOOL_RE
        .get_or_init(|| compile_static_regex(r"^`(?P<tool_name>[^`]+)`$"))
        .as_ref();
    if let Some(captures) = tool_re.and_then(|re| re.captures(label)) {
        return (captures["tool_name"].to_string(), None);
    }

    (label.trim().to_string(), None)
}

fn strip_stream_prefix(line: &str) -> &str {
    line.trim()
        .strip_prefix("[stderr] ")
        .or_else(|| line.trim().strip_prefix("[stdout] "))
        .unwrap_or_else(|| line.trim())
}

fn background_task_failure_summary(preview: &str) -> Option<String> {
    let normalized = preview.replace("\r\n", "\n").replace('\r', "\n");
    let mut fallback: Option<String> = None;

    for raw_line in normalized.lines() {
        let line = strip_stream_prefix(raw_line);
        if line.is_empty() {
            continue;
        }
        if line.contains("Compile terminated by signal")
            || line.contains("Source tree drift detected")
            || line.contains("source metadata")
        {
            return Some(line.to_string());
        }
        if fallback.is_none()
            && (line.starts_with("error:")
                || line.starts_with("Error:")
                || line.starts_with("Failed:"))
        {
            fallback = Some(line.to_string());
        }
    }

    fallback
}

pub fn format_background_task_notification_markdown(task: &BackgroundTaskCompleted) -> String {
    let exit_code = task
        .exit_code
        .map(|code| format!("exit {}", code))
        .unwrap_or_else(|| "exit n/a".to_string());

    let mut message = format!(
        "**Background task** `{}` · {} · {} · {:.1}s · {}",
        task.task_id,
        background_task_header_label(&task.tool_name, task.display_name.as_deref()),
        format_background_task_status(&task.status),
        task.duration_secs,
        exit_code,
    );

    if matches!(task.status, BackgroundTaskStatus::Failed)
        && let Some(summary) = background_task_failure_summary(&task.output_preview)
    {
        message.push_str(&format!(
            "\n\n_Failure:_ {}",
            sanitize_fenced_block(&summary)
        ));
    }

    if let Some(preview) = normalize_background_task_preview(&task.output_preview) {
        message.push_str(&format!("\n\n```text\n{}\n```", preview));
    } else {
        message.push_str("\n\n_No output captured._");
    }

    message.push_str(&format!(
        "\n\n_Full output:_ `bg action=\"output\" task_id=\"{}\"`",
        task.task_id
    ));

    message
}

pub fn format_background_task_progress_markdown(task: &BackgroundTaskProgressEvent) -> String {
    format!(
        "**Background task progress** `{}` · {}\n\n{}",
        task.task_id,
        background_task_header_label(&task.tool_name, task.display_name.as_deref()),
        jcode_background_types::format_progress_display(&task.progress, 12)
    )
}

pub fn format_model_refresh_progress_markdown(detail: &str, percent: Option<u8>) -> String {
    let detail = detail.trim();
    let progress = percent
        .map(|percent| format!("{}% · {}", percent.min(100), detail))
        .unwrap_or_else(|| detail.to_string());
    format!(
        "**Background task progress** `refresh-model-list` · `Model list refresh` (`catalog`)\n\n{}",
        progress
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBackgroundTaskProgressNotification {
    pub task_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub detail: String,
    pub summary: String,
    pub source: Option<String>,
    pub percent: Option<f32>,
}

fn split_progress_source(detail: &str) -> (String, Option<String>) {
    for source in ["reported", "parsed", "estimated"] {
        let suffix = format!(" ({source})");
        if let Some(summary) = detail.strip_suffix(&suffix) {
            return (summary.trim().to_string(), Some(source.to_string()));
        }
    }
    (detail.trim().to_string(), None)
}

fn strip_progress_bar_prefix(summary: &str) -> &str {
    if summary.starts_with('[')
        && let Some((bar, rest)) = summary.split_once("] ")
        && bar.chars().all(|ch| matches!(ch, '[' | '#' | '-'))
    {
        return rest.trim();
    }
    summary.trim()
}

fn parse_progress_percent(summary: &str) -> Option<f32> {
    static PERCENT_RE: OnceLock<Option<Regex>> = OnceLock::new();
    let percent_re = PERCENT_RE
        .get_or_init(|| compile_static_regex(r"(?P<percent>[0-9]+(?:\.[0-9]+)?)%"))
        .as_ref()?;
    let captures = percent_re.captures(summary)?;
    captures["percent"].parse::<f32>().ok()
}

pub fn parse_background_task_progress_notification_markdown(
    content: &str,
) -> Option<ParsedBackgroundTaskProgressNotification> {
    static HEADER_RE: OnceLock<Option<Regex>> = OnceLock::new();
    static INLINE_RE: OnceLock<Option<Regex>> = OnceLock::new();

    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();

    let header_re = HEADER_RE
        .get_or_init(|| {
            compile_static_regex(
                r"^\*\*Background task progress\*\* `(?P<task_id>[^`]+)` · (?P<label>.+)$",
            )
        })
        .as_ref()?;
    let inline_re = INLINE_RE
        .get_or_init(|| {
            compile_static_regex(
                r"^\*\*Background task progress\*\* `(?P<task_id>[^`]+)` · (?P<label>.+?) · (?P<detail>.+)$",
            )
        })
        .as_ref()?;

    let (task_id, tool_name, display_name, detail) =
        if let Some(captures) = inline_re.captures(trimmed) {
            let (tool_name, display_name) = parse_background_task_header_label(&captures["label"]);
            (
                captures["task_id"].to_string(),
                tool_name,
                display_name,
                captures["detail"].trim().to_string(),
            )
        } else {
            let mut lines = trimmed.lines();
            let header = lines.next()?.trim();
            let captures = header_re.captures(header)?;
            let (tool_name, display_name) = parse_background_task_header_label(&captures["label"]);
            let detail = lines
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if detail.is_empty() {
                return None;
            }
            (
                captures["task_id"].to_string(),
                tool_name,
                display_name,
                detail,
            )
        };

    let (summary_with_bar, source) = split_progress_source(&detail);
    let summary = strip_progress_bar_prefix(&summary_with_bar).to_string();
    let percent = parse_progress_percent(&summary);

    Some(ParsedBackgroundTaskProgressNotification {
        task_id,
        tool_name,
        display_name,
        detail,
        summary,
        source,
        percent,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBackgroundTaskNotification {
    pub task_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub status: String,
    pub duration: String,
    pub exit_label: String,
    pub failure_summary: Option<String>,
    pub preview: Option<String>,
    pub full_output_command: String,
}

pub fn parse_background_task_notification_markdown(
    content: &str,
) -> Option<ParsedBackgroundTaskNotification> {
    static HEADER_RE: OnceLock<Option<Regex>> = OnceLock::new();
    static FULL_OUTPUT_RE: OnceLock<Option<Regex>> = OnceLock::new();

    let header_re = HEADER_RE
        .get_or_init(|| {
            compile_static_regex(
                r"^\*\*Background task\*\* `(?P<task_id>[^`]+)` · (?P<label>.+?) · (?P<status>.+?) · (?P<duration>[0-9]+(?:\.[0-9]+)?s) · (?P<exit_label>.+)$",
            )
        })
        .as_ref()?;
    let full_output_re = FULL_OUTPUT_RE
        .get_or_init(|| compile_static_regex(r#"^_Full output:_ `(?P<command>[^`]+)`$"#))
        .as_ref()?;

    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut sections = normalized.split("\n\n");
    let header = sections.next()?.trim();
    let captures = header_re.captures(header)?;
    let (tool_name, display_name) = parse_background_task_header_label(&captures["label"]);

    let mut preview: Option<String> = None;
    let mut failure_summary: Option<String> = None;
    let mut full_output_command: Option<String> = None;

    for section in sections {
        let trimmed = section.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(captures) = full_output_re.captures(trimmed) {
            full_output_command = Some(captures["command"].to_string());
            continue;
        }

        if let Some(summary) = trimmed.strip_prefix("_Failure:_ ") {
            failure_summary = Some(summary.to_string());
            continue;
        }

        if trimmed == "_No output captured._" {
            preview = None;
            continue;
        }

        if let Some(fenced) = trimmed
            .strip_prefix("```text\n")
            .and_then(|body| body.strip_suffix("\n```"))
        {
            preview = Some(fenced.to_string());
        }
    }

    Some(ParsedBackgroundTaskNotification {
        task_id: captures["task_id"].to_string(),
        tool_name,
        display_name,
        status: captures["status"].to_string(),
        duration: captures["duration"].to_string(),
        exit_label: captures["exit_label"].to_string(),
        failure_summary,
        preview,
        full_output_command: full_output_command?,
    })
}

pub fn background_task_status_notice(task: &BackgroundTaskCompleted) -> String {
    let label = background_task_display_label(&task.tool_name, task.display_name.as_deref());
    match task.status {
        BackgroundTaskStatus::Completed => {
            format!("Background task completed · {}", label)
        }
        BackgroundTaskStatus::Superseded => {
            format!("Background task superseded · {}", label)
        }
        BackgroundTaskStatus::Failed => match task.exit_code {
            Some(code) => format!("Background task failed · {} · exit {}", label, code),
            None => format!("Background task failed · {}", label),
        },
        BackgroundTaskStatus::Running => format!("Background task running · {}", label),
    }
}
