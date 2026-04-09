use super::{App, DisplayMessage, ImproveMode, ProcessingStatus};
use crate::bus::{Bus, BusEvent, ManualToolCompleted, ToolEvent, ToolStatus};
use crate::id;
use crate::message::{ContentBlock, Message, Role, ToolCall};
use crate::session::{Session, StoredMessage};
use std::path::PathBuf;
use std::time::Instant;

const BTW_PAGE_ID: &str = "btw";
const REVIEW_PREFERRED_MODEL: &str = "gpt-5.4";

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
    let _ = app.session.save();
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
    session.autoreview_enabled = Some(app.autoreview_enabled);
    session.autojudge_enabled = Some(app.autojudge_enabled);
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

fn handle_observe_command(app: &mut App, trimmed: &str) -> bool {
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
{}\
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
        format!(
            "{}{}",
            judge_session_visible_context_notice(),
            review_session_read_only_guardrails()
        ),
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
    let mut child = Session::create(
        Some(active_session_id(app)),
        Some(session_title.to_string()),
    );
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
) {
    if let Ok(mut session) = crate::session::Session::load(session_id) {
        session.autoreview_enabled = Some(false);
        session.autojudge_enabled = Some(false);
        if let Some(title) = title_override {
            session.title = Some(title);
        }
        if let Some(model) = model_override {
            session.model = Some(model);
        }
        if provider_key_override.is_some() {
            session.provider_key = provider_key_override;
        }
        let _ = session.save();
    }
    App::save_startup_message_for_session(session_id, startup_message);
}

pub(super) fn prepare_autoreview_spawned_session(session_id: &str, startup_message: String) {
    prepare_review_spawned_session(
        session_id,
        startup_message,
        current_autoreview_model_override(),
        None,
        Some("autoreview".to_string()),
    );
}

pub(super) fn prepare_autojudge_spawned_session(session_id: &str, startup_message: String) {
    prepare_review_spawned_session(
        session_id,
        startup_message,
        current_autojudge_model_override(),
        None,
        Some("autojudge".to_string()),
    );
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
    launch_review_window_local(
        app,
        "autoreview",
        "Autoreview",
        build_autoreview_startup_message(&active_session_id(app)),
        current_autoreview_model_override(),
        None,
    )
}

fn launch_review_once_local(app: &mut App) -> anyhow::Result<bool> {
    let (model_override, provider_key_override) = current_review_model_override();
    launch_review_window_local(
        app,
        "review",
        "Review",
        build_review_startup_message(&active_session_id(app)),
        model_override,
        provider_key_override,
    )
}

fn launch_autojudge_window_local(app: &mut App) -> anyhow::Result<bool> {
    launch_review_window_local(
        app,
        "autojudge",
        "Autojudge",
        build_autojudge_startup_message(&active_session_id(app)),
        current_autojudge_model_override(),
        None,
    )
}

fn launch_judge_once_local(app: &mut App) -> anyhow::Result<bool> {
    let (model_override, provider_key_override) = current_judge_model_override();
    launch_review_window_local(
        app,
        "judge",
        "Judge",
        build_judge_startup_message(&active_session_id(app)),
        model_override,
        provider_key_override,
    )
}

pub(super) fn queue_review_spawn_remote(
    app: &mut App,
    label: &str,
    startup_message: String,
    model_override: Option<String>,
    provider_key_override: Option<String>,
) {
    app.pending_split_startup_message = Some(startup_message);
    app.pending_split_model_override = model_override;
    app.pending_split_provider_key_override = provider_key_override;
    app.pending_split_label = Some(label.to_string());
    app.pending_split_started_at = Some(Instant::now());
    app.pending_split_request = true;
    app.set_status_notice(format!("{} queued", label));
}

pub(super) fn queue_autoreview_remote(app: &mut App) {
    if !app.autoreview_enabled
        || app.pending_split_request
        || app.pending_split_startup_message.is_some()
    {
        return;
    }
    queue_review_spawn_remote(
        app,
        "Autoreview",
        build_autoreview_startup_message(&active_session_id(app)),
        current_autoreview_model_override(),
        None,
    );
}

