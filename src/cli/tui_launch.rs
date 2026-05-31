#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::{Context, Result};

const MAX_INTERACTIVE_SWARM_REPLAY_PANES: usize = 16;
use std::io::{self, Write};
use std::process::Command as ProcessCommand;

use crate::{
    id, logging, replay, server, session, setup_hints, startup_profile, tui, video_export,
};

use super::hot_exec::{execute_requested_action, has_requested_action};

use super::terminal::{
    cleanup_tui_runtime, cleanup_tui_runtime_for_run_result, init_tui_runtime,
    print_session_resume_hint, set_current_session, spawn_session_signal_watchers,
};

pub(crate) use crate::session_launch::resumed_window_title;

pub async fn run_client() -> Result<()> {
    let mut client = server::Client::connect().await?;

    if !client.ping().await? {
        anyhow::bail!("Failed to ping server");
    }

    println!("Connected to J-Code server");
    println!("Type your message, or 'quit' to exit.\n");

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if input == "quit" || input == "exit" {
            break;
        }

        match client.send_message(input).await {
            Ok(msg_id) => loop {
                match client.read_event().await {
                    Ok(event) => {
                        use crate::protocol::ServerEvent;
                        match event {
                            ServerEvent::TextDelta { text } => {
                                print!("{}", text);
                                std::io::stdout().flush()?;
                            }
                            ServerEvent::Done { id } if id == msg_id => {
                                break;
                            }
                            ServerEvent::Error { message, .. } => {
                                eprintln!("Error: {}", message);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        eprintln!("Event error: {}", e);
                        break;
                    }
                }
            },
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }

        println!();
    }

    Ok(())
}

pub async fn run_tui_client(
    resume_session: Option<String>,
    startup_hints: Option<setup_hints::StartupHints>,
    server_spawning: bool,
    fresh_spawn: bool,
) -> Result<()> {
    startup_profile::mark("tui_client_enter");
    let (terminal, tui_runtime) = init_tui_runtime()?;
    startup_profile::mark("tui_terminal_init");
    startup_profile::mark("mermaid_picker");
    startup_profile::mark("config_load");
    startup_profile::mark("keyboard_enhancement");
    startup_profile::mark("terminal_modes");

    if let Some(ref session_id) = resume_session {
        set_current_session(session_id);
    }
    spawn_session_signal_watchers();

    if let Some(ref session_id) = resume_session {
        let session_name = id::extract_session_name(session_id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| session_id.clone());
        let is_selfdev = super::selfdev::client_selfdev_requested();
        if let Some(server_info) =
            crate::registry::find_server_by_socket_sync(&server::socket_path())
        {
            crate::process_title::set_client_remote_display_title(
                &server_info.name,
                &session_name,
                is_selfdev,
            );
        } else {
            crate::process_title::set_client_display_title(&session_name, is_selfdev);
        }
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(resumed_window_title(session_id))
        );
    } else {
        crate::process_title::set_client_generic_title(super::selfdev::client_selfdev_requested());
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("jcode"));
    }
    startup_profile::mark("terminal_title");

    let mut app = tui::App::new_for_remote_with_options(resume_session.clone(), fresh_spawn);
    if should_show_server_spawning(server_spawning).await {
        app.set_server_spawning();
    }
    startup_profile::mark("app_new_for_remote");
    if resume_session.is_none()
        && let Some(hints) = startup_hints
    {
        apply_startup_hints(&mut app, hints);
    }

    startup_profile::mark("pre_run_remote");
    startup_profile::report_to_log();

    let result = app.run_remote(terminal).await;

    let run_result = result?;

    cleanup_tui_runtime_for_run_result(&tui_runtime, &run_result, false);

    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    execute_requested_action(&run_result)?;

    if !has_requested_action(&run_result)
        && let Some(ref session_id) = run_result.session_id
    {
        print_session_resume_hint(session_id);
    }

    Ok(())
}

async fn should_show_server_spawning(server_spawning: bool) -> bool {
    if !server_spawning {
        return false;
    }

    let socket_path = server::socket_path();
    if server::has_live_listener(&socket_path).await {
        logging::info(&format!(
            "Skipping stale startup phase: server already listening at {}",
            socket_path.display()
        ));
        return false;
    }

    true
}

