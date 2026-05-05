use super::commands::{active_session_id, active_working_dir, REVIEW_PREFERRED_MODEL};
use super::{App, DisplayMessage};
use crate::id;
use crate::message::{ContentBlock, Role, ToolCall};
use crate::session::{Session, StoredMessage};
use std::time::Instant;

fn review_session_read_only_guardrails() -> &'static str {
    "Important constraints for this session:\n\
- This session is analysis-only. Do not do the work yourself.\n\
- Do not modify files or repo state. Do not call `edit`, `write`, `multiedit`, `patch`, `apply_patch`, or destructive `bash`/`git` commands.\n\
- Do not continue implementation, fix issues, or take follow-up actions yourself.\n\
- If additional work is needed, describe it in your DM to the parent session instead.\n\
\n"
}

fn judge_session_visible_context_notice() -> &'static str {
    "Important context for this judge session:\n\
- This session contains a user-visible mirror of the parent conversation, not the full original implementation context.\n\
- It includes the user's prompts, the assistant's visible replies, and shallow summaries of visible tool calls.\n\
- It intentionally omits deep tool-result details and hidden internal context beyond what the user could see.\n\
- Base your judgment on this mirror, then verify claims by inspecting repo state or tests directly when needed.\n\
\n"
}

fn is_judge_session_title(title: Option<&str>) -> bool {
    matches!(title, Some("judge" | "autojudge"))
}

fn is_analysis_feedback_session_title(title: Option<&str>) -> bool {
    matches!(title, Some("review" | "autoreview" | "judge" | "autojudge"))
}

fn resolve_feedback_target_session_id(session_id: &str) -> String {
    let mut current_id = session_id.to_string();

    for _ in 0..16 {
        let Ok(session) = Session::load(&current_id) else {
            break;
        };

        if !is_analysis_feedback_session_title(session.title.as_deref()) {
            return current_id;
        }

        let Some(parent_id) = session.parent_id.clone() else {
            return current_id;
        };

        if parent_id == current_id {
            return current_id;
        }

        current_id = parent_id;
    }

    current_id
}

pub(super) fn current_feedback_target_session_id(app: &App) -> String {
    resolve_feedback_target_session_id(&active_session_id(app))
}

fn judge_transcript_text_message(role: Role, text: String) -> StoredMessage {
    StoredMessage {
        id: id::new_id("message"),
        role,
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }
}

fn truncate_judge_visible_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated.trim_end())
}

fn judge_visible_value_summary(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(v) => Some(v.to_string()),
        serde_json::Value::Number(v) => Some(v.to_string()),
        serde_json::Value::String(v) => Some(truncate_judge_visible_text(v, 120)),
        serde_json::Value::Array(values) => Some(format!(
            "{} item{}",
            values.len(),
            if values.len() == 1 { "" } else { "s" }
        )),
        serde_json::Value::Object(map) => Some(format!(
            "{} field{}",
            map.len(),
            if map.len() == 1 { "" } else { "s" }
        )),
    }
}