pub(super) fn queue_autojudge_remote(app: &mut App) {
    if !app.autojudge_enabled
        || app.pending_split_request
        || app.pending_split_startup_message.is_some()
    {
        return;
    }
    queue_review_spawn_remote(
        app,
        "Autojudge",
        build_autojudge_startup_message(&active_session_id(app)),
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

fn handle_review_command_local(app: &mut App, trimmed: &str) -> bool {
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

fn handle_autoreview_command_local(app: &mut App, trimmed: &str) -> bool {
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
            let _ = app.session.save();
            app.push_display_message(DisplayMessage::system(
                "Autoreview enabled for this session.".to_string(),
            ));
            app.set_status_notice("Autoreview: ON");
            true
        }
        "off" => {
            app.set_autoreview_feature_enabled(false);
            let _ = app.session.save();
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

fn handle_judge_command_local(app: &mut App, trimmed: &str) -> bool {
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

fn handle_autojudge_command_local(app: &mut App, trimmed: &str) -> bool {
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
            let _ = app.session.save();
            app.push_display_message(DisplayMessage::system(
                "Autojudge enabled for this session.".to_string(),
            ));
            app.set_status_notice("Autojudge: ON");
            true
        }
        "off" => {
            app.set_autojudge_feature_enabled(false);
            let _ = app.session.save();
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

pub(super) fn improve_usage() -> &'static str {
    "Usage: `/improve [focus]`, `/improve plan [focus]`, `/improve resume`, `/improve status`, or `/improve stop`"
}

pub(super) fn parse_improve_command(trimmed: &str) -> Option<Result<ImproveCommand, String>> {
    let rest = trimmed.strip_prefix("/improve")?.trim();
    if rest.is_empty() {
        return Some(Ok(ImproveCommand::Run {
            plan_only: false,
            focus: None,
        }));
    }

    if rest == "status" {
        return Some(Ok(ImproveCommand::Status));
    }

    if rest == "resume" {
        return Some(Ok(ImproveCommand::Resume));
    }

    if rest == "stop" {
        return Some(Ok(ImproveCommand::Stop));
    }

    if rest == "plan" {
        return Some(Ok(ImproveCommand::Run {
            plan_only: true,
            focus: None,
        }));
    }

    if let Some(focus) = rest.strip_prefix("plan ") {
        let focus = focus.trim();
        return Some(if focus.is_empty() {
            Err(improve_usage().to_string())
        } else {
            Ok(ImproveCommand::Run {
                plan_only: true,
                focus: Some(focus.to_string()),
            })
        });
    }

    if rest.starts_with("status ") || rest.starts_with("resume ") || rest.starts_with("stop ") {
        return Some(Err(improve_usage().to_string()));
    }

    Some(Ok(ImproveCommand::Run {
        plan_only: false,
        focus: Some(rest.to_string()),
    }))
}

pub(super) fn refactor_usage() -> &'static str {
    "Usage: `/refactor [focus]`, `/refactor plan [focus]`, `/refactor resume`, `/refactor status`, or `/refactor stop`"
}

pub(super) fn parse_refactor_command(trimmed: &str) -> Option<Result<RefactorCommand, String>> {
    let rest = trimmed.strip_prefix("/refactor")?.trim();
    if rest.is_empty() {
        return Some(Ok(RefactorCommand::Run {
            plan_only: false,
            focus: None,
        }));
    }

    if rest == "status" {
        return Some(Ok(RefactorCommand::Status));
    }

    if rest == "resume" {
        return Some(Ok(RefactorCommand::Resume));
    }

    if rest == "stop" {
        return Some(Ok(RefactorCommand::Stop));
    }

    if rest == "plan" {
        return Some(Ok(RefactorCommand::Run {
            plan_only: true,
            focus: None,
        }));
    }

    if let Some(focus) = rest.strip_prefix("plan ") {
        let focus = focus.trim();
        return Some(if focus.is_empty() {
            Err(refactor_usage().to_string())
        } else {
            Ok(RefactorCommand::Run {
                plan_only: true,
                focus: Some(focus.to_string()),
            })
        });
    }

    if rest.starts_with("status ") || rest.starts_with("resume ") || rest.starts_with("stop ") {
        return Some(Err(refactor_usage().to_string()));
    }

    Some(Ok(RefactorCommand::Run {
        plan_only: false,
        focus: Some(rest.to_string()),
    }))
}

pub(super) fn build_improve_prompt(plan_only: bool, focus: Option<&str>) -> String {
    let focus_line = focus
        .map(|focus| {
            format!(
                "\nFocus area: {}. Prefer this area when leverage is comparable, but you may choose a different task if it is clearly higher leverage.",
                focus.trim()
            )
        })
        .unwrap_or_default();

    if plan_only {
        format!(
            "You are entering improvement planning mode for this repository.\n\
Your job is to inspect the project and identify the highest-leverage improvements worth doing next.\n\
\n\
First inspect the codebase and current repo state. Then write a concise ranked todo list using `todowrite` with the best 3-7 candidate improvements. Prefer work that is high-impact, low-risk, and easy to validate. Consider refactors, reliability issues, missing tests, UX papercuts, docs gaps, startup/runtime performance, and profiling opportunities.\n\
\n\
This is plan-only mode: do not edit files, write patches, or otherwise modify source code or git state. Read/search/analyze freely, and you may run builds/tests/profiling commands if that helps you rank the work, but stop after presenting the todo list and brief rationale.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and busywork. If the repo already has todos, replace them with a tighter ranked improve plan if appropriate.{}",
            focus_line,
        )
    } else {
        format!(
            "You are entering improvement mode for this repository.\n\
Your job is to identify and implement the highest-leverage safe improvements to this project, then reassess and continue only while further work is clearly worthwhile.\n\
\n\
First inspect the codebase and current repo state. Then write a concise ranked todo list using `todowrite` with the best 3-7 improvements to tackle next. Prefer work that is high-impact, low-risk, locally scoped, and easy to validate. Consider refactors, reliability issues, missing tests, UX papercuts, docs gaps, startup/runtime performance, and profiling opportunities.{}\n\
\n\
Execute the strongest items, updating the todo list as you go. Validate meaningful changes with builds, tests, or measurements. If you make performance claims, measure before and after when possible.\n\
\n\
After completing the batch, reassess. If strong opportunities remain, write a fresh todo list and continue. If remaining work has diminishing returns, stop and explain why the next ideas are not clearly worth the churn.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and busywork. Do not invent work just to stay busy. If the repo already has todos, refine or replace them with the best current improve batch before continuing.",
            focus_line,
        )
    }
}

pub(super) fn build_refactor_prompt(plan_only: bool, focus: Option<&str>) -> String {
    let focus_line = focus
        .map(|focus| {
            format!(
                "\nFocus area: {}. Prefer this area when leverage is comparable, but choose a different task if it is clearly higher leverage.",
                focus.trim()
            )
        })
        .unwrap_or_default();

    if plan_only {
        format!(
            "You are entering refactor planning mode for this repository.\n\
Your job is to inspect the project and identify the highest-leverage safe refactors worth doing next.\n\
\n\
First inspect the codebase, current repo state, and the in-repo quality docs if they exist, especially `docs/REFACTORING.md`, `docs/CODE_QUALITY_10_10_PLAN.md`, and `docs/CODE_QUALITY_TODO.md`. Then write a concise ranked todo list using `todowrite` with the best 3-7 candidate refactors. Prefer behavior-preserving extraction, file splits, dead-code deletion, warning reduction, test isolation, and clearer module boundaries.\n\
\n\
This is plan-only mode: do not edit files, write patches, or otherwise modify source code or git state. Read/search/analyze freely, and you may run builds/tests if that helps rank the work, but stop after presenting the ranked refactor plan and brief rationale.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and risky busywork. If the repo already has todos, tighten or replace them with the best current refactor plan.{}",
            focus_line,
        )
    } else {
        format!(
            "You are entering refactor mode for this repository.\n\
Your job is to move the codebase closer to a practical 10/10 by making the highest-leverage safe refactors, validating them, getting an independent review, and only continuing while the next batch is clearly worth the churn.\n\
\n\
First inspect the codebase, current repo state, and the in-repo quality docs if they exist, especially `docs/REFACTORING.md`, `docs/CODE_QUALITY_10_10_PLAN.md`, and `docs/CODE_QUALITY_TODO.md`. Then write a concise ranked todo list using `todowrite` with the best 3-7 refactors to tackle next. Prefer behavior-preserving extraction, splitting oversized modules, dead-code deletion, warning reduction, test improvements, and boundary clarification.{}\n\
\n\
For v1, do the implementation work yourself in this main session. Do not create a swarm for ordinary execution. Keep changes locally scoped and easy to validate.\n\
\n\
After each meaningful batch, use the `subagent` tool exactly once to launch an independent read-only reviewer. In that subagent prompt, explicitly forbid file edits, patch application, and git changes. Ask it to inspect the changed areas plus nearby tests and report concrete regressions, risks, abstraction problems, or follow-up refactors. Incorporate valid findings before continuing.\n\
\n\
Validate each meaningful batch with relevant builds, tests, or repo verification scripts. Prefer behavior-preserving changes first. After the batch and independent review, reassess. If strong refactors remain, write a fresh todo list and continue. If remaining work has diminishing returns or becomes too risky, stop and explain why.\n\
\n\
Avoid broad speculative rewrites, cosmetic churn, and busywork. Do not invent work just to stay busy.",
            focus_line,
        )
    }
}

pub(super) fn improve_mode_for(plan_only: bool) -> ImproveMode {
    if plan_only {
        ImproveMode::ImprovePlan
    } else {
        ImproveMode::ImproveRun
    }
}

pub(super) fn refactor_mode_for(plan_only: bool) -> ImproveMode {
    if plan_only {
        ImproveMode::RefactorPlan
    } else {
        ImproveMode::RefactorRun
    }
}

pub(super) fn session_improve_mode_for(mode: ImproveMode) -> crate::session::SessionImproveMode {
    match mode {
        ImproveMode::ImproveRun => crate::session::SessionImproveMode::ImproveRun,
        ImproveMode::ImprovePlan => crate::session::SessionImproveMode::ImprovePlan,
        ImproveMode::RefactorRun => crate::session::SessionImproveMode::RefactorRun,
        ImproveMode::RefactorPlan => crate::session::SessionImproveMode::RefactorPlan,
    }
}

pub(super) fn restore_improve_mode(mode: crate::session::SessionImproveMode) -> ImproveMode {
    match mode {
        crate::session::SessionImproveMode::ImproveRun => ImproveMode::ImproveRun,
        crate::session::SessionImproveMode::ImprovePlan => ImproveMode::ImprovePlan,
        crate::session::SessionImproveMode::RefactorRun => ImproveMode::RefactorRun,
        crate::session::SessionImproveMode::RefactorPlan => ImproveMode::RefactorPlan,
    }
}

pub(super) fn improve_launch_notice(
    plan_only: bool,
    focus: Option<&str>,
    interrupted: bool,
) -> String {
    let action = if plan_only {
        "improvement plan"
    } else {
        "improvement loop"
    };
    let prefix = if interrupted {
        "👉 Interrupting and starting"
    } else {
        "🚀 Starting"
    };
    match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        Some(focus) => format!("{} {} focused on **{}**...", prefix, action, focus),
        None => format!("{} {}...", prefix, action),
    }
}

pub(super) fn improve_stop_notice(interrupted: bool) -> String {
    if interrupted {
        "🛑 Interrupting and stopping the improve loop at the next safe point...".to_string()
    } else {
        "🛑 Stopping the improve loop after the next safe point...".to_string()
    }
}

pub(super) fn improve_stop_prompt() -> String {
    "Stop improvement mode after the current safe point. Do not start a new improve batch. Update the todo list so it accurately reflects what is completed, cancelled, or still pending, and then summarize what remains plus why you stopped.".to_string()
}

pub(super) fn refactor_launch_notice(
    plan_only: bool,
    focus: Option<&str>,
    interrupted: bool,
) -> String {
    let action = if plan_only {
        "refactor plan"
    } else {
        "refactor loop"
    };
    let prefix = if interrupted {
        "👉 Interrupting and starting"
    } else {
        "🚀 Starting"
    };
    match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        Some(focus) => format!("{} {} focused on **{}**...", prefix, action, focus),
        None => format!("{} {}...", prefix, action),
    }
}

pub(super) fn refactor_stop_notice(interrupted: bool) -> String {
    if interrupted {
        "🛑 Interrupting and stopping the refactor loop at the next safe point...".to_string()
    } else {
        "🛑 Stopping the refactor loop after the next safe point...".to_string()
    }
}

pub(super) fn refactor_stop_prompt() -> String {
    "Stop refactor mode after the current safe point. Do not start a new refactor batch. Update the todo list so it accurately reflects what is completed, cancelled, or still pending, note any remaining high-value refactors, and summarize why you stopped. If you finished a meaningful code batch without yet running the independent read-only review subagent, run that review before stopping.".to_string()
}

pub(super) fn build_improve_resume_prompt(
    mode: ImproveMode,
    incomplete: &[&crate::todo::TodoItem],
) -> String {
    if incomplete.is_empty() {
        return match mode {
            ImproveMode::ImproveRun => "Resume improvement mode for this repository. Start by inspecting the current repo state, writing or refreshing a ranked todo list with `todowrite`, then continue implementing the highest-leverage safe improvements until the next ideas have diminishing returns.".to_string(),
            ImproveMode::ImprovePlan => "Resume improvement planning mode for this repository. Reinspect the current repo state, refresh the ranked improve todo list with `todowrite`, and stop after presenting the updated plan without editing files.".to_string(),
            ImproveMode::RefactorRun | ImproveMode::RefactorPlan => {
                "Resume improvement mode for this repository by first writing an improve-oriented todo list with `todowrite`, then continue only with high-leverage safe improvements.".to_string()
            }
        };
    }

    let mut todo_list = String::new();
    for todo in incomplete {
        let icon = if todo.status == "in_progress" {
            "🔄"
        } else {
            "⬜"
        };
        todo_list.push_str(&format!(
            "  {} [{}] {}\n",
            icon, todo.priority, todo.content
        ));
    }

    match mode {
        ImproveMode::ImproveRun => format!(
            "Resume improvement mode. Your current improve todo list still has {} incomplete item{}:\n\n{}\nContinue the highest-leverage work, keep the todo list accurate with `todowrite`, validate meaningful changes, and once this batch is done reassess whether another batch is still worth doing.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::ImprovePlan => format!(
            "Resume improvement planning mode. The current improve todo list has {} pending item{}:\n\n{}\nRefresh or tighten this plan using `todowrite`, keeping it ranked and concrete, then stop without editing files.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::RefactorRun | ImproveMode::RefactorPlan => format!(
            "Resume improvement mode with these incomplete items:\n\n{}\nContinue only the highest-leverage safe improvements and keep the todo list accurate with `todowrite`.",
            todo_list,
        ),
    }
}

pub(super) fn build_refactor_resume_prompt(
    mode: ImproveMode,
    incomplete: &[&crate::todo::TodoItem],
) -> String {
    if incomplete.is_empty() {
        return match mode {
            ImproveMode::RefactorRun => "Resume refactor mode for this repository. Start by inspecting the current repo state and relevant quality docs, write or refresh a ranked refactor todo list with `todowrite`, implement the highest-leverage safe refactors yourself, validate them, run an independent read-only review subagent after each meaningful batch, and continue only while more work is clearly worth the churn.".to_string(),
            ImproveMode::RefactorPlan => "Resume refactor planning mode for this repository. Reinspect the current repo state and quality docs, refresh the ranked refactor todo list with `todowrite`, and stop after presenting the updated plan without editing files.".to_string(),
            ImproveMode::ImproveRun | ImproveMode::ImprovePlan => {
                "Resume refactor mode for this repository by first producing a ranked refactor todo list with `todowrite`, then continue only with high-leverage safe refactors.".to_string()
            }
        };
    }

    let mut todo_list = String::new();
    for todo in incomplete {
        let icon = if todo.status == "in_progress" {
            "🔄"
        } else {
            "⬜"
        };
        todo_list.push_str(&format!(
            "  {} [{}] {}\n",
            icon, todo.priority, todo.content
        ));
    }

    match mode {
        ImproveMode::RefactorRun => format!(
            "Resume refactor mode. Your current refactor todo list still has {} incomplete item{}:\n\n{}\nContinue the highest-leverage safe refactors yourself in this session, keep the todo list accurate with `todowrite`, validate meaningful changes, run one independent read-only review subagent after each meaningful batch, and then reassess whether another batch is still worth doing.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::RefactorPlan => format!(
            "Resume refactor planning mode. The current refactor todo list has {} pending item{}:\n\n{}\nRefresh or tighten this plan using `todowrite`, keeping it ranked and concrete, then stop without editing files.",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        ),
        ImproveMode::ImproveRun | ImproveMode::ImprovePlan => format!(
            "Resume refactor mode with these incomplete items:\n\n{}\nConvert them into the best current refactor batch, then continue only with high-leverage safe refactors.",
            todo_list,
        ),
    }
}

fn current_mode_for(app: &App, predicate: impl Fn(ImproveMode) -> bool) -> Option<ImproveMode> {
    app.improve_mode
        .or_else(|| app.session.improve_mode.map(restore_improve_mode))
        .filter(|mode| predicate(*mode))
}

fn persist_improve_mode_local(app: &mut App, mode: Option<ImproveMode>) {
    app.improve_mode = mode;
    app.session.improve_mode = mode.map(session_improve_mode_for);
    let _ = app.session.save();
}

fn start_synthetic_user_turn(app: &mut App, content: String) {
    app.commit_pending_streaming_assistant_message();
    app.add_provider_message(Message::user(&content));
    app.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: content,
            cache_control: None,
        }],
    );
    let _ = app.session.save();

    app.is_processing = true;
    app.status = ProcessingStatus::Sending;
    app.clear_streaming_render_state();
    app.stream_buffer.clear();
    app.thought_line_inserted = false;
    app.thinking_prefix_emitted = false;
    app.thinking_buffer.clear();
    app.streaming_tool_calls.clear();
    app.batch_progress = None;
    app.streaming_input_tokens = 0;
    app.streaming_output_tokens = 0;
    app.streaming_cache_read_tokens = None;
    app.streaming_cache_creation_tokens = None;
    app.upstream_provider = None;
    app.status_detail = None;
    app.streaming_tps_start = None;
    app.streaming_tps_elapsed = std::time::Duration::ZERO;
    app.streaming_tps_collect_output = false;
    app.streaming_total_output_tokens = 0;
    app.processing_started = Some(Instant::now());
    app.pending_turn = true;
}