fn apply_startup_hints(app: &mut tui::App, hints: setup_hints::StartupHints) {
    if let Some(status_notice) = hints.status_notice {
        app.set_status_notice(status_notice);
    }
    if let Some((title, message)) = hints.display_message {
        app.push_display_message(tui::DisplayMessage::system(message).with_title(title));
    }
    if let Some(message) = hints.auto_send_message {
        app.queue_startup_message(message);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Replay command maps directly from CLI flags and transport options"
)]
pub async fn run_replay_command(
    session_id_or_path: &str,
    swarm: bool,
    export: bool,
    auto_edit: bool,
    speed: f64,
    timeline_path: Option<&str>,
    video_output: Option<&str>,
    cols: u16,
    rows: u16,
    fps: u32,
    centered_override: Option<bool>,
) -> Result<()> {
    if swarm {
        let swarm_sessions = replay::load_swarm_sessions(session_id_or_path, auto_edit)?;
        if export {
            let timelines: Vec<_> = swarm_sessions
                .iter()
                .map(|pane| {
                    serde_json::json!({
                        "session_id": pane.session.id,
                        "session_name": pane.session.short_name,
                        "timeline": pane.timeline,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&timelines)?);
            return Ok(());
        }

        if let Some(output) = video_output {
            let output_path = if output == "auto" {
                let date = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let safe_name = session_id_or_path
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                std::path::PathBuf::from(format!("jcode_swarm_replay_{}_{}.mp4", safe_name, date))
            } else {
                std::path::PathBuf::from(output)
            };
            let panes: Vec<_> = swarm_sessions
                .into_iter()
                .map(|pane| replay::PaneReplayInput {
                    session: pane.session,
                    timeline: pane.timeline,
                })
                .collect();
            eprintln!(
                "🐝 Exporting swarm replay from seed {} ({} panes)",
                session_id_or_path,
                panes.len()
            );
            video_export::export_swarm_video(
                &panes,
                speed,
                &output_path,
                cols,
                rows,
                fps,
                centered_override,
            )
            .await?;
            return Ok(());
        }

        let mut replayable_panes: Vec<_> = swarm_sessions
            .into_iter()
            .filter(|pane| !pane.timeline.is_empty())
            .map(|pane| replay::PaneReplayInput {
                session: pane.session,
                timeline: pane.timeline,
            })
            .collect();

        if replayable_panes.is_empty() {
            eprintln!("Swarm has no messages to replay.");
            return Ok(());
        }

        let total_panes = replayable_panes.len();
        if replayable_panes.len() > MAX_INTERACTIVE_SWARM_REPLAY_PANES {
            replayable_panes.truncate(MAX_INTERACTIVE_SWARM_REPLAY_PANES);
            eprintln!(
                "  Limiting interactive swarm replay to {} panes ({} discovered). Use --export/--video for the full set.",
                replayable_panes.len(),
                total_panes,
            );
        }

        let pane_count = replayable_panes.len();
        eprintln!(
            "🐝 Replaying swarm: {} ({} panes, {:.1}x speed)",
            session_id_or_path, pane_count, speed
        );
        eprintln!("  Controls: Space=pause  +/-=speed  q=quit\n");

        let (terminal, tui_runtime) = init_tui_runtime()?;
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!("🐝 swarm replay: {}", session_id_or_path))
        );

        let result =
            tui::App::run_swarm_replay(terminal, replayable_panes, speed, centered_override).await;

        cleanup_tui_runtime(&tui_runtime, true);
        result?;
        return Ok(());
    }

    let session = replay::load_session(session_id_or_path)?;

    let mut timeline = if let Some(path) = timeline_path {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read timeline file: {}", path))?;
        serde_json::from_str::<Vec<replay::TimelineEvent>>(&data)
            .with_context(|| format!("Failed to parse timeline JSON: {}", path))?
    } else {
        replay::export_timeline(&session)
    };

    if auto_edit {
        timeline = replay::auto_edit_timeline(&timeline, &replay::AutoEditOpts::default());
    }

    if export {
        let json = serde_json::to_string_pretty(&timeline)?;
        println!("{}", json);
        return Ok(());
    }

    if timeline.is_empty() {
        eprintln!("Session has no messages to replay.");
        return Ok(());
    }

    let session_name = session.short_name.as_deref().unwrap_or(&session.id);
    let icon = id::session_icon(session_name);

    if let Some(output) = video_output {
        let output_path = if output == "auto" {
            let date = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let safe_name = session_name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            std::path::PathBuf::from(format!("jcode_replay_{}_{}.mp4", safe_name, date))
        } else {
            std::path::PathBuf::from(output)
        };
        eprintln!(
            "{} Exporting session: {} ({} events)",
            icon,
            session_name,
            timeline.len()
        );
        video_export::export_video(
            &session,
            &timeline,
            speed,
            &output_path,
            cols,
            rows,
            fps,
            centered_override,
        )
        .await?;
        return Ok(());
    }

    eprintln!(
        "{} Replaying session: {} ({} events, {:.1}x speed)",
        icon,
        session_name,
        timeline.len(),
        speed
    );
    eprintln!("  Controls: Space=pause  +/-=speed  q=quit\n");

    let (terminal, tui_runtime) = init_tui_runtime()?;

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(format!("{} replay: {}", icon, session_name))
    );

    let mut app = tui::App::new_for_replay(session);
    if let Some(centered) = centered_override {
        app.set_centered(centered);
    }
    let result = app.run_replay(terminal, timeline, speed).await;

    cleanup_tui_runtime(&tui_runtime, true);

    result?;
    Ok(())
}

