use super::{App, DisplayMessage, ProcessingStatus};
use crate::message::{ContentBlock, Message, Role};
use crate::session::Session;
use std::time::Instant;

pub(super) fn reset_current_session(app: &mut App) {
    app.clear_provider_messages();
    app.clear_display_messages();
    app.queued_messages.clear();
    app.pasted_contents.clear();
    app.pending_images.clear();
    app.active_skill = None;
    let mut session = Session::create(None, None);
    session.model = Some(app.provider.model());
    app.session = session;
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.provider_session_id = None;
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

pub(super) fn handle_session_command(app: &mut App, trimmed: &str) -> bool {
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
                app.session.messages.truncate(n);
                app.replace_provider_messages(app.session.messages_for_provider());
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
            app.streaming_tps_start = None;
            app.streaming_tps_elapsed = std::time::Duration::ZERO;
            app.streaming_total_output_tokens = 0;
            app.processing_started = Some(Instant::now());
            app.pending_turn = true;
        }

        return true;
    }

    false
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

pub(super) fn handle_config_command(app: &mut App, trimmed: &str) -> bool {
    use crate::bus::{Bus, BusEvent};

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

    if trimmed == "/usage" {
        app.push_display_message(DisplayMessage::system(
            "Fetching usage limits from all providers...".to_string(),
        ));
        tokio::spawn(async move {
            let results = crate::usage::fetch_all_provider_usage().await;
            Bus::global().publish(BusEvent::UsageReport(results));
        });
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

pub(super) fn handle_info_command(app: &mut App, trimmed: &str) -> bool {
    super::state_ui::handle_info_command(app, trimmed)
}

pub(super) fn handle_auth_command(app: &mut App, trimmed: &str) -> bool {
    super::auth::handle_auth_command(app, trimmed)
}

pub(super) fn handle_dev_command(app: &mut App, trimmed: &str) -> bool {
    super::tui_lifecycle::handle_dev_command(app, trimmed)
}