fn interrupt_and_queue_synthetic_message(
    app: &mut App,
    content: String,
    status_notice: &str,
    display_notice: String,
) {
    app.cancel_requested = true;
    app.interleave_message = None;
    app.pending_soft_interrupts.clear();
    app.pending_soft_interrupt_requests.clear();
    app.set_status_notice(status_notice);
    app.push_display_message(DisplayMessage::system(display_notice));
    app.queued_messages.push(content);
}

pub(super) fn format_improve_status(app: &App) -> String {
    let session_id = active_session_id(app);
    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let cancelled = todos.iter().filter(|t| t.status == "cancelled").count();
    let incomplete: Vec<_> = todos
        .iter()
        .filter(|t| t.status != "completed" && t.status != "cancelled")
        .collect();

    let phase = if app.is_processing {
        if current_mode_for(app, ImproveMode::is_improve).is_some() || !incomplete.is_empty() {
            "running"
        } else {
            "busy (no improve batch detected yet)"
        }
    } else if !incomplete.is_empty() {
        "paused / resumable"
    } else if completed > 0 || cancelled > 0 {
        "idle (last improve batch finished)"
    } else {
        "idle"
    };

    let mode = current_mode_for(app, ImproveMode::is_improve)
        .map(|mode| mode.status_label())
        .unwrap_or("not yet started in this session");

    let mut lines = vec![
        format!("Improve status: **{}**", phase),
        format!("Last requested mode: **{}**", mode),
        format!(
            "Todos: {} incomplete · {} completed · {} cancelled",
            incomplete.len(),
            completed,
            cancelled
        ),
    ];

    if !incomplete.is_empty() {
        lines.push(String::new());
        lines.push("Current improve batch:".to_string());
        for todo in incomplete.iter().take(5) {
            let icon = if todo.status == "in_progress" {
                "🔄"
            } else {
                "⬜"
            };
            lines.push(format!("- {} [{}] {}", icon, todo.priority, todo.content));
        }
        if incomplete.len() > 5 {
            lines.push(format!("- …and {} more", incomplete.len() - 5));
        }
    } else {
        lines.push(String::new());
        lines.push("No current improve todo batch for this session.".to_string());
    }

    lines.push(String::new());
    lines.push("Use `/improve` to start/continue, `/improve resume` to continue the last saved mode, `/improve plan` for plan-only mode, or `/improve stop` to halt after a safe point.".to_string());
    lines.join("\n")
}

pub(super) fn format_refactor_status(app: &App) -> String {
    let session_id = active_session_id(app);
    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
    let completed = todos.iter().filter(|t| t.status == "completed").count();
    let cancelled = todos.iter().filter(|t| t.status == "cancelled").count();
    let incomplete: Vec<_> = todos
        .iter()
        .filter(|t| t.status != "completed" && t.status != "cancelled")
        .collect();

    let phase = if app.is_processing {
        if current_mode_for(app, ImproveMode::is_refactor).is_some() || !incomplete.is_empty() {
            "running"
        } else {
            "busy (no refactor batch detected yet)"
        }
    } else if !incomplete.is_empty() {
        "paused / resumable"
    } else if completed > 0 || cancelled > 0 {
        "idle (last refactor batch finished)"
    } else {
        "idle"
    };

    let mode = current_mode_for(app, ImproveMode::is_refactor)
        .map(|mode| mode.status_label())
        .unwrap_or("not yet started in this session");

    let mut lines = vec![
        format!("Refactor status: **{}**", phase),
        format!("Last requested mode: **{}**", mode),
        format!(
            "Todos: {} incomplete · {} completed · {} cancelled",
            incomplete.len(),
            completed,
            cancelled
        ),
    ];

    if !incomplete.is_empty() {
        lines.push(String::new());
        lines.push("Current refactor batch:".to_string());
        for todo in incomplete.iter().take(5) {
            let icon = if todo.status == "in_progress" {
                "🔄"
            } else {
                "⬜"
            };
            lines.push(format!("- {} [{}] {}", icon, todo.priority, todo.content));
        }
        if incomplete.len() > 5 {
            lines.push(format!("- …and {} more", incomplete.len() - 5));
        }
    } else {
        lines.push(String::new());
        lines.push("No current refactor todo batch for this session.".to_string());
    }

    lines.push(String::new());
    lines.push("Use `/refactor` to start/continue, `/refactor resume` to continue the last saved mode, `/refactor plan` for plan-only mode, or `/refactor stop` to halt after a safe point.".to_string());
    lines.join("\n")
}