// Session-launching helpers live in the core `session_launch` module so that
// lower layers (server, restart_snapshot, tool) can relaunch sessions without
// depending on `cli`. Re-exported here for the CLI's own callers.
pub use crate::session_launch::{
    spawn_resume_in_new_terminal, spawn_resume_in_new_terminal_with_provider,
    spawn_selfdev_in_new_terminal, spawn_selfdev_in_new_terminal_with_provider,
};

pub fn list_sessions() -> Result<()> {
    fn build_resume_target_command(
        exe: &std::path::Path,
        target: &jcode_tui_session_picker::ResumeTarget,
    ) -> (std::path::PathBuf, Vec<String>) {
        match target {
            jcode_tui_session_picker::ResumeTarget::JcodeSession { session_id } => (
                exe.to_path_buf(),
                vec!["--resume".to_string(), session_id.clone()],
            ),
            jcode_tui_session_picker::ResumeTarget::ClaudeCodeSession { session_id, .. } => (
                exe.to_path_buf(),
                vec![
                    "--resume".to_string(),
                    crate::import::imported_claude_code_session_id(session_id),
                ],
            ),
            jcode_tui_session_picker::ResumeTarget::CodexSession { session_id, .. } => (
                exe.to_path_buf(),
                vec![
                    "--resume".to_string(),
                    crate::import::imported_codex_session_id(session_id),
                ],
            ),
            jcode_tui_session_picker::ResumeTarget::PiSession { session_path } => (
                exe.to_path_buf(),
                vec![
                    "--resume".to_string(),
                    crate::import::imported_pi_session_id(session_path),
                ],
            ),
            jcode_tui_session_picker::ResumeTarget::OpenCodeSession { session_id, .. } => (
                exe.to_path_buf(),
                vec![
                    "--resume".to_string(),
                    crate::import::imported_opencode_session_id(session_id),
                ],
            ),
        }
    }

    fn command_display(program: &std::path::Path, args: &[String]) -> String {
        std::iter::once(program.to_string_lossy().to_string())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn spawn_target_in_new_terminal(
        target: &jcode_tui_session_picker::ResumeTarget,
        exe: &std::path::Path,
        cwd: &std::path::Path,
    ) -> Result<bool> {
        let (program, args) = build_resume_target_command(exe, target);
        let title = match target {
            jcode_tui_session_picker::ResumeTarget::JcodeSession { session_id } => {
                resumed_window_title(session_id)
            }
            jcode_tui_session_picker::ResumeTarget::ClaudeCodeSession { session_id, .. } => {
                format!("🧵 Claude Code {}", &session_id[..session_id.len().min(8)])
            }
            jcode_tui_session_picker::ResumeTarget::CodexSession { session_id, .. } => {
                format!("🧠 Codex {}", &session_id[..session_id.len().min(8)])
            }
            jcode_tui_session_picker::ResumeTarget::PiSession { session_path } => {
                format!(
                    "π Pi {}",
                    std::path::Path::new(session_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("session")
                )
            }
            jcode_tui_session_picker::ResumeTarget::OpenCodeSession { session_id, .. } => {
                format!("◌ OpenCode {}", &session_id[..session_id.len().min(8)])
            }
        };
        let command = crate::terminal_launch::TerminalCommand::new(program, args).title(title);
        crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)
    }

    match tui::session_picker::pick_session()? {
        Some(
            tui::session_picker::PickerResult::Selected(targets)
            | tui::session_picker::PickerResult::SelectedInCurrentTerminal(targets),
        ) => {
            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;

            if targets.len() == 1 {
                let target = &targets[0];
                let resolved_target = crate::import::resolve_resume_target_to_jcode(target)?;
                let mut session_cwd = cwd.clone();
                if let jcode_tui_session_picker::ResumeTarget::JcodeSession { session_id } =
                    &resolved_target
                    && let Ok(sess) = session::Session::load(session_id)
                    && let Some(dir) = sess.working_dir.as_deref()
                    && std::path::Path::new(dir).is_dir()
                {
                    session_cwd = std::path::PathBuf::from(dir);
                }
                let (program, args) = build_resume_target_command(&exe, &resolved_target);
                let err = crate::platform::replace_process(
                    ProcessCommand::new(&program)
                        .args(&args)
                        .current_dir(session_cwd),
                );

                Err(anyhow::anyhow!("Failed to exec {:?}: {}", program, err))
            } else {
                let mut spawned = 0usize;
                let mut warned_no_terminal = false;

                for target in targets {
                    let resolved_target =
                        match crate::import::resolve_resume_target_to_jcode(&target) {
                            Ok(target) => target,
                            Err(e) => {
                                eprintln!("Failed to import selected session: {}", e);
                                continue;
                            }
                        };
                    let mut session_cwd = cwd.clone();
                    if let jcode_tui_session_picker::ResumeTarget::JcodeSession { session_id } =
                        &resolved_target
                        && let Ok(sess) = session::Session::load(session_id)
                        && let Some(dir) = sess.working_dir.as_deref()
                        && std::path::Path::new(dir).is_dir()
                    {
                        session_cwd = std::path::PathBuf::from(dir);
                    }

                    match spawn_target_in_new_terminal(&resolved_target, &exe, &session_cwd) {
                        Ok(true) => spawned += 1,
                        Ok(false) => {
                            if !warned_no_terminal {
                                eprintln!(
                                    "No supported terminal emulator found. Run these commands manually:"
                                );
                                warned_no_terminal = true;
                            }
                            let (program, args) =
                                build_resume_target_command(&exe, &resolved_target);
                            eprintln!("  {}", command_display(&program, &args));
                        }
                        Err(e) => {
                            eprintln!("Failed to spawn selected session: {}", e);
                        }
                    }
                }

                if spawned == 0 && warned_no_terminal {
                    return Ok(());
                }

                if spawned == 0 {
                    anyhow::bail!("Failed to spawn any selected sessions");
                }

                Ok(())
            }
        }
        Some(tui::session_picker::PickerResult::SelectedInNewTerminal(targets)) => {
            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;
            let mut spawned = 0usize;
            let mut warned_no_terminal = false;

            for target in targets {
                let resolved_target = match crate::import::resolve_resume_target_to_jcode(&target) {
                    Ok(target) => target,
                    Err(e) => {
                        eprintln!("Failed to import selected session: {}", e);
                        continue;
                    }
                };
                let mut session_cwd = cwd.clone();
                if let jcode_tui_session_picker::ResumeTarget::JcodeSession { session_id } =
                    &resolved_target
                    && let Ok(sess) = session::Session::load(session_id)
                    && let Some(dir) = sess.working_dir.as_deref()
                    && std::path::Path::new(dir).is_dir()
                {
                    session_cwd = std::path::PathBuf::from(dir);
                }

                match spawn_target_in_new_terminal(&resolved_target, &exe, &session_cwd) {
                    Ok(true) => spawned += 1,
                    Ok(false) => {
                        if !warned_no_terminal {
                            eprintln!(
                                "No supported terminal emulator found. Run these commands manually:"
                            );
                            warned_no_terminal = true;
                        }
                        let (program, args) = build_resume_target_command(&exe, &resolved_target);
                        eprintln!("  {}", command_display(&program, &args));
                    }
                    Err(e) => {
                        eprintln!("Failed to spawn selected session: {}", e);
                    }
                }
            }

            if spawned == 0 && warned_no_terminal {
                return Ok(());
            }

            if spawned == 0 {
                anyhow::bail!("Failed to spawn any selected sessions");
            }

            Ok(())
        }
        Some(tui::session_picker::PickerResult::RestoreCrashedGroup(session_ids)) => {
            let recovered = session::recover_crashed_sessions_by_ids(&session_ids)?;
            if recovered.is_empty() {
                eprintln!("No crashed sessions found in the selected restore group.");
                return Ok(());
            }

            eprintln!(
                "Recovered {} crashed session(s) from the selected restore group.",
                recovered.len()
            );

            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;
            let mut spawned = 0usize;
            let mut warned_no_terminal = false;

            for session_id in recovered {
                let mut session_cwd = cwd.clone();
                if let Ok(sess) = session::Session::load(&session_id)
                    && let Some(dir) = sess.working_dir.as_deref()
                    && std::path::Path::new(dir).is_dir()
                {
                    session_cwd = std::path::PathBuf::from(dir);
                }

                match spawn_resume_in_new_terminal(&exe, &session_id, &session_cwd) {
                    Ok(true) => {
                        spawned += 1;
                    }
                    Ok(false) => {
                        if !warned_no_terminal {
                            eprintln!(
                                "No supported terminal emulator found. Run these commands manually:"
                            );
                            warned_no_terminal = true;
                        }
                        eprintln!("  jcode --resume {}", session_id);
                    }
                    Err(e) => {
                        eprintln!("Failed to spawn session {}: {}", session_id, e);
                    }
                }
            }

            if spawned == 0 && warned_no_terminal {
                return Ok(());
            }

            if spawned == 0 {
                anyhow::bail!("Failed to spawn any recovered sessions");
            }

            Ok(())
        }
        None => {
            eprintln!("No session selected.");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