fn judge_visible_tool_summary(tool: &ToolCall) -> Option<String> {
    let obj = tool.input.as_object()?;
    let preferred_keys = [
        "file_path",
        "command",
        "pattern",
        "query",
        "url",
        "path",
        "subject",
        "channel",
        "action",
        "description",
        "task_id",
        "target_session",
        "to_session",
        "model",
        "reason",
    ];
    let mut parts = Vec::new();
    for key in preferred_keys {
        let Some(value) = obj.get(key) else {
            continue;
        };
        let Some(summary) = judge_visible_value_summary(value) else {
            continue;
        };
        if summary.is_empty() {
            continue;
        }
        parts.push(format!("{}={}", key, summary));
        if parts.len() >= 2 {
            break;
        }
    }

    if parts.is_empty() {
        if obj.contains_key("patch_text") {
            let lines = obj
                .get("patch_text")
                .and_then(|v| v.as_str())
                .map(|text| text.lines().count())
                .unwrap_or(0);
            return Some(format!("patch_text={} lines", lines));
        }
        if obj.contains_key("tool_calls") {
            let count = obj
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|items| items.len())
                .unwrap_or(0);
            return Some(format!(
                "tool_calls={} item{}",
                count,
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn build_judge_visible_transcript_messages(parent_session: &Session) -> Vec<StoredMessage> {
    let mut transcript = Vec::new();

    for rendered in crate::session::render_messages(parent_session) {
        match rendered.role.as_str() {
            "user" => {
                if !rendered.content.trim().is_empty() {
                    transcript.push(judge_transcript_text_message(
                        Role::User,
                        rendered.content.trim().to_string(),
                    ));
                }
            }
            "assistant" => {
                let mut text = rendered.content.trim().to_string();
                if !rendered.tool_calls.is_empty() {
                    let visible_tools = rendered
                        .tool_calls
                        .iter()
                        .map(|name| format!("`{}`", name))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if text.is_empty() {
                        text = format!(
                            "Visible tool call{}: {}",
                            if rendered.tool_calls.len() == 1 {
                                ""
                            } else {
                                "s"
                            },
                            visible_tools
                        );
                    } else {
                        text.push_str(&format!(
                            "\n\nVisible tool call{}: {}",
                            if rendered.tool_calls.len() == 1 {
                                ""
                            } else {
                                "s"
                            },
                            visible_tools
                        ));
                    }
                }
                if !text.trim().is_empty() {
                    transcript.push(judge_transcript_text_message(Role::Assistant, text));
                }
            }
            "tool" => {
                let text = if let Some(tool) = rendered.tool_data.as_ref() {
                    let status = if rendered.content.trim_start().starts_with("Error:")
                        || rendered.content.trim_start().starts_with("error:")
                        || rendered.content.trim_start().starts_with("Failed:")
                    {
                        "failed"
                    } else {
                        "completed"
                    };
                    let summary = judge_visible_tool_summary(tool)
                        .map(|summary| format!(" — {}", summary))
                        .unwrap_or_default();
                    format!(
                        "Visible tool call: `{}`{} ({}). Detailed tool output is intentionally omitted from this judge transcript.",
                        tool.name, summary, status
                    )
                } else {
                    "Visible tool call completed. Detailed tool output is intentionally omitted from this judge transcript.".to_string()
                };
                transcript.push(judge_transcript_text_message(Role::Assistant, text));
            }
            "system" => {}
            _ => {}
        }
    }

    transcript
}

fn apply_judge_visible_context_if_needed(session: &mut Session, title_override: Option<&str>) {
    let effective_title = title_override.or(session.title.as_deref());
    if !is_judge_session_title(effective_title) {
        return;
    }

    let Some(parent_session_id) = session.parent_id.clone() else {
        return;
    };
    let Ok(parent_session) = Session::load(&parent_session_id) else {
        return;
    };

    let transcript = build_judge_visible_transcript_messages(&parent_session);
    session.replace_messages(transcript);
    session.compaction = None;
    session.provider_session_id = None;
}

pub(super) fn reset_current_session(app: &mut App) {
    app.session.mark_closed();
    if let Err(e) = app.session.save() {
        crate::logging::warn(&format!("Failed to save session: {}", e));
    }
    app.clear_provider_messages();
    app.clear_display_messages();
    app.queued_messages.clear();
    app.pasted_contents.clear();
    app.pending_images.clear();
    app.active_skill = None;
    app.improve_mode = None;
    let mut session = Session::create(None, None);
    session.mark_active();
    session.model = Some(app.provider.model());
    session.provider_key = crate::session::derive_session_provider_key(app.provider.name());
    session.autoreview_enabled = Some(app.autoreview_enabled);
    session.autojudge_enabled = Some(app.autojudge_enabled);
    session.ensure_initial_session_context_message();
    app.session = session;
    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot::default());
    app.last_side_panel_focus_id = None;
    app.diff_pane_scroll_x = 0;
    app.provider_session_id = None;
}

fn observe_status_message(app: &App) -> String {
    format!(
        "Observe mode: **{}**\n\nWhen enabled, the side panel shows a transient `Observe` page with only the latest useful tool call or tool result added to context. UI/bookkeeping tools like `side_panel`, `goal`, and todo reads/writes are skipped so the view stays readable. It is not persisted to disk.",
        if app.observe_mode_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    )
}

pub(super) fn handle_observe_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/observe") {
        return false;
    }

    let arg = trimmed.strip_prefix("/observe").unwrap_or_default().trim();
    match arg {
        "" => {
            let enabled = !app.observe_mode_enabled();
            app.set_observe_mode_enabled(enabled, true);
            if enabled {
                app.set_status_notice("Observe: ON");
                app.push_display_message(DisplayMessage::system(
                    "Observe mode enabled — the side panel now tracks the latest useful tool call/result added to context."
                        .to_string(),
                ));
            } else {
                app.set_status_notice("Observe: OFF");
                app.push_display_message(DisplayMessage::system(
                    "Observe mode disabled.".to_string(),
                ));
            }
        }
        "on" => {
            app.set_observe_mode_enabled(true, true);
            app.set_status_notice("Observe: ON");
            app.push_display_message(DisplayMessage::system(
                "Observe mode enabled — the side panel now tracks the latest useful tool call/result added to context."
                    .to_string(),
            ));
        }
        "off" => {
            app.set_observe_mode_enabled(false, false);
            app.set_status_notice("Observe: OFF");
            app.push_display_message(DisplayMessage::system("Observe mode disabled.".to_string()));
        }
        "status" => {
            app.push_display_message(DisplayMessage::system(observe_status_message(app)));
        }
        _ => {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/observe [on|off|status]`".to_string(),
            ));
        }
    }

    true
}

fn current_autoreview_model_summary(app: &App) -> String {
    crate::config::config()
        .autoreview
        .model
        .clone()
        .or_else(|| app.session.model.clone())
        .unwrap_or_else(|| app.provider.model())
}

fn current_autoreview_model_override() -> Option<String> {
    crate::config::config().autoreview.model.clone()
}

fn current_autojudge_model_summary(app: &App) -> String {
    crate::config::config()
        .autojudge
        .model
        .clone()
        .or_else(|| app.session.model.clone())
        .unwrap_or_else(|| app.provider.model())
}

fn current_autojudge_model_override() -> Option<String> {
    crate::config::config().autojudge.model.clone()
}

pub(super) fn autoreview_status_message(app: &App) -> String {
    let default_enabled = crate::config::config().autoreview.enabled;
    let config_model = crate::config::config().autoreview.model.as_deref();
    let model_line = match config_model {
        Some(model) => format!("Reviewer model override: `{}`", model),
        None => format!(
            "Reviewer model: inherit current session (`{}`)",
            current_autoreview_model_summary(app)
        ),
    };
    format!(
        "Autoreview: **{}** (config default: {})\n{}",
        if app.autoreview_enabled {
            "enabled"
        } else {
            "disabled"
        },
        if default_enabled {
            "enabled"
        } else {
            "disabled"
        },
        model_line,
    )
}

pub(super) fn autojudge_status_message(app: &App) -> String {
    let default_enabled = crate::config::config().autojudge.enabled;
    let config_model = crate::config::config().autojudge.model.as_deref();
    let model_line = match config_model {
        Some(model) => format!("Judge model override: `{}`", model),
        None => format!(
            "Judge model: inherit current session (`{}`)",
            current_autojudge_model_summary(app)
        ),
    };
    format!(
        "Autojudge: **{}** (config default: {})\n{}",
        if app.autojudge_enabled {
            "enabled"
        } else {
            "disabled"
        },
        if default_enabled {
            "enabled"
        } else {
            "disabled"
        },
        model_line,
    )
}

pub(super) fn build_autoreview_startup_message(parent_session_id: &str) -> String {
    format!(
        "You are the automatic reviewer for parent session `{}`.\n\
Your job is to inspect the just-finished work and decide whether a review is needed.\n\
\n\
First read only the conversation history you actually need:\n\
1. Use `conversation_search` with `stats=true` to learn the history size.\n\
2. Read the most recent turns with `conversation_search turns` (start with roughly the last 6-12 turns, then widen only if needed).\n\
3. If requirements are unclear, use `conversation_search query` to find the latest relevant user request or acceptance criteria.\n\
\n\
{}\
Then determine whether review is needed. Review is needed if the recent work likely changed code, config, docs, tests, tooling behavior, or made technical claims worth validating. If the recent turn was purely conversational or administrative, no review is needed.\n\
\n\
If no review is needed:\n\
- Send exactly one DM to session `{}` using `communicate` with action `dm`.\n\
- Briefly explain why no review was needed.\n\
- Then stop.\n\
\n\
If review is needed:\n\
- Inspect the actual repo changes with targeted commands such as `git diff --stat`, `git diff --name-only`, and focused file reads.\n\
- Perform a concise code review. Look for correctness bugs, regressions, missing validation, missing tests, edge cases, unsafe behavior, or broken assumptions. Prefer concrete findings over style comments.\n\
- When finished, send exactly one DM to session `{}` summarizing:\n\
  - whether review was needed\n\
  - any findings with severity and file paths\n\
  - or `No issues found` if the work looks good\n\
- After sending the DM, stop.\n\
\n\
Do not ask the user anything unless absolutely necessary. Keep your own session concise.",
        parent_session_id,
        review_session_read_only_guardrails(),
        parent_session_id,
        parent_session_id
    )
}

pub(super) fn build_autojudge_startup_message(parent_session_id: &str) -> String {
    format!(
        "You are the automatic judge for parent session `{}`.\n\
Your job is to act like a strong completion manager/reviewer for the parent agent.\n\
Your purpose is not just to critique. Your purpose is to decide whether the parent agent should keep going, and if so, tell it exactly what to do next. Only tell it to stop when the user's best likely intent has been carried through thoughtfully and completely.\n\
\n\
First read only the conversation history you actually need:\n\
1. Use `conversation_search` with `stats=true` to learn the history size.\n\
2. Read the most recent turns with `conversation_search turns` (start with roughly the last 6-12 turns, then widen only if needed).\n\
3. If requirements are unclear, use `conversation_search query` to find the latest relevant user request, constraints, preferences, or acceptance criteria.\n\
\n\
{}{}\
Then determine whether a judgment pass is needed. It is needed if the recent work likely changed code, docs, tests, tooling behavior, repo state, or made claims about what was completed. If the recent turn was purely conversational or administrative, no judgment is needed.\n\
\n\
If no judgment is needed:\n\
- Send exactly one DM to session `{}` using `communicate` with action `dm`.\n\
- Start the DM with `STOP:` and briefly explain why no judgment was needed.\n\
- Then stop.\n\
\n\
If judgment is needed:\n\
- Inspect the actual repo changes with targeted commands such as `git diff --stat`, `git diff --name-only`, focused file reads, and relevant tests or validation commands when warranted.\n\
- Evaluate: intent alignment, completeness, initiative, approach quality, correctness, validation quality, and whether obvious next steps were missed.\n\
- Prefer concrete findings over vague commentary. Call out if the work stopped after one pass when more follow-through was clearly needed.\n\
- Be strict about incomplete execution. If the parent likely stopped too early, missed obvious follow-through, only implemented a narrow slice of the user's intent, skipped validation, or left a refactor/feature half-finished, you should tell it to continue.\n\
- Default to `CONTINUE:` unless you are genuinely convinced the work is complete, well-executed, and ready to stop.\n\
- When finished, send exactly one DM to session `{}` summarizing:\n\
  - Start with either `CONTINUE:` or `STOP:`\n\
  - `CONTINUE:` means the parent should immediately keep working. Include the concrete missing follow-through, better interpretation of user intent, and the next steps to execute now. Be specific and action-oriented.\n\
  - `STOP:` means the work is aligned, thoughtful, complete, and it is fine for the parent to stop. Briefly say why the completion bar is met.\n\
  - Mention file paths, validation gaps, correctness concerns, or missed next steps when relevant.\n\
- After sending the DM, stop.\n\
\n\
Do not ask the user anything unless absolutely necessary. Keep your own session concise. Address the DM to the parent agent, not to the user.",
        parent_session_id,
        judge_session_visible_context_notice(),
        review_session_read_only_guardrails(),
        parent_session_id,
        parent_session_id
    )
}

pub(super) fn build_review_startup_message(parent_session_id: &str) -> String {
    format!(
        "You are the one-shot reviewer for parent session `{}`.\n\
Your job is to inspect the recent work, determine whether a review is needed, and perform that review if needed.\n\
\n\
First read only the conversation history you actually need:\n\
1. Use `conversation_search` with `stats=true` to learn the history size.\n\
2. Read the most recent turns with `conversation_search turns` (start with roughly the last 6-12 turns, then widen only if needed).\n\
3. If requirements are unclear, use `conversation_search query` to find the latest relevant user request or acceptance criteria.\n\
\n\
{}\
Then determine whether review is needed. Review is needed if the recent work likely changed code, config, docs, tests, tooling behavior, or made technical claims worth validating. If the recent turn was purely conversational or administrative, no review is needed.\n\
\n\
If no review is needed:\n\
- Send exactly one DM to session `{}` using `communicate` with action `dm`.\n\
- Briefly explain why no review was needed.\n\
- Then stop.\n\
\n\
If review is needed:\n\
- Inspect the actual repo changes with targeted commands such as `git diff --stat`, `git diff --name-only`, and focused file reads.\n\
- Perform a concise code review. Look for correctness bugs, regressions, missing validation, missing tests, edge cases, unsafe behavior, or broken assumptions. Prefer concrete findings over style comments.\n\
- When finished, send exactly one DM to session `{}` summarizing:\n\
  - whether review was needed\n\
  - any findings with severity and file paths\n\
  - or `No issues found` if the work looks good\n\
- After sending the DM, stop.\n\
\n\
Do not ask the user anything unless absolutely necessary. Keep your own session concise.",
        parent_session_id,
        review_session_read_only_guardrails(),
        parent_session_id,
        parent_session_id
    )
}

pub(super) fn build_judge_startup_message(parent_session_id: &str) -> String {
    format!(
        "You are the one-shot judge for parent session `{}`.\n\
Your job is to inspect the recent work, determine whether a judgment pass is needed, and perform that judgment if needed.\n\
{}\
\n\
First read only the conversation history you actually need:\n\
1. Use `conversation_search` with `stats=true` to learn the history size.\n\
2. Read the most recent turns with `conversation_search turns` (start with roughly the last 6-12 turns, then widen only if needed).\n\
3. If requirements are unclear, use `conversation_search query` to find the latest relevant user request, constraints, preferences, or acceptance criteria.\n\
\n\
{}\
Then determine whether a judgment pass is needed. It is needed if the recent work likely changed code, docs, tests, tooling behavior, repo state, or made claims about what was completed. If the recent turn was purely conversational or administrative, no judgment is needed.\n\
\n\
If no judgment is needed:\n\
- Send exactly one DM to session `{}` using `communicate` with action `dm`.\n\
- Briefly explain why no judgment was needed.\n\
- Then stop.\n\
\n\
If judgment is needed:\n\
- Inspect the actual repo changes with targeted commands such as `git diff --stat`, `git diff --name-only`, focused file reads, and relevant tests or validation commands when warranted.\n\
- Evaluate: intent alignment, completeness, initiative, approach quality, correctness, validation quality, and whether obvious next steps were missed.\n\
- Prefer concrete findings over vague commentary. Call out if the work stopped after one pass when more follow-through was clearly needed.\n\
- When finished, send exactly one DM to session `{}` summarizing:\n\
  - whether judgment was needed\n\
  - whether the work looks complete and well-executed\n\
  - any findings with severity and file paths when relevant\n\
  - specific missing follow-through or better next steps if the execution was incomplete or low-agency\n\
  - or `Looks good` if the work is aligned, thoughtful, and complete\n\
- After sending the DM, stop.\n\
\n\
Do not ask the user anything unless absolutely necessary. Keep your own session concise.",
        parent_session_id,
        judge_session_visible_context_notice(),
        review_session_read_only_guardrails(),
        parent_session_id,
        parent_session_id
    )
}

pub(super) fn preferred_one_shot_review_override() -> Option<(String, String)> {
    let creds = crate::auth::codex::load_credentials().ok()?;
    let has_oauth = !creds.refresh_token.trim().is_empty() || creds.id_token.is_some();
    if has_oauth {
        Some((REVIEW_PREFERRED_MODEL.to_string(), "openai".to_string()))
    } else {
        None
    }
}

fn current_review_model_override() -> (Option<String>, Option<String>) {
    preferred_one_shot_review_override()
        .map(|(model, provider_key)| (Some(model), Some(provider_key)))
        .unwrap_or_else(|| (current_autoreview_model_override(), None))
}

fn current_judge_model_override() -> (Option<String>, Option<String>) {
    preferred_one_shot_review_override()
        .map(|(model, provider_key)| (Some(model), Some(provider_key)))
        .unwrap_or_else(|| (current_autojudge_model_override(), None))
}

fn clone_session_for_review(
    app: &App,
    session_title: &str,
    initial_model: String,
    provider_key_override: Option<String>,
) -> anyhow::Result<(String, String)> {
    let parent_session_id = current_feedback_target_session_id(app);
    let mut child = Session::create(Some(parent_session_id), Some(session_title.to_string()));
    child.replace_messages(app.session.messages.clone());
    child.compaction = app.session.compaction.clone();
    child.working_dir = app.session.working_dir.clone();
    child.model = Some(initial_model);
    child.provider_key = provider_key_override.or_else(|| app.session.provider_key.clone());
    child.subagent_model = app.session.subagent_model.clone();
    child.autoreview_enabled = Some(false);
    child.autojudge_enabled = Some(false);
    child.status = crate::session::SessionStatus::Closed;
    child.save()?;
    Ok((child.id.clone(), child.display_name().to_string()))
}

fn clone_session_for_prompt(app: &App) -> anyhow::Result<(String, String)> {
    let mut child = Session::create(Some(active_session_id(app)), None);
    child.replace_messages(app.session.messages.clone());
    child.compaction = app.session.compaction.clone();
    child.working_dir = app.session.working_dir.clone();
    child.model = app.session.model.clone();
    child.provider_key = app.session.provider_key.clone();
    child.subagent_model = app.session.subagent_model.clone();
    child.autoreview_enabled = app.session.autoreview_enabled;
    child.autojudge_enabled = app.session.autojudge_enabled;
    child.status = crate::session::SessionStatus::Closed;
    child.save()?;
    Ok((child.id.clone(), child.display_name().to_string()))
}

pub(super) fn prepare_review_spawned_session(
    session_id: &str,
    startup_message: String,
    model_override: Option<String>,
    provider_key_override: Option<String>,
    title_override: Option<String>,
    parent_session_id_override: Option<String>,
) {
    if let Ok(mut session) = crate::session::Session::load(session_id) {
        session.autoreview_enabled = Some(false);
        session.autojudge_enabled = Some(false);
        if let Some(parent_session_id) = parent_session_id_override {
            session.parent_id = Some(parent_session_id);
        }
        if let Some(title) = title_override.clone() {
            session.title = Some(title);
        }
        if let Some(model) = model_override {
            session.model = Some(model);
        }
        if provider_key_override.is_some() {
            session.provider_key = provider_key_override;
        }
        apply_judge_visible_context_if_needed(&mut session, title_override.as_deref());
        if let Err(e) = session.save() {
            crate::logging::warn(&format!("Failed to save session: {}", e));
        }
    }
    App::save_startup_message_for_session(session_id, startup_message);
}

pub(super) fn launch_prompt_in_new_session_local(
    app: &mut App,
    content: String,
    images: Vec<(String, String)>,
) -> anyhow::Result<bool> {
    let (session_id, session_name) = clone_session_for_prompt(app)?;
    App::save_startup_submission_for_session(&session_id, content, images);
    let exe = super::launch_client_executable();
    let cwd = active_working_dir(app)
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let socket = std::env::var("JCODE_SOCKET").ok();
    let opened = super::spawn_in_new_terminal(&exe, &session_id, &cwd, socket.as_deref())?;
    if opened {
        app.push_display_message(DisplayMessage::system(format!(
            "↗ Next prompt launched in **{}**.",
            session_name
        )));
        app.set_status_notice("Prompt launched in new session");
    } else {
        app.push_display_message(DisplayMessage::system(format!(
            "↗ New session **{}** created for the next prompt.\n\nNo terminal was opened automatically. Resume manually:\n```\njcode --resume {}\n```",
            session_name, session_id
        )));
        app.set_status_notice("Prompt session created");
    }
    Ok(opened)
}

fn launch_review_window_local(
    app: &mut App,
    session_title: &str,
    label: &str,
    startup_message: String,
    model_override: Option<String>,
    provider_key_override: Option<String>,
) -> anyhow::Result<bool> {
    let initial_model = model_override
        .clone()
        .unwrap_or_else(|| current_autoreview_model_summary(app));
    let (session_id, session_name) = clone_session_for_review(
        app,
        session_title,
        initial_model,
        provider_key_override.clone(),
    )?;
    prepare_review_spawned_session(
        &session_id,
        startup_message,
        model_override,
        provider_key_override,
        Some(session_title.to_string()),
        None,
    );
    let exe = super::launch_client_executable();
    let cwd = active_working_dir(app)
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let socket = std::env::var("JCODE_SOCKET").ok();
    let opened = super::spawn_in_new_terminal(&exe, &session_id, &cwd, socket.as_deref())?;
    if opened {
        app.push_display_message(DisplayMessage::system(format!(
            "🔍 {} launched in **{}**.",
            label, session_name
        )));
        app.set_status_notice(format!("{} launched", label));
    } else {
        app.push_display_message(DisplayMessage::system(format!(
            "🔍 {} session **{}** created.\n\nNo terminal was opened automatically. Resume manually:\n```\njcode --resume {}\n```",
            label, session_name, session_id
        )));
        app.set_status_notice(format!("{} session created", label));
    }
    Ok(opened)
}

fn launch_autoreview_window_local(app: &mut App) -> anyhow::Result<bool> {
    let parent_session_id = current_feedback_target_session_id(app);
    launch_review_window_local(
        app,
        "autoreview",
        "Autoreview",
        build_autoreview_startup_message(&parent_session_id),
        current_autoreview_model_override(),
        None,
    )
}

fn launch_review_once_local(app: &mut App) -> anyhow::Result<bool> {
    let (model_override, provider_key_override) = current_review_model_override();
    let parent_session_id = current_feedback_target_session_id(app);
    launch_review_window_local(
        app,
        "review",
        "Review",
        build_review_startup_message(&parent_session_id),
        model_override,
        provider_key_override,
    )
}

fn launch_autojudge_window_local(app: &mut App) -> anyhow::Result<bool> {
    let parent_session_id = current_feedback_target_session_id(app);
    launch_review_window_local(
        app,
        "autojudge",
        "Autojudge",
        build_autojudge_startup_message(&parent_session_id),
        current_autojudge_model_override(),
        None,
    )
}

fn launch_judge_once_local(app: &mut App) -> anyhow::Result<bool> {
    let (model_override, provider_key_override) = current_judge_model_override();
    let parent_session_id = current_feedback_target_session_id(app);
    launch_review_window_local(
        app,
        "judge",
        "Judge",
        build_judge_startup_message(&parent_session_id),
        model_override,
        provider_key_override,
    )
}

pub(super) fn queue_review_spawn_remote(
    app: &mut App,
    label: &str,
    parent_session_id: String,
    startup_message: String,
    model_override: Option<String>,
    provider_key_override: Option<String>,
) {
    app.pending_split_parent_session_id = Some(parent_session_id);
    app.pending_split_startup_message = Some(startup_message);
    app.pending_split_model_override = model_override;
    app.pending_split_provider_key_override = provider_key_override;
    app.pending_split_label = Some(label.to_string());
    app.pending_split_started_at = Some(Instant::now());
    app.pending_split_request = true;
    app.set_status_notice(format!("{} queued", label));
}

#[cfg(test)]
pub(super) fn queue_autojudge_remote(app: &mut App) {
    if !app.autojudge_enabled
        || app.pending_split_request
        || app.pending_split_startup_message.is_some()
    {
        return;
    }
    let parent_session_id = current_feedback_target_session_id(app);
    queue_review_spawn_remote(
        app,
        "Autojudge",
        parent_session_id.clone(),
        build_autojudge_startup_message(&parent_session_id),
        current_autojudge_model_override(),
        None,
    );
}

pub(super) fn maybe_trigger_autoreview_local(app: &mut App) {
    if !app.autoreview_enabled || app.is_remote || app.is_replay {
        return;
    }
    if let Err(error) = launch_autoreview_window_local(app) {
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to launch autoreview: {}",
            error
        )));
        app.set_status_notice("Autoreview launch failed");
    }
}

pub(super) fn maybe_trigger_autojudge_local(app: &mut App) {
    if !app.autojudge_enabled || app.is_remote || app.is_replay {
        return;
    }
    if let Err(error) = launch_autojudge_window_local(app) {
        app.push_display_message(DisplayMessage::error(format!(
            "Failed to launch autojudge: {}",
            error
        )));
        app.set_status_notice("Autojudge launch failed");
    }
}

pub(super) fn handle_review_command_local(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/review") {
        return false;
    }

    let rest = trimmed.strip_prefix("/review").unwrap_or_default().trim();

    if rest.is_empty() {
        if let Err(error) = launch_review_once_local(app) {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to launch review: {}",
                error
            )));
            app.set_status_notice("Review launch failed");
        }
        return true;
    }

    app.push_display_message(DisplayMessage::error("Usage: `/review`".to_string()));
    true
}

pub(super) fn handle_autoreview_command_local(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/autoreview") {
        return false;
    }

    let rest = trimmed
        .strip_prefix("/autoreview")
        .unwrap_or_default()
        .trim();

    if rest.is_empty() || matches!(rest, "status" | "show") {
        app.push_display_message(DisplayMessage::system(autoreview_status_message(app)));
        return true;
    }

    match rest {
        "on" => {
            app.set_autoreview_feature_enabled(true);
            if let Err(e) = app.session.save() {
                crate::logging::warn(&format!("Failed to save session: {}", e));
            }
            app.push_display_message(DisplayMessage::system(
                "Autoreview enabled for this session.".to_string(),
            ));
            app.set_status_notice("Autoreview: ON");
            true
        }
        "off" => {
            app.set_autoreview_feature_enabled(false);
            if let Err(e) = app.session.save() {
                crate::logging::warn(&format!("Failed to save session: {}", e));
            }
            app.push_display_message(DisplayMessage::system(
                "Autoreview disabled for this session.".to_string(),
            ));
            app.set_status_notice("Autoreview: OFF");
            true
        }
        "now" => {
            if let Err(error) = launch_autoreview_window_local(app) {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to launch autoreview: {}",
                    error
                )));
                app.set_status_notice("Autoreview launch failed");
            }
            true
        }
        _ => {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/autoreview [on|off|status|now]`".to_string(),
            ));
            true
        }
    }
}