fn handle_improve_command_local(app: &mut App, command: ImproveCommand) {
    match command {
        ImproveCommand::Resume => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let incomplete: Vec<_> = todos
                .iter()
                .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                .collect();

            let mode = current_mode_for(app, ImproveMode::is_improve);
            let Some(mode) = mode else {
                app.push_display_message(DisplayMessage::system(
                    "No saved improve run found for this session. Use `/improve` or `/improve plan` to start one."
                        .to_string(),
                ));
                return;
            };

            persist_improve_mode_local(app, Some(mode));
            let prompt = build_improve_resume_prompt(mode, &incomplete);
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    "Interrupting for /improve resume...",
                    improve_launch_notice(matches!(mode, ImproveMode::ImprovePlan), None, true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(format!(
                    "♻️ Resuming {}...",
                    mode.status_label()
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
        ImproveCommand::Status => {
            app.push_display_message(DisplayMessage::system(format_improve_status(app)));
        }
        ImproveCommand::Stop => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let has_incomplete = todos
                .iter()
                .any(|todo| todo.status != "completed" && todo.status != "cancelled");

            if current_mode_for(app, ImproveMode::is_improve).is_none()
                && !app.is_processing
                && !has_incomplete
            {
                app.push_display_message(DisplayMessage::system(
                    "No active improve loop to stop. Use `/improve` to start one.".to_string(),
                ));
                return;
            }

            persist_improve_mode_local(app, None);
            let stop_prompt = improve_stop_prompt();
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    stop_prompt,
                    "Interrupting for /improve stop...",
                    improve_stop_notice(true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(improve_stop_notice(false)));
                start_synthetic_user_turn(app, stop_prompt);
            }
        }
        ImproveCommand::Run { plan_only, focus } => {
            let mode = improve_mode_for(plan_only);
            persist_improve_mode_local(app, Some(mode));
            let prompt = build_improve_prompt(plan_only, focus.as_deref());
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    if plan_only {
                        "Interrupting for /improve plan..."
                    } else {
                        "Interrupting for /improve..."
                    },
                    improve_launch_notice(plan_only, focus.as_deref(), true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(improve_launch_notice(
                    plan_only,
                    focus.as_deref(),
                    false,
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
    }
}

fn handle_refactor_command_local(app: &mut App, command: RefactorCommand) {
    match command {
        RefactorCommand::Resume => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let incomplete: Vec<_> = todos
                .iter()
                .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                .collect();

            let mode = current_mode_for(app, ImproveMode::is_refactor);
            let Some(mode) = mode else {
                app.push_display_message(DisplayMessage::system(
                    "No saved refactor run found for this session. Use `/refactor` or `/refactor plan` to start one."
                        .to_string(),
                ));
                return;
            };

            persist_improve_mode_local(app, Some(mode));
            let prompt = build_refactor_resume_prompt(mode, &incomplete);
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    "Interrupting for /refactor resume...",
                    refactor_launch_notice(matches!(mode, ImproveMode::RefactorPlan), None, true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(format!(
                    "♻️ Resuming {}...",
                    mode.status_label()
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
        RefactorCommand::Status => {
            app.push_display_message(DisplayMessage::system(format_refactor_status(app)));
        }
        RefactorCommand::Stop => {
            let session_id = active_session_id(app);
            let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
            let has_incomplete = todos
                .iter()
                .any(|todo| todo.status != "completed" && todo.status != "cancelled");

            if current_mode_for(app, ImproveMode::is_refactor).is_none()
                && !app.is_processing
                && !has_incomplete
            {
                app.push_display_message(DisplayMessage::system(
                    "No active refactor loop to stop. Use `/refactor` to start one.".to_string(),
                ));
                return;
            }

            persist_improve_mode_local(app, None);
            let stop_prompt = refactor_stop_prompt();
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    stop_prompt,
                    "Interrupting for /refactor stop...",
                    refactor_stop_notice(true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(refactor_stop_notice(false)));
                start_synthetic_user_turn(app, stop_prompt);
            }
        }
        RefactorCommand::Run { plan_only, focus } => {
            let mode = refactor_mode_for(plan_only);
            persist_improve_mode_local(app, Some(mode));
            let prompt = build_refactor_prompt(plan_only, focus.as_deref());
            if app.is_processing {
                interrupt_and_queue_synthetic_message(
                    app,
                    prompt,
                    if plan_only {
                        "Interrupting for /refactor plan..."
                    } else {
                        "Interrupting for /refactor..."
                    },
                    refactor_launch_notice(plan_only, focus.as_deref(), true),
                );
            } else {
                app.push_display_message(DisplayMessage::system(refactor_launch_notice(
                    plan_only,
                    focus.as_deref(),
                    false,
                )));
                start_synthetic_user_turn(app, prompt);
            }
        }
    }
}

pub(super) fn current_subagent_model_summary(app: &App) -> String {
    match app.session.subagent_model.as_deref() {
        Some(model) => format!("fixed `{}`", model),
        None => format!("inherit current (`{}`)", app.provider.model()),
    }
}

fn derive_subagent_description(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(4).collect();
    if words.is_empty() {
        "Manual subagent".to_string()
    } else {
        words.join(" ")
    }
}

pub(super) fn parse_manual_subagent_spec(rest: &str) -> Result<ManualSubagentSpec, String> {
    let mut iter = rest.split_whitespace().peekable();
    let mut subagent_type = "general".to_string();
    let mut model = None;
    let mut session_id = None;
    let mut prompt_tokens = Vec::new();

    while let Some(token) = iter.next() {
        match token {
            "--type" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing value for `--type`.".to_string())?;
                subagent_type = value.to_string();
            }
            "--model" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing value for `--model`.".to_string())?;
                model = Some(value.to_string());
            }
            "--continue" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing value for `--continue`.".to_string())?;
                session_id = Some(value.to_string());
            }
            flag if flag.starts_with("--") => {
                return Err(format!("Unknown flag `{}`.", flag));
            }
            prompt_start => {
                prompt_tokens.push(prompt_start.to_string());
                prompt_tokens.extend(iter.map(str::to_string));
                break;
            }
        }
    }

    let prompt = prompt_tokens.join(" ").trim().to_string();
    if prompt.is_empty() {
        return Err("Missing prompt. Add text after `/subagent`.".to_string());
    }

    Ok(ManualSubagentSpec {
        subagent_type,
        model,
        session_id,
        prompt,
    })
}

fn launch_manual_subagent(app: &mut App, spec: ManualSubagentSpec) {
    let description = derive_subagent_description(&spec.prompt);
    let tool_call = crate::message::ToolCall {
        id: id::new_id("call"),
        name: "subagent".to_string(),
        input: serde_json::json!({
            "description": description,
            "prompt": spec.prompt,
            "subagent_type": spec.subagent_type,
            "model": spec.model,
            "session_id": spec.session_id,
            "command": "/subagent",
        }),
        intent: None,
    };

    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: tool_call.name.clone(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call.clone()),
    });

    let content_blocks = vec![ContentBlock::ToolUse {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        input: tool_call.input.clone(),
    }];
    app.add_provider_message(Message {
        role: Role::Assistant,
        content: content_blocks.clone(),
        timestamp: Some(chrono::Utc::now()),
        tool_duration_ms: None,
    });
    let message_id = app.session.add_message(Role::Assistant, content_blocks);
    let _ = app.session.save();
    app.subagent_status = Some("starting subagent".to_string());
    app.set_status_notice("Running subagent");

    let registry = app.registry.clone();
    let session_id = app.session.id.clone();
    let working_dir = app.session.working_dir.clone();
    let tool_call_for_task = tool_call.clone();
    tokio::spawn(async move {
        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            tool_call_id: tool_call_for_task.id.clone(),
            tool_name: tool_call_for_task.name.clone(),
            status: ToolStatus::Running,
            title: None,
        }));

        let ctx = crate::tool::ToolContext {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            tool_call_id: tool_call_for_task.id.clone(),
            working_dir: working_dir.as_deref().map(PathBuf::from),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };

        let start = Instant::now();
        let result = registry
            .execute(
                &tool_call_for_task.name,
                tool_call_for_task.input.clone(),
                ctx,
            )
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let (output, is_error, title, status) = match result {
            Ok(output) => {
                crate::telemetry::record_tool_call();
                (output.output, false, output.title, ToolStatus::Completed)
            }
            Err(error) => {
                crate::telemetry::record_tool_failure();
                (format!("Error: {}", error), true, None, ToolStatus::Error)
            }
        };

        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
            session_id: session_id.clone(),
            message_id,
            tool_call_id: tool_call_for_task.id.clone(),
            tool_name: tool_call_for_task.name.clone(),
            status,
            title: title.clone(),
        }));

        Bus::global().publish(BusEvent::ManualToolCompleted(ManualToolCompleted {
            session_id,
            tool_call: tool_call_for_task,
            output,
            is_error,
            title,
            duration_ms,
        }));
    });
}

