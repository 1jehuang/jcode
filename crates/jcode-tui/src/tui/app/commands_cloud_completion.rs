//! Cloud command completion and handoff.

use super::*;

pub(super) fn finish_cloud_command(app: &mut App, job: CloudJob, output: std::process::Output) {
    if matches!(job.command, CloudCommand::Prepare { .. }) {
        // Canceled or replaced requests must never inherit stale host preparation.
        if job.preparation_for.as_ref() != app.pending_cloud_handoff.as_ref() {
            return;
        }
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if output.status.success()
            && let Some(host) = report["host"].as_str().filter(|host| !host.is_empty())
        {
            let mut steps = job.steps.clone();
            push_cloud_step(
                &mut steps,
                "cloud host ready, starting transfer".to_string(),
            );
            app.replace_display_message_content(
                job.progress_message,
                render_cloud_steps(&steps, 0, Some(true)),
            );
            if let Some(pending) = app.pending_cloud_handoff.as_mut() {
                match &mut pending.command {
                    CloudCommand::Move { host: target }
                    | CloudCommand::MoveSelected { host: target, .. } => {
                        *target = Some(host.to_string());
                    }
                    _ => unreachable!("only move commands prepare a host"),
                }
                pending.prepared = true;
                pending.progress = Some((job.progress_message, steps));
            }
        } else {
            app.pending_cloud_handoff = None;
            app.replace_display_message_content(
                job.progress_message,
                render_cloud_steps(&job.steps, 0, Some(false)),
            );
            let details = if output.status.success() {
                "Cloud host preparation returned an invalid response: missing non-empty host."
                    .to_string()
            } else {
                cloud_output_failure(&job, &output)
            };
            show_cloud_failure(
                app,
                &job.trimmed,
                &job.session_id,
                output.status.code(),
                &details,
            );
        }
        return;
    }
    if !output.status.success() {
        let details = cloud_output_failure(&job, &output);
        show_cloud_failure(
            app,
            &job.trimmed,
            &job.session_id,
            output.status.code(),
            &details,
        );
        return;
    }
    if let CloudCommand::Replace { dry_run, .. } = job.command {
        let report = serde_json::from_slice::<serde_json::Value>(&output.stdout);
        if let Ok(report) = report
            && let Some(root) = report["repo_root"].as_str().filter(|s| !s.is_empty())
            && report["dry_run"].as_bool() == Some(dry_run)
        {
            let text = if dry_run {
                format!(
                    "Cloud replacement preview passed for {root}. Nothing replaced. Apply with /cloud replace --yes{}.",
                    match &job.command {
                        CloudCommand::Replace {
                            host: Some(host), ..
                        } => format!(" --host {host}"),
                        _ => String::new(),
                    }
                )
            } else {
                let backup = report["backup_path"]
                    .as_str()
                    .unwrap_or("none (checkout was absent)");
                format!(
                    "Cloud repository replaced from local working files: {root}\nBackup: {backup}\nThis session stays here. No conversation or provider login was moved."
                )
            };
            app.push_display_message(DisplayMessage::system(text));
            if !dry_run {
                let host = report["host"].as_str().or(match &job.command {
                    CloudCommand::Replace { host, .. } => host.as_deref(),
                    _ => None,
                });
                app.cloud_recovery_context = Some((
                    job.session_id.clone(),
                    crate::cloud_environment::replacement_guidance(host),
                ));
            }
        } else {
            show_cloud_failure(
                app,
                &job.trimmed,
                &job.session_id,
                output.status.code(),
                "Cloud replacement returned an invalid response. Inspect the cloud checkout and backup directories before retrying.",
            );
        }
        return;
    }
    if matches!(
        job.command,
        CloudCommand::Move { .. } | CloudCommand::MoveSelected { .. } | CloudCommand::Return
    ) {
        let report = serde_json::from_slice::<serde_json::Value>(&output.stdout);
        let valid = report.as_ref().is_ok_and(|report| match job.command {
            CloudCommand::Move { .. } | CloudCommand::MoveSelected { .. } => {
                report["host"].as_str().is_some_and(|host| !host.is_empty())
            }
            CloudCommand::Return => report.is_object(),
            _ => unreachable!(),
        });
        if !valid {
            app.replace_display_message_content(
                job.progress_message,
                render_cloud_steps(&job.steps, 0, Some(false)),
            );
            show_cloud_failure(
                app,
                &job.trimmed,
                &job.session_id,
                output.status.code(),
                "Cloud command returned an invalid response. Could not confirm the handoff result. Run /cloud status to check the session location before retrying.",
            );
            return;
        }
    }
    let CloudJob {
        command,
        session_id,
        ..
    } = job;
    let session_id = session_id.as_str();
    if matches!(
        command,
        CloudCommand::Move { .. } | CloudCommand::MoveSelected { .. } | CloudCommand::Return
    ) && app
        .cloud_recovery_context
        .as_ref()
        .is_some_and(|(id, _)| id == session_id)
    {
        app.cloud_recovery_context = None;
        app.cloud_recovery_auto_pending = false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    match command {
        CloudCommand::Cancel
        | CloudCommand::Prepare { .. }
        | CloudCommand::Replace { .. }
        | CloudCommand::ReplaceUsage
        | CloudCommand::MoveUsage => {
            unreachable!("handled before transfer completion")
        }
        CloudCommand::Reload { .. } => {
            app.push_display_message(DisplayMessage::system(
                "Cloud daemon reloaded onto its selected runtime. Cloud sessions resume when their clients reconnect.",
            ));
        }
        CloudCommand::Where => {
            let text = if stdout.trim().is_empty() {
                "This session has never moved.".to_string()
            } else {
                stdout.trim().to_string()
            };
            app.push_display_message(DisplayMessage::system(text));
        }
        CloudCommand::Move { .. } | CloudCommand::MoveSelected { .. } => {
            let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
            let host = report["host"].as_str().unwrap_or_default().to_string();
            let repo_root = report["repo_root"].as_str().map(str::to_string);
            let selected = report_session_ids(&report, "members");
            let excluded = report_session_ids(&report, "excluded_session_ids");
            let group = selected.len() > 1;
            let selection_note = if selected.is_empty() {
                String::new()
            } else {
                format!("\nSelected sessions: {}", selected.join(", "))
            };
            let excluded_note = if excluded.is_empty() {
                String::new()
            } else {
                format!(
                    "\nStaying local: {}. Include explicitly with --include-session <id>.",
                    excluded.join(", ")
                )
            };
            if matches!(command, CloudCommand::MoveSelected { dry_run: true, .. }) {
                if report["dry_run"].as_bool() != Some(true) {
                    app.push_display_message(DisplayMessage::error(
                        "Cloud preview returned an invalid response. Check /cloud status before retrying.".to_string(),
                    ));
                    return;
                }
                app.push_display_message(DisplayMessage::system(format!(
                    "Cloud migration preview passed for `{host}`. No session ownership moved. Run the command without --dry-run to migrate.{selection_note}{excluded_note}"
                )));
                return;
            }
            let continuation = if group {
                "The coordinator handles shared environment setup before resuming workers. Group return is not yet supported; do not return individual members."
            } else {
                "Use /local to bring it back. Your local files stay editable, and git merges both sides on return."
            };
            app.push_display_message(DisplayMessage::system(format!(
                "☁️ {} moved to `{host}`. Reattaching there and continuing. Its disk persists while it sleeps. {continuation}{selection_note}{excluded_note}",
                if group { "Session group" } else { "Session" }
            )));
            app.cloud_handoff_requested = Some(CloudHandoff::Remote {
                session_id: report["session_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .unwrap_or(session_id)
                    .to_string(),
                host,
                working_dir: repo_root,
            });
            app.should_quit = true;
        }
        CloudCommand::Return => {
            let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
            let merge = &report["merge"];
            let note = match merge["result"].as_str() {
                Some("applied") => "Cloud work was merged into your checkout.".to_string(),
                Some("unchanged") => "No code changed on the cloud side.".to_string(),
                Some("conflicts") => format!(
                    "Code conflicts, working tree untouched: {}. The agent can resolve them.",
                    merge["files"]
                        .as_array()
                        .map(|f| f
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                ),
                _ => String::new(),
            };
            app.push_display_message(DisplayMessage::system(format!(
                "🏠 Session is back on this machine. {note}"
            )));
            app.cloud_handoff_requested = Some(CloudHandoff::Local {
                session_id: session_id.to_string(),
            });
            app.should_quit = true;
        }
    }
}