pub(super) fn handle_judge_command_local(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/judge") {
        return false;
    }

    let rest = trimmed.strip_prefix("/judge").unwrap_or_default().trim();

    if rest.is_empty() {
        if let Err(error) = launch_judge_once_local(app) {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to launch judge: {}",
                error
            )));
            app.set_status_notice("Judge launch failed");
        }
        return true;
    }

    app.push_display_message(DisplayMessage::error("Usage: `/judge`".to_string()));
    true
}

pub(super) fn handle_autojudge_command_local(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/autojudge") {
        return false;
    }

    let rest = trimmed
        .strip_prefix("/autojudge")
        .unwrap_or_default()
        .trim();

    if rest.is_empty() || matches!(rest, "status" | "show") {
        app.push_display_message(DisplayMessage::system(autojudge_status_message(app)));
        return true;
    }

    match rest {
        "on" => {
            app.set_autojudge_feature_enabled(true);
            if let Err(e) = app.session.save() {
                crate::logging::warn(&format!("Failed to save session: {}", e));
            }
            app.push_display_message(DisplayMessage::system(
                "Autojudge enabled for this session.".to_string(),
            ));
            app.set_status_notice("Autojudge: ON");
            true
        }
        "off" => {
            app.set_autojudge_feature_enabled(false);
            if let Err(e) = app.session.save() {
                crate::logging::warn(&format!("Failed to save session: {}", e));
            }
            app.push_display_message(DisplayMessage::system(
                "Autojudge disabled for this session.".to_string(),
            ));
            app.set_status_notice("Autojudge: OFF");
            true
        }
        "now" => {
            if let Err(error) = launch_autojudge_window_local(app) {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to launch autojudge: {}",
                    error
                )));
                app.set_status_notice("Autojudge launch failed");
            }
            true
        }
        _ => {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/autojudge [on|off|status|now]`".to_string(),
            ));
            true
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManualSubagentSpec {
    pub(super) subagent_type: String,
    pub(super) model: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImproveCommand {
    Run {
        plan_only: bool,
        focus: Option<String>,
    },
    Resume,
    Status,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RefactorCommand {
    Run {
        plan_only: bool,
        focus: Option<String>,
    },
    Resume,
    Status,
    Stop,
}