fn handle_subagent_model_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/subagent-model") {
        return false;
    }

    if app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/subagent-model` requires a live jcode server connection in remote mode.".to_string(),
        ));
        return true;
    }

    let rest = trimmed
        .strip_prefix("/subagent-model")
        .unwrap_or_default()
        .trim();

    if rest.is_empty() || matches!(rest, "show" | "status") {
        app.push_display_message(DisplayMessage::system(format!(
            "Subagent model for this session: {}\n\nUse `/subagent-model <name>` to pin a model, or `/subagent-model inherit` to use the current model.",
            current_subagent_model_summary(app)
        )));
        return true;
    }

    if matches!(rest, "inherit" | "reset" | "clear") {
        app.session.subagent_model = None;
        let _ = app.session.save();
        app.push_display_message(DisplayMessage::system(format!(
            "Subagent model reset to inherit the current model (`{}`).",
            app.provider.model()
        )));
        app.set_status_notice("Subagent model: inherit");
        return true;
    }

    app.session.subagent_model = Some(rest.to_string());
    let _ = app.session.save();
    app.push_display_message(DisplayMessage::system(format!(
        "Subagent model pinned to `{}` for this session.",
        rest
    )));
    app.set_status_notice(format!("Subagent model → {}", rest));
    true
}

fn handle_subagent_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/subagent") || trimmed.starts_with("/subagent-model") {
        return false;
    }

    if app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/subagent` requires a live jcode server connection in remote mode.".to_string(),
        ));
        return true;
    }

    let rest = trimmed.strip_prefix("/subagent").unwrap_or_default().trim();
    if rest.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`"
                .to_string(),
        ));
        return true;
    }

    match parse_manual_subagent_spec(rest) {
        Ok(spec) => launch_manual_subagent(app, spec),
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "{}\nUsage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`",
                error
            )));
        }
    }
    true
}

pub(super) fn handle_help_command(app: &mut App, trimmed: &str) -> bool {
    if let Some(topic) = trimmed
        .strip_prefix("/help ")
        .or_else(|| trimmed.strip_prefix("/? "))
    {
        if let Some(help) = app.command_help(topic) {
            app.push_display_message(DisplayMessage::system(help));
        } else {
            app.push_display_message(DisplayMessage::error(format!(
                "Unknown command '{}'. Use `/help` to list commands.",
                topic.trim()
            )));
        }
        return true;
    }

    if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
        app.help_scroll = Some(0);
        return true;
    }

    false
}

fn build_btw_loading_markdown(question: &str) -> String {
    format!(
        "# `/btw`\n\n## Question\n{}\n\n## Status\nThinking…\n",
        question.trim()
    )
}

fn build_btw_system_reminder(question: &str) -> String {
    format!(
        "The user invoked `/btw`, which is a side question about the current session. \
Answer ONLY from the existing conversation/context already in memory for this session. \
Do not read files, run commands, search the web, or call any tool except `side_panel`.\n\n\
Use the `side_panel` tool exactly once with:\n\
- `action`: `write`\n\
- `page_id`: `{}`\n\
- `title`: ``/btw``\n\
- `focus`: `true`\n\n\
Write markdown with this shape:\n\
# `/btw`\n\
## Question\n<repeat the question>\n\
## Answer\n<your concise answer>\n\n\
If the answer is not already knowable from the current session context, say so clearly in the Answer section and explain that a normal prompt is needed.\n\n\
After writing the side panel content, do not add any normal chat response text.\n\n\
Question: {}",
        BTW_PAGE_ID,
        question.trim()
    )
}

fn handle_btw_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/btw") {
        return false;
    }

    let question = trimmed.strip_prefix("/btw").unwrap_or_default().trim();
    if question.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/btw <question>`".to_string(),
        ));
        return true;
    }

    match crate::side_panel::write_markdown_page(
        active_session_id(app).as_str(),
        BTW_PAGE_ID,
        Some("`/btw`"),
        &build_btw_loading_markdown(question),
        true,
    ) {
        Ok(snapshot) => app.set_side_panel_snapshot(snapshot),
        Err(error) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to prepare `/btw` side panel: {}",
                error
            )));
            return true;
        }
    }

    app.hidden_queued_system_messages
        .push(build_btw_system_reminder(question));
    if app.is_processing {
        app.push_display_message(DisplayMessage::system(
            "Queued `/btw` — answer will appear in the side panel after the current turn."
                .to_string(),
        ));
        app.set_status_notice("Queued /btw");
    } else {
        app.push_display_message(DisplayMessage::system(
            "Running `/btw` — answer will appear in the side panel.".to_string(),
        ));
        app.pending_queued_dispatch = true;
        app.set_status_notice("Running /btw");
    }

    true
}

fn load_catchup_candidates(app: &App) -> Vec<crate::tui::session_picker::SessionInfo> {
    let current_session_id = active_session_id(app);
    crate::tui::session_picker::load_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.id != current_session_id && session.needs_catchup)
        .collect()
}

fn handle_catchup_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/catchup") {
        return false;
    }
    if !app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/catchup` currently requires a connected shared server session.".to_string(),
        ));
        return true;
    }

    let rest = trimmed.strip_prefix("/catchup").unwrap_or_default().trim();
    match rest {
        "" | "list" | "show" => {
            app.open_catchup_picker();
            true
        }
        "next" => {
            if app.is_processing {
                app.set_status_notice("Finish current work before Catch Up");
                return true;
            }
            let candidates = load_catchup_candidates(app);
            let total = candidates.len();
            let Some(target) = candidates.first() else {
                app.push_display_message(DisplayMessage::system(
                    "No sessions currently need catch up.".to_string(),
                ));
                app.set_status_notice("Catch Up: none waiting");
                return true;
            };

            let source_session_id = active_session_id(app);
            let target_name = crate::id::extract_session_name(&target.id)
                .map(|name| name.to_string())
                .unwrap_or_else(|| target.id.clone());
            app.queue_catchup_resume(
                target.id.clone(),
                Some(source_session_id),
                Some((1, total)),
                true,
            );
            app.push_display_message(DisplayMessage::system(format!(
                "Queued Catch Up for **{}**.",
                target_name,
            )));
            app.set_status_notice(format!("Catch Up → {}", target_name));
            true
        }
        _ => {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/catchup [next|list]`".to_string(),
            ));
            true
        }
    }
}

fn handle_back_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed != "/back" {
        return false;
    }
    if !app.is_remote {
        app.push_display_message(DisplayMessage::error(
            "`/back` currently requires a connected shared server session.".to_string(),
        ));
        return true;
    }
    if app.is_processing {
        app.set_status_notice("Finish current work before going back");
        return true;
    }
    let Some(target) = app.pop_catchup_return_target() else {
        app.push_display_message(DisplayMessage::system(
            "No previous Catch Up session is available.".to_string(),
        ));
        app.set_status_notice("Back: empty");
        return true;
    };

    let target_name = crate::id::extract_session_name(&target)
        .map(|name| name.to_string())
        .unwrap_or_else(|| target.clone());
    app.queue_catchup_resume(target, None, None, false);
    app.push_display_message(DisplayMessage::system(format!(
        "Queued return to **{}**.",
        target_name,
    )));
    app.set_status_notice(format!("Back → {}", target_name));
    true
}

pub(super) fn handle_session_command(app: &mut App, trimmed: &str) -> bool {
    if handle_subagent_model_command(app, trimmed)
        || handle_subagent_command(app, trimmed)
        || handle_observe_command(app, trimmed)
        || handle_btw_command(app, trimmed)
        || handle_catchup_command(app, trimmed)
        || handle_back_command(app, trimmed)
        || handle_autoreview_command_local(app, trimmed)
        || handle_autojudge_command_local(app, trimmed)
        || handle_review_command_local(app, trimmed)
        || handle_judge_command_local(app, trimmed)
        || handle_selfdev_command(app, trimmed)
    {
        return true;
    }

    if let Some(command) = parse_improve_command(trimmed) {
        match command {
            Ok(command) => handle_improve_command_local(app, command),
            Err(error) => app.push_display_message(DisplayMessage::error(error)),
        }
        return true;
    }

    if let Some(command) = parse_refactor_command(trimmed) {
        match command {
            Ok(command) => handle_refactor_command_local(app, command),
            Err(error) => app.push_display_message(DisplayMessage::error(error)),
        }
        return true;
    }

    if trimmed == "/clear" {
        reset_current_session(app);
        return true;
    }

    if trimmed == "/save" || trimmed.starts_with("/save ") {
        let label = trimmed.strip_prefix("/save").unwrap_or_default().trim();
        let label = if label.is_empty() {
            None
        } else {
            Some(label.to_string())
        };
        app.session.mark_saved(label.clone());
        if let Err(e) = app.session.save() {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to save session: {}",
                e
            )));
            return true;
        }
        app.trigger_save_memory_extraction();
        let name = app.session.display_name().to_string();
        let msg = if let Some(ref lbl) = app.session.save_label {
            format!(
                "📌 Session **{}** saved as \"**{}**\". It will appear at the top of `/resume`.",
                name, lbl,
            )
        } else {
            format!(
                "📌 Session **{}** saved. It will appear at the top of `/resume`.",
                name,
            )
        };
        app.push_display_message(DisplayMessage::system(msg));
        app.set_status_notice("Session saved");
        return true;
    }

    if trimmed == "/unsave" {
        app.session.unmark_saved();
        if let Err(e) = app.session.save() {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to save session: {}",
                e
            )));
            return true;
        }
        let name = app.session.display_name().to_string();
        app.push_display_message(DisplayMessage::system(format!(
            "Removed bookmark from session **{}**.",
            name,
        )));
        app.set_status_notice("Bookmark removed");
        return true;
    }

    if trimmed == "/memory status" {
        let default_enabled = crate::config::config().features.memory;
        app.push_display_message(DisplayMessage::system(format!(
            "Memory feature: **{}** (config default: {})",
            if app.memory_enabled {
                "enabled"
            } else {
                "disabled"
            },
            if default_enabled {
                "enabled"
            } else {
                "disabled"
            }
        )));
        return true;
    }

    if trimmed == "/memory" {
        let new_state = !app.memory_enabled;
        app.set_memory_feature_enabled(new_state);
        let label = if new_state { "ON" } else { "OFF" };
        app.set_status_notice(&format!("Memory: {}", label));
        app.push_display_message(DisplayMessage::system(format!(
            "Memory feature {} for this session.",
            if new_state { "enabled" } else { "disabled" }
        )));
        return true;
    }

    if trimmed == "/memory on" {
        app.set_memory_feature_enabled(true);
        app.set_status_notice("Memory: ON");
        app.push_display_message(DisplayMessage::system(
            "Memory feature enabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed == "/memory off" {
        app.set_memory_feature_enabled(false);
        app.set_status_notice("Memory: OFF");
        app.push_display_message(DisplayMessage::system(
            "Memory feature disabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed.starts_with("/memory ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/memory [on|off|status]`".to_string(),
        ));
        return true;
    }

    if handle_goals_command(app, trimmed) {
        return true;
    }

    if trimmed == "/swarm" || trimmed == "/swarm status" {
        let default_enabled = crate::config::config().features.swarm;
        app.push_display_message(DisplayMessage::system(format!(
            "Swarm feature: **{}** (config default: {})",
            if app.swarm_enabled {
                "enabled"
            } else {
                "disabled"
            },
            if default_enabled {
                "enabled"
            } else {
                "disabled"
            }
        )));
        return true;
    }

    if trimmed == "/swarm on" {
        app.set_swarm_feature_enabled(true);
        app.set_status_notice("Swarm: ON");
        app.push_display_message(DisplayMessage::system(
            "Swarm feature enabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed == "/swarm off" {
        app.set_swarm_feature_enabled(false);
        app.set_status_notice("Swarm: OFF");
        app.push_display_message(DisplayMessage::system(
            "Swarm feature disabled for this session.".to_string(),
        ));
        return true;
    }

    if trimmed.starts_with("/swarm ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/swarm [on|off|status]`".to_string(),
        ));
        return true;
    }

    if trimmed == "/rewind" {
        if app.session.messages.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "No messages in conversation.".to_string(),
            ));
            return true;
        }

        let mut history = String::from("**Conversation history:**\n\n");
        for (i, msg) in app.session.messages.iter().enumerate() {
            let role_str = match msg.role {
                Role::User => "👤 User",
                Role::Assistant => "🤖 Assistant",
            };
            let content = msg.content_preview();
            let preview = crate::util::truncate_str(&content, 80);
            history.push_str(&format!("  `{}` {} - {}\n", i + 1, role_str, preview));
        }
        history.push_str("\nUse `/rewind N` to rewind to message N (removes all messages after).");

        app.push_display_message(DisplayMessage::system(history));
        return true;
    }

    if let Some(num_str) = trimmed.strip_prefix("/rewind ") {
        let num_str = num_str.trim();
        match num_str.parse::<usize>() {
            Ok(n) if n > 0 && n <= app.session.messages.len() => {
                let removed = app.session.messages.len() - n;
                app.session.truncate_messages(n);
                let provider_messages = app.session.messages_for_provider();
                app.replace_provider_messages(provider_messages);
                app.session.updated_at = chrono::Utc::now();

                app.clear_display_messages();
                for rendered in crate::session::render_messages(&app.session) {
                    app.push_display_message(DisplayMessage {
                        role: rendered.role,
                        content: rendered.content,
                        tool_calls: rendered.tool_calls,
                        duration_secs: None,
                        title: None,
                        tool_data: rendered.tool_data,
                    });
                }

                app.provider_session_id = None;
                app.session.provider_session_id = None;
                let _ = app.session.save();

                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Rewound to message {}. Removed {} message{}.",
                    n,
                    removed,
                    if removed == 1 { "" } else { "s" }
                )));
            }
            Ok(n) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Invalid message number: {}. Valid range: 1-{}",
                    n,
                    app.session.messages.len()
                )));
            }
            Err(_) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Usage: `/rewind N` where N is a message number (1-{})",
                    app.session.messages.len()
                )));
            }
        }
        return true;
    }

    if trimmed == "/poke" {
        let session_id = app.session.id.clone();
        let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
        let incomplete: Vec<_> = todos
            .iter()
            .filter(|t| t.status != "completed" && t.status != "cancelled")
            .collect();

        if incomplete.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "No incomplete todos found. Nothing to poke about.".to_string(),
            ));
            return true;
        }

        let mut todo_list = String::new();
        for t in &incomplete {
            let status_icon = match t.status.as_str() {
                "in_progress" => "🔄",
                _ => "⬜",
            };
            todo_list.push_str(&format!(
                "  {} [{}] {}\n",
                status_icon, t.priority, t.content
            ));
        }

        let poke_msg = format!(
            "Your todo list has {} incomplete item{}:\n\n{}\n\
            Please continue your work. Either:\n\
            1. Keep working and complete the remaining tasks\n\
            2. Update the todo list with `todo_write` if items are already done or no longer needed\n\
            3. If you genuinely need user input to proceed, say so clearly and specifically — \
            but only if truly blocked (this should be rare; prefer making reasonable assumptions)",
            incomplete.len(),
            if incomplete.len() == 1 { "" } else { "s" },
            todo_list,
        );

        if app.is_processing {
            app.cancel_requested = true;
            app.interleave_message = None;
            app.pending_soft_interrupts.clear();
            app.pending_soft_interrupt_requests.clear();
            app.set_status_notice("Interrupting for poke...");
            app.push_display_message(DisplayMessage::system(format!(
                "👉 Interrupting and poking with {} incomplete todo{}...",
                incomplete.len(),
                if incomplete.len() == 1 { "" } else { "s" },
            )));
            app.queued_messages.push(poke_msg);
        } else {
            app.push_display_message(DisplayMessage::system(format!(
                "👉 Poking model with {} incomplete todo{}...",
                incomplete.len(),
                if incomplete.len() == 1 { "" } else { "s" },
            )));

            app.add_provider_message(Message::user(&poke_msg));
            app.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: poke_msg,
                    cache_control: None,
                }],
            );
            let _ = app.session.save();

            app.is_processing = true;
            app.status = ProcessingStatus::Sending;
            app.clear_streaming_render_state();
            app.stream_buffer.clear();
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            app.streaming_tool_calls.clear();
            app.batch_progress = None;
            app.streaming_input_tokens = 0;
            app.streaming_output_tokens = 0;
            app.streaming_cache_read_tokens = None;
            app.streaming_cache_creation_tokens = None;
            app.upstream_provider = None;
            app.status_detail = None;
            app.streaming_tps_start = None;
            app.streaming_tps_elapsed = std::time::Duration::ZERO;
            app.streaming_tps_collect_output = false;
            app.streaming_total_output_tokens = 0;
            app.processing_started = Some(Instant::now());
            app.pending_turn = true;
        }

        return true;
    }

    false
}

fn handle_selfdev_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/selfdev") {
        return false;
    }

    let rest = trimmed.strip_prefix("/selfdev").unwrap_or_default().trim();
    if rest == "status" {
        match crate::tool::selfdev::selfdev_status_output() {
            Ok(output) => {
                app.push_display_message(DisplayMessage::system(output.output));
                app.set_status_notice("Self-dev status");
            }
            Err(e) => app.push_display_message(DisplayMessage::error(format!(
                "Failed to read self-dev status: {}",
                e
            ))),
        }
        return true;
    }

    if rest == "help" {
        app.push_display_message(DisplayMessage::system(
            "`/selfdev`\nSpawn a new self-dev jcode session in a separate terminal.\n\n`/selfdev <prompt>`\nSpawn a new self-dev session and auto-deliver the prompt to it.\n\n`/selfdev status`\nShow current self-dev/build status."
                .to_string(),
        ));
        return true;
    }

    let prompt = if rest.is_empty() || rest == "enter" {
        None
    } else if let Some(prompt) = rest.strip_prefix("enter ") {
        let prompt = prompt.trim();
        (!prompt.is_empty()).then(|| prompt.to_string())
    } else {
        Some(rest.to_string())
    };

    match crate::tool::selfdev::enter_selfdev_session(
        Some(&active_session_id(app)),
        active_working_dir(app).as_deref(),
    ) {
        Ok(launch) => {
            let mut message = if launch.test_mode {
                format!(
                    "Created self-dev session `{}` in `{}`.\n\nTest mode skipped launching a new terminal.",
                    launch.session_id,
                    launch.repo_dir.display()
                )
            } else if launch.launched {
                format!(
                    "Spawned self-dev session `{}` in a new terminal.\n\nRepo: `{}`",
                    launch.session_id,
                    launch.repo_dir.display()
                )
            } else {
                format!(
                    "Created self-dev session `{}` but could not auto-open a supported terminal.\n\nRun manually:\n`{}`",
                    launch.session_id,
                    launch.command_preview().unwrap_or_else(|| format!(
                        "jcode --resume {} self-dev",
                        launch.session_id
                    ))
                )
            };

            if launch.inherited_context {
                message.push_str("\n\nContext was cloned from the current session.");
            }

            if let Some(prompt_text) = prompt {
                if launch.launched && !launch.test_mode {
                    crate::tool::selfdev::schedule_selfdev_prompt_delivery(
                        launch.session_id.clone(),
                        prompt_text,
                    );
                    message.push_str("\n\nPrompt delivery queued to the spawned self-dev session.");
                } else if launch.test_mode {
                    message.push_str("\n\nPrompt captured but not delivered in test mode.");
                } else {
                    message.push_str("\n\nPrompt was not auto-delivered because the self-dev terminal did not launch.");
                }
            }

            app.push_display_message(DisplayMessage::system(message));
            app.set_status_notice("Self-dev");
        }
        Err(e) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to enter self-dev mode: {}",
            e
        ))),
    }

    true
}

pub(super) fn handle_goals_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/goals" {
        match crate::goal::open_goals_overview_for_session(
            active_session_id(app).as_str(),
            active_working_dir(app).as_deref(),
            true,
        ) {
            Ok(snapshot) => {
                app.set_side_panel_snapshot(snapshot);
                let count = crate::goal::list_relevant_goals(active_working_dir(app).as_deref())
                    .map(|goals| goals.len())
                    .unwrap_or(0);
                app.push_display_message(DisplayMessage::system(format!(
                    "Opened goals overview in the side panel ({} goal{}).",
                    count,
                    if count == 1 { "" } else { "s" }
                )));
                app.set_status_notice("Goals");
            }
            Err(e) => app.push_display_message(DisplayMessage::error(format!(
                "Failed to open goals overview: {}",
                e
            ))),
        }
        return true;
    }

    if trimmed == "/goals resume" {
        match crate::goal::resume_goal_for_session(
            active_session_id(app).as_str(),
            active_working_dir(app).as_deref(),
            true,
        ) {
            Ok(Some(result)) => {
                app.set_side_panel_snapshot(result.snapshot);
                let mut msg = format!("Resumed goal **{}**.", result.goal.title);
                if let Some(next_step) = result.goal.next_steps.first() {
                    msg.push_str(&format!(" Next step: {}", next_step));
                }
                app.push_display_message(DisplayMessage::system(msg));
                app.set_status_notice(format!("Goal: {}", result.goal.title));
            }
            Ok(None) => app.push_display_message(DisplayMessage::system(
                "No resumable goals found for this session.".to_string(),
            )),
            Err(e) => app.push_display_message(DisplayMessage::error(format!(
                "Failed to resume goal: {}",
                e
            ))),
        }
        return true;
    }

    if let Some(id) = trimmed.strip_prefix("/goals show ") {
        let id = id.trim();
        if id.is_empty() {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/goals show <id>`".to_string(),
            ));
            return true;
        }
        match crate::goal::open_goal_for_session(
            active_session_id(app).as_str(),
            active_working_dir(app).as_deref(),
            id,
            true,
        ) {
            Ok(Some(result)) => {
                app.set_side_panel_snapshot(result.snapshot);
                app.push_display_message(DisplayMessage::system(format!(
                    "Opened goal **{}** in the side panel.",
                    result.goal.title
                )));
                app.set_status_notice(format!("Goal: {}", result.goal.title));
            }
            Ok(None) => {
                app.push_display_message(DisplayMessage::error(format!("Goal not found: {}", id)))
            }
            Err(e) => app
                .push_display_message(DisplayMessage::error(format!("Failed to open goal: {}", e))),
        }
        return true;
    }

    if trimmed.starts_with("/goals ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/goals`, `/goals resume`, or `/goals show <id>`".to_string(),
        ));
        return true;
    }

    false
}

pub(super) fn active_session_id(app: &App) -> String {
    if app.is_remote {
        app.remote_session_id
            .clone()
            .unwrap_or_else(|| app.session.id.clone())
    } else {
        app.session.id.clone()
    }
}

fn active_working_dir(app: &App) -> Option<std::path::PathBuf> {
    app.session
        .working_dir
        .as_deref()
        .map(std::path::PathBuf::from)
}

pub(super) fn handle_dictation_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/dictate" || trimmed == "/dictation" {
        app.handle_dictation_trigger();
        return true;
    }

    if trimmed.starts_with("/dictate ") || trimmed.starts_with("/dictation ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/dictate`\nConfigure `[dictation]` in `~/.jcode/config.toml` to customize command, mode, hotkey, and timeout."
                .to_string(),
        ));
        return true;
    }

    false
}

fn alignment_label(centered: bool) -> &'static str {
    if centered { "centered" } else { "left-aligned" }
}

fn alignment_status_notice(centered: bool) -> &'static str {
    if centered {
        "Layout: Centered"
    } else {
        "Layout: Left-aligned"
    }
}

fn parse_alignment_value(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "centered" | "center" | "centre" | "on" => Some(true),
        "left" | "left-aligned" | "left_aligned" | "off" => Some(false),
        _ => None,
    }
}

fn parse_agents_target(raw: &str) -> Option<crate::tui::AgentModelTarget> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "swarm" | "agent" | "agents" | "subagent" | "subagents" => {
            Some(crate::tui::AgentModelTarget::Swarm)
        }
        "review" | "reviewer" | "code-review" | "codereview" => {
            Some(crate::tui::AgentModelTarget::Review)
        }
        "judge" | "judging" | "execution-judge" | "autojudge" => {
            Some(crate::tui::AgentModelTarget::Judge)
        }
        "memory" | "memories" | "sidecar" => Some(crate::tui::AgentModelTarget::Memory),
        "ambient" => Some(crate::tui::AgentModelTarget::Ambient),
        _ => None,
    }
}

pub(super) fn handle_agents_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/agents") {
        return false;
    }

    let rest = trimmed.strip_prefix("/agents").unwrap_or_default().trim();
    if rest.is_empty() {
        app.open_agents_picker();
        return true;
    }

    let Some(target) = parse_agents_target(rest) else {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/agents` or `/agents <swarm|review|judge|memory|ambient>`".to_string(),
        ));
        return true;
    };

    app.open_agent_model_picker(target);
    true
}

fn handle_alignment_command(app: &mut App, trimmed: &str) -> bool {
    if !trimmed.starts_with("/alignment") {
        return false;
    }

    let rest = trimmed
        .strip_prefix("/alignment")
        .unwrap_or_default()
        .trim();

    if rest.is_empty() || matches!(rest, "show" | "status") {
        let saved = crate::config::Config::load().display.centered;
        app.push_display_message(DisplayMessage::system(format!(
            "Alignment is currently **{}**.\nSaved default: **{}**.\n\nUse `/alignment centered` or `/alignment left` to change it permanently, or press `Alt+C` to toggle it for the current session.",
            alignment_label(app.centered),
            alignment_label(saved)
        )));
        return true;
    }

    let Some(centered) = parse_alignment_value(rest) else {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/alignment` (show), `/alignment centered`, or `/alignment left`".to_string(),
        ));
        return true;
    };

    app.set_centered(centered);
    app.set_status_notice(alignment_status_notice(centered));

    match crate::config::Config::set_display_centered(centered) {
        Ok(()) => app.push_display_message(DisplayMessage::system(format!(
            "Saved default alignment: **{}**. Applied to this session immediately.",
            alignment_label(centered)
        ))),
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Applied **{}** alignment for this session, but failed to save it as the default: {}",
            alignment_label(centered),
            error
        ))),
    }

    true
}

pub(super) fn handle_config_command(app: &mut App, trimmed: &str) -> bool {
    if handle_alignment_command(app, trimmed) {
        return true;
    }

    if handle_agents_command(app, trimmed) {
        return true;
    }

    if trimmed == "/compact mode" || trimmed == "/compact mode status" {
        let mode = app
            .registry
            .compaction()
            .try_read()
            .map(|manager| manager.mode())
            .unwrap_or_default();
        app.push_display_message(DisplayMessage::system(format!(
            "Compaction mode: **{}**\nAvailable: reactive · proactive · semantic\nUse `/compact mode <mode>` to change it for this session.",
            mode.as_str()
        )));
        return true;
    }

    if let Some(mode_str) = trimmed.strip_prefix("/compact mode ") {
        let mode_str = mode_str.trim();
        let Some(mode) = crate::config::CompactionMode::parse(mode_str) else {
            app.push_display_message(DisplayMessage::error(
                "Usage: `/compact mode <reactive|proactive|semantic>`".to_string(),
            ));
            return true;
        };

        match app.registry.compaction().try_write() {
            Ok(mut manager) => {
                manager.set_mode(mode.clone());
                let label = mode.as_str();
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Compaction mode → {}",
                    label
                )));
                app.set_status_notice(format!("Compaction: {}", label));
            }
            Err(_) => {
                app.push_display_message(DisplayMessage::error(
                    "Cannot access compaction manager (lock held)".to_string(),
                ));
            }
        }
        return true;
    }

    if trimmed == "/compact" {
        if !app.provider.supports_compaction() {
            app.push_display_message(DisplayMessage::system(
                "Manual compaction is not available for this provider.".to_string(),
            ));
            return true;
        }
        let compaction = app.registry.compaction();
        match compaction.try_write() {
            Ok(mut manager) => {
                let stats = manager.stats_with(&app.messages);
                let status_msg = format!(
                    "**Context Status:**\n\
                    • Messages: {} (active), {} (total history)\n\
                    • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                    • Has summary: {}\n\
                    • Compacting: {}",
                    stats.active_messages,
                    stats.total_turns,
                    stats.effective_tokens / 1000,
                    stats.token_estimate / 1000,
                    manager.token_budget() / 1000,
                    stats.context_usage * 100.0,
                    if stats.has_summary { "yes" } else { "no" },
                    if stats.is_compacting {
                        "in progress..."
                    } else {
                        "no"
                    }
                );

                match manager.force_compact_with(&app.messages, app.provider.clone()) {
                    Ok(()) => {
                        app.push_display_message(DisplayMessage {
                            role: "system".to_string(),
                            content: format!(
                                "{}\n\n{}\n\
                                The summary will be applied automatically when ready.\n\
                                Use `/help compact` for details.",
                                status_msg,
                                App::format_compaction_started_message("manual")
                            ),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                    Err(reason) => {
                        app.push_display_message(DisplayMessage {
                            role: "system".to_string(),
                            content: format!(
                                "{}\n\n⚠ **Cannot compact:** {}\n\
                                Try `/fix` for emergency recovery.",
                                status_msg, reason
                            ),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                }
            }
            Err(_) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: "⚠ Cannot access compaction manager (lock held)".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
        return true;
    }

    if trimmed == "/fix" {
        app.run_fix_command();
        return true;
    }

    if handle_usage_command(app, trimmed) {
        return true;
    }

    if trimmed == "/subscription" || trimmed == "/subscription status" {
        app.show_jcode_subscription_status();
        return true;
    }

    if trimmed == "/config" {
        use crate::config::config;
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: config().display_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/config init" || trimmed == "/config create" {
        use crate::config::Config;
        match Config::create_default_config_file() {
            Ok(path) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "Created default config file at:\n`{}`\n\nEdit this file to customize your keybindings and settings.",
                        path.display()
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            Err(e) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Failed to create config file: {}", e),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
        return true;
    }

    if trimmed == "/config edit" {
        use crate::config::Config;
        if let Some(path) = Config::path() {
            if !path.exists() {
                if let Err(e) = Config::create_default_config_file() {
                    app.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("Failed to create config file: {}", e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    return true;
                }
            }

            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            app.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "Opening config in editor...\n`{} {}`\n\n*Restart jcode after editing for changes to take effect.*",
                    editor,
                    path.display()
                ),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });

            let _ = std::process::Command::new(&editor).arg(&path).spawn();
        }
        return true;
    }

    if trimmed.starts_with("/config ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/config` (show), `/config init` (create), `/config edit` (open in editor)"
                .to_string(),
        ));
        return true;
    }

    false
}

pub(super) fn handle_debug_command(app: &mut App, trimmed: &str) -> bool {
    super::debug::handle_debug_command(app, trimmed)
}

pub(super) fn handle_model_command(app: &mut App, trimmed: &str) -> bool {
    super::model_context::handle_model_command(app, trimmed)
}

pub(super) fn handle_usage_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/usage") else {
        return false;
    };

    if !rest.is_empty()
        && !rest
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
    {
        return false;
    }

    app.open_usage_inline_loading();
    app.request_usage_report();
    true
}

pub(super) fn handle_feedback_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("/feedback") else {
        return false;
    };

    let rest = rest.trim();
    if rest.is_empty() {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/feedback <up|down> [wrong_answer|slow|bad_edit|auth_problem|tool_failure|crash|confusing_ux|other]`"
                .to_string(),
        ));
        return true;
    }

    let mut parts = rest.split_whitespace();
    let rating = match parts.next().unwrap_or_default() {
        "up" | "+" | "good" | "positive" => "up",
        "down" | "-" | "bad" | "negative" => "down",
        _ => {
            app.push_display_message(DisplayMessage::error(
                "Feedback rating must be `up` or `down`.".to_string(),
            ));
            return true;
        }
    };

    let reason = parts
        .next()
        .map(|value| value.trim().to_ascii_lowercase().replace('-', "_"));
    if let Some(reason) = reason.as_deref()
        && !matches!(
            reason,
            "wrong_answer"
                | "slow"
                | "bad_edit"
                | "auth_problem"
                | "tool_failure"
                | "crash"
                | "confusing_ux"
                | "other"
        )
    {
        app.push_display_message(DisplayMessage::error(
            "Feedback reason must be one of: wrong_answer, slow, bad_edit, auth_problem, tool_failure, crash, confusing_ux, other"
                .to_string(),
        ));
        return true;
    }

    crate::telemetry::record_feedback(rating, reason.as_deref());
    let detail = reason
        .as_deref()
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    app.push_display_message(DisplayMessage::system(format!(
        "Thanks, recorded feedback: **{}**{}.",
        rating, detail
    )));
    app.set_status_notice("Feedback recorded");
    true
}

pub(super) fn handle_info_command(app: &mut App, trimmed: &str) -> bool {
    super::state_ui::handle_info_command(app, trimmed)
}

pub(super) fn handle_auth_command(app: &mut App, trimmed: &str) -> bool {
    super::auth::handle_auth_command(app, trimmed)
}

pub(super) fn handle_dev_command(app: &mut App, trimmed: &str) -> bool {
    super::tui_lifecycle::handle_dev_command(app, trimmed)
}

#[cfg(test)]
mod tests {
    use super::parse_manual_subagent_spec;

    #[test]
    fn parse_manual_subagent_spec_accepts_flags_and_prompt() {
        let spec = parse_manual_subagent_spec(
            "--type research --model gpt-5.4 --continue session_123 investigate this bug",
        )
        .expect("parse manual subagent spec");

        assert_eq!(spec.subagent_type, "research");
        assert_eq!(spec.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(spec.session_id.as_deref(), Some("session_123"));
        assert_eq!(spec.prompt, "investigate this bug");
    }

    #[test]
    fn parse_manual_subagent_spec_rejects_missing_prompt() {
        let err = parse_manual_subagent_spec("--model gpt-5.4")
            .expect_err("missing prompt should be rejected");
        assert!(err.contains("Missing prompt"));
    }
}
