use anyhow::{Context, Result};

const MAX_INTERACTIVE_SWARM_REPLAY_PANES: usize = 16;
use std::io::{self, Write};
use std::process::Command as ProcessCommand;
use std::sync::Arc;

use crate::{
    id, logging, provider, replay, server, session, setup_hints, startup_profile, tool, tui,
    video_export,
};

use super::hot_exec::{execute_requested_action, has_requested_action};

use super::terminal::{
    cleanup_tui_runtime, cleanup_tui_runtime_for_run_result, init_tui_runtime,
    print_session_resume_hint, set_current_session, spawn_session_signal_watchers,
};

pub async fn run_tui(
    provider: Arc<dyn provider::Provider>,
    registry: tool::Registry,
    resume_session: Option<String>,
    debug_socket: bool,
    startup_hints: Option<setup_hints::StartupHints>,
) -> Result<()> {
    let (terminal, tui_runtime) = init_tui_runtime()?;
    let mut app = tui::App::new(provider, registry);

    let _debug_handle = if debug_socket {
        let rx = app.enable_debug_socket();
        let handle = app.start_debug_socket_listener(rx);
        logging::info(&format!(
            "Debug socket enabled at: {:?}",
            tui::App::debug_socket_path()
        ));
        Some(handle)
    } else {
        None
    };

    if let Some(ref session_id) = resume_session {
        app.restore_session(session_id);
    } else if let Some(hints) = startup_hints {
        apply_startup_hints(&mut app, hints);
    }

    set_current_session(app.session_id());
    spawn_session_signal_watchers();

    let session_id = app.session_id().to_string();
    let session_name = id::extract_session_name(&session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.clone());

    let icon = id::session_icon(&session_name);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(format!("{} jcode {}", icon, session_name))
    );

    app.init_mcp().await;
    let result = app.run(terminal).await;

    let run_result = result?;

    cleanup_tui_runtime_for_run_result(&tui_runtime, &run_result, false);

    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    execute_requested_action(&run_result)?;

    if !has_requested_action(&run_result) {
        print_session_resume_hint(&session_id);
    }

    Ok(())
}

fn resumed_window_title(session_id: &str) -> String {
    let session_name = id::extract_session_name(session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id.to_string());
    let icon = id::session_icon(&session_name);
    format!("{} jcode {}", icon, session_name)
}

fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(unix)]
fn sh_escape(text: &str) -> String {
    format!("'{}'", text.replace('"', "\"").replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn shell_command(args: &[String]) -> String {
    args.iter()
        .map(|arg| sh_escape(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn focus_title_best_effort(title: &str) {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(
            "sleep 0.4; \
             if command -v wmctrl >/dev/null 2>&1; then wmctrl -a \"$JCODE_WINDOW_TITLE\" >/dev/null 2>&1 && exit 0; fi; \
             if command -v xdotool >/dev/null 2>&1; then xdotool search --name \"$JCODE_WINDOW_TITLE\" windowactivate >/dev/null 2>&1 && exit 0; fi; \
             exit 0",
        )
        .env("JCODE_WINDOW_TITLE", title)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let _ = crate::platform::spawn_detached(&mut cmd);
}

#[cfg(any(not(unix), target_os = "macos"))]
fn focus_title_best_effort(_title: &str) {}

fn push_unique_terminal(candidates: &mut Vec<String>, term: impl Into<String>) {
    let term = term.into();
    if term.trim().is_empty() {
        return;
    }
    if !candidates.iter().any(|candidate| candidate == &term) {
        candidates.push(term);
    }
}

#[cfg(unix)]
fn detected_resume_terminal() -> Option<&'static str> {
    if std::env::var("HANDTERM_SESSION").is_ok() || std::env::var("HANDTERM_PID").is_ok() {
        return Some("handterm");
    }
    if std::env::var("TERM_PROGRAM")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("handterm"))
        .unwrap_or(false)
    {
        return Some("handterm");
    }
    if std::env::var("KITTY_PID").is_ok() {
        return Some("kitty");
    }
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        return Some("wezterm");
    }
    if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        return Some("alacritty");
    }

    #[cfg(target_os = "macos")]
    {
        let term_program = std::env::var("TERM_PROGRAM")
            .ok()
            .map(|value| value.to_ascii_lowercase());
        return match term_program.as_deref() {
            Some("kitty") => Some("kitty"),
            Some("wezterm") => Some("wezterm"),
            Some("alacritty") => Some("alacritty"),
            Some("iterm.app") | Some("iterm2") => Some("iterm2"),
            Some("apple_terminal") | Some("terminal") => Some("terminal"),
            _ => None,
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(unix)]
fn resume_terminal_candidates_unix() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        push_unique_terminal(&mut candidates, term);
    }
    if let Some(term) = detected_resume_terminal() {
        push_unique_terminal(&mut candidates, term);
    }

    #[cfg(target_os = "macos")]
    {
        for term in ["kitty", "wezterm", "alacritty", "iterm2", "terminal"] {
            push_unique_terminal(&mut candidates, term);
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        for term in [
            "handterm",
            "kitty",
            "wezterm",
            "alacritty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
        ] {
            push_unique_terminal(&mut candidates, term);
        }
    }

    candidates
}

#[cfg(not(unix))]
fn detected_resume_terminal() -> Option<&'static str> {
    if std::env::var("WT_SESSION").is_ok() {
        return Some("wt");
    }
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        return Some("wezterm");
    }
    if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        return Some("alacritty");
    }
    None
}

#[cfg(not(unix))]
fn resume_terminal_candidates_windows() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        push_unique_terminal(&mut candidates, term);
    }
    if let Some(term) = detected_resume_terminal() {
        push_unique_terminal(&mut candidates, term);
    }
    for term in ["wezterm", "wt", "alacritty"] {
        push_unique_terminal(&mut candidates, term);
    }
    candidates
}

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
        crate::process_title::set_client_display_title(
            &session_name,
            super::selfdev::client_selfdev_requested(),
        );
        let icon = id::session_icon(&session_name);
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!("{} jcode {}", icon, session_name))
        );
    } else {
        crate::process_title::set_client_generic_title(super::selfdev::client_selfdev_requested());
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("jcode"));
    }
    startup_profile::mark("terminal_title");

    let mut app = tui::App::new_for_remote(resume_session.clone());
    if server_spawning {
        app.set_server_spawning();
    }
    startup_profile::mark("app_new_for_remote");
    if resume_session.is_none() {
        if let Some(hints) = startup_hints {
            apply_startup_hints(&mut app, hints);
        }
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

    if !has_requested_action(&run_result) {
        if let Some(ref session_id) = run_result.session_id {
            print_session_resume_hint(session_id);
        }
    }

    Ok(())
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

#[cfg(unix)]
pub fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    for term in resume_terminal_candidates_unix() {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match term.as_str() {
            "handterm" => {
                let command = shell_command(&[
                    exe.to_string_lossy().into_owned(),
                    "--resume".to_string(),
                    session_id.to_string(),
                ]);
                cmd.args(["--standalone", "--backend", "gpu", "--exec", &command]);
            }
            "kitty" => {
                let title = resumed_window_title(session_id);
                cmd.args(["--title", &title, "-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id);
            }
            "wezterm" => {
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    exe.to_string_lossy().as_ref(),
                    "--resume",
                    session_id,
                ]);
            }
            "alacritty" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "gnome-terminal" => {
                cmd.args(["--", exe.to_string_lossy().as_ref(), "--resume", session_id]);
            }
            "konsole" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "xterm" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "foot" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "iterm2" => {
                cmd = Command::new("osascript");
                cmd.args([
                    "-e",
                    &format!(
                        r#"tell application "iTerm2"
                            create window with default profile command "{} --resume {}"
                        end tell"#,
                        exe.to_string_lossy(),
                        session_id
                    ),
                ]);
            }
            "terminal" => {
                cmd = Command::new("open");
                cmd.args([
                    "-a",
                    "Terminal",
                    exe.to_str().unwrap_or("jcode"),
                    "--args",
                    "--resume",
                    session_id,
                ]);
            }
            _ => continue,
        }

        if crate::platform::spawn_detached(&mut cmd).is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(unix)]
pub fn spawn_selfdev_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let selfdev_title = format!("{} [self-dev]", resumed_window_title(session_id));

    for term in resume_terminal_candidates_unix() {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let focus_title = match term.as_str() {
            "kitty" | "alacritty" | "gnome-terminal" | "xterm" => Some(selfdev_title.as_str()),
            _ => None,
        };

        match term.as_str() {
            "handterm" => {
                let command = shell_command(&[
                    exe.to_string_lossy().into_owned(),
                    "--resume".to_string(),
                    session_id.to_string(),
                    "self-dev".to_string(),
                ]);
                cmd.args(["--standalone", "--backend", "gpu", "--exec", &command]);
            }
            "kitty" => {
                cmd.args(["--title", selfdev_title.as_str(), "-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("self-dev");
            }
            "wezterm" => {
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    exe.to_string_lossy().as_ref(),
                    "--resume",
                    session_id,
                    "self-dev",
                ]);
            }
            "alacritty" => {
                cmd.args(["--title", selfdev_title.as_str(), "-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("self-dev");
            }
            "gnome-terminal" => {
                cmd.arg("--title").arg(selfdev_title.as_str());
                cmd.args([
                    "--",
                    exe.to_string_lossy().as_ref(),
                    "--resume",
                    session_id,
                    "self-dev",
                ]);
            }
            "konsole" => {
                cmd.args(["-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("self-dev");
            }
            "xterm" => {
                cmd.args(["-T", selfdev_title.as_str(), "-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("self-dev");
            }
            "foot" => {
                cmd.args(["-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("self-dev");
            }
            "iterm2" => {
                cmd = Command::new("osascript");
                let command = format!(
                    "\"{}\" --resume {} self-dev",
                    applescript_escape(exe.to_string_lossy().as_ref()),
                    session_id
                );
                cmd.args([
                    "-e",
                    &format!(
                        r#"tell application "iTerm2"
                            create window with default profile command "{}"
                            activate
                        end tell"#,
                        command
                    ),
                ]);
            }
            "terminal" => {
                cmd = Command::new("osascript");
                let command = format!(
                    "\"{}\" --resume {} self-dev",
                    applescript_escape(exe.to_string_lossy().as_ref()),
                    session_id
                );
                cmd.args([
                    "-e",
                    &format!(
                        r#"tell application "Terminal"
                            activate
                            do script "{}"
                        end tell"#,
                        command
                    ),
                ]);
            }
            _ => continue,
        }

        if crate::platform::spawn_detached(&mut cmd).is_ok() {
            if let Some(title) = focus_title {
                focus_title_best_effort(title);
            }
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
fn find_wezterm_gui_binary() -> Option<String> {
    use std::process::{Command, Stdio};

    if let Ok(exe) = std::env::var("WEZTERM_EXECUTABLE") {
        let p = std::path::Path::new(&exe);
        let gui = p.with_file_name("wezterm-gui.exe");
        if gui.exists() {
            return Some(gui.to_string_lossy().into_owned());
        }
        return Some(exe);
    }

    let candidates = [
        r"C:\Program Files\WezTerm\wezterm-gui.exe",
        r"C:\Program Files (x86)\WezTerm\wezterm-gui.exe",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }

    for bin in &["wezterm-gui", "wezterm"] {
        if let Ok(output) = Command::new("where")
            .arg(bin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.lines().next() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        if *bin == "wezterm" {
                            let p = std::path::Path::new(trimmed);
                            let gui = p.with_file_name("wezterm-gui.exe");
                            if gui.exists() {
                                return Some(gui.to_string_lossy().into_owned());
                            }
                        }
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(not(unix))]
pub fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let wezterm_gui = find_wezterm_gui_binary();
    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    for term in resume_terminal_candidates_windows() {
        let status = match term.as_str() {
            "wezterm" => {
                let Some(ref wezterm_bin) = wezterm_gui else {
                    continue;
                };
                let mut cmd = Command::new(wezterm_bin);
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    &exe.to_string_lossy(),
                    "--resume",
                    session_id,
                ])
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "wt" | "windows-terminal" => {
                if !wt_available {
                    continue;
                }
                let mut cmd = Command::new("wt.exe");
                cmd.args([
                    "-p",
                    "Command Prompt",
                    &exe.to_string_lossy(),
                    "--resume",
                    session_id,
                ])
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "alacritty" => {
                if !alacritty_available {
                    continue;
                }
                let mut cmd = Command::new("alacritty");
                cmd.args(["-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            _ => continue,
        };

        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
pub fn spawn_selfdev_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let wezterm_gui = find_wezterm_gui_binary();
    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    for term in resume_terminal_candidates_windows() {
        let status = match term.as_str() {
            "wezterm" => {
                let Some(ref wezterm_bin) = wezterm_gui else {
                    continue;
                };
                let mut cmd = Command::new(wezterm_bin);
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    &exe.to_string_lossy(),
                    "--resume",
                    session_id,
                    "self-dev",
                ])
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "wt" | "windows-terminal" => {
                if !wt_available {
                    continue;
                }
                let mut cmd = Command::new("wt.exe");
                cmd.args([
                    "-p",
                    "Command Prompt",
                    &exe.to_string_lossy(),
                    "--resume",
                    session_id,
                    "self-dev",
                ])
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "alacritty" => {
                if !alacritty_available {
                    continue;
                }
                let mut cmd = Command::new("alacritty");
                cmd.args(["-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id)
                    .arg("self-dev")
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            _ => continue,
        };

        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn list_sessions() -> Result<()> {
    fn resolve_cli_executable(name: &str) -> std::path::PathBuf {
        let mut search_dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect())
            .unwrap_or_default();
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(home);
            for extra in [".npm-global/bin", ".local/bin", ".cargo/bin"] {
                let path = home.join(extra);
                if !search_dirs.iter().any(|existing| existing == &path) {
                    search_dirs.push(path);
                }
            }
        }
        for dir in search_dirs {
            let path = dir.join(name);
            if path.is_file() {
                return path;
            }
        }
        std::path::PathBuf::from(name)
    }

    fn build_resume_target_command(
        exe: &std::path::Path,
        target: &crate::tui::session_picker::ResumeTarget,
    ) -> (std::path::PathBuf, Vec<String>) {
        match target {
            crate::tui::session_picker::ResumeTarget::JcodeSession { session_id } => (
                exe.to_path_buf(),
                vec!["--resume".to_string(), session_id.clone()],
            ),
            crate::tui::session_picker::ResumeTarget::CodexSession { session_id } => (
                resolve_cli_executable("codex"),
                vec!["resume".to_string(), session_id.clone()],
            ),
            crate::tui::session_picker::ResumeTarget::PiSession { session_path } => (
                resolve_cli_executable("pi"),
                vec!["--session".to_string(), session_path.clone()],
            ),
            crate::tui::session_picker::ResumeTarget::OpenCodeSession { session_id } => (
                resolve_cli_executable("opencode"),
                vec!["--session".to_string(), session_id.clone()],
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
        target: &crate::tui::session_picker::ResumeTarget,
        exe: &std::path::Path,
        cwd: &std::path::Path,
    ) -> Result<bool> {
        use std::process::{Command, Stdio};

        let (program, args) = build_resume_target_command(exe, target);
        let title = match target {
            crate::tui::session_picker::ResumeTarget::JcodeSession { session_id } => {
                resumed_window_title(session_id)
            }
            crate::tui::session_picker::ResumeTarget::CodexSession { session_id } => {
                format!("🧠 Codex {}", &session_id[..session_id.len().min(8)])
            }
            crate::tui::session_picker::ResumeTarget::PiSession { session_path } => {
                format!(
                    "π Pi {}",
                    std::path::Path::new(session_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("session")
                )
            }
            crate::tui::session_picker::ResumeTarget::OpenCodeSession { session_id } => {
                format!("◌ OpenCode {}", &session_id[..session_id.len().min(8)])
            }
        };

        for term in resume_terminal_candidates_unix() {
            let mut cmd = Command::new(&term);
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            match term.as_str() {
                "handterm" => {
                    let command = shell_command(
                        &std::iter::once(program.to_string_lossy().into_owned())
                            .chain(args.iter().cloned())
                            .collect::<Vec<_>>(),
                    );
                    cmd.args(["--standalone", "--backend", "gpu", "--exec", &command]);
                }
                "kitty" => {
                    cmd.args(["--title", &title, "-e"])
                        .arg(&program)
                        .args(&args);
                }
                "wezterm" => {
                    cmd.args([
                        "start",
                        "--always-new-process",
                        "--",
                        program.to_string_lossy().as_ref(),
                    ]);
                    cmd.args(&args);
                }
                "alacritty" => {
                    cmd.args(["--title", &title, "-e"])
                        .arg(&program)
                        .args(&args);
                }
                "gnome-terminal" => {
                    cmd.arg("--title").arg(&title);
                    cmd.arg("--").arg(&program).args(&args);
                }
                "konsole" | "xterm" | "foot" => {
                    cmd.args(["-e"]).arg(&program).args(&args);
                }
                "iterm2" => {
                    cmd = Command::new("osascript");
                    cmd.args([
                        "-e",
                        &format!(
                            r#"tell application "iTerm2"
                                create window with default profile command "{}"
                            end tell"#,
                            shell_command(
                                &std::iter::once(program.to_string_lossy().into_owned())
                                    .chain(args.iter().cloned())
                                    .collect::<Vec<_>>()
                            )
                        ),
                    ]);
                }
                "terminal" => {
                    cmd = Command::new("open");
                    cmd.args([
                        "-a",
                        "Terminal",
                        program.to_str().unwrap_or("jcode"),
                        "--args",
                    ]);
                    cmd.args(&args);
                }
                _ => continue,
            }

            if crate::platform::spawn_detached(&mut cmd).is_ok() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    match tui::session_picker::pick_session()? {
        Some(tui::session_picker::PickerResult::Selected(targets)) => {
            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;

            if targets.len() == 1 {
                let target = &targets[0];
                let mut session_cwd = cwd.clone();
                if let crate::tui::session_picker::ResumeTarget::JcodeSession { session_id } =
                    target
                {
                    if let Ok(sess) = session::Session::load(session_id) {
                        if let Some(dir) = sess.working_dir.as_deref() {
                            if std::path::Path::new(dir).is_dir() {
                                session_cwd = std::path::PathBuf::from(dir);
                            }
                        }
                    }
                }
                let (program, args) = build_resume_target_command(&exe, target);
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
                    let mut session_cwd = cwd.clone();
                    if let crate::tui::session_picker::ResumeTarget::JcodeSession { session_id } =
                        &target
                    {
                        if let Ok(sess) = session::Session::load(session_id) {
                            if let Some(dir) = sess.working_dir.as_deref() {
                                if std::path::Path::new(dir).is_dir() {
                                    session_cwd = std::path::PathBuf::from(dir);
                                }
                            }
                        }
                    }

                    match spawn_target_in_new_terminal(&target, &exe, &session_cwd) {
                        Ok(true) => spawned += 1,
                        Ok(false) => {
                            if !warned_no_terminal {
                                eprintln!(
                                    "No supported terminal emulator found. Run these commands manually:"
                                );
                                warned_no_terminal = true;
                            }
                            let (program, args) = build_resume_target_command(&exe, &target);
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
        Some(tui::session_picker::PickerResult::RestoreAllCrashed) => {
            let recovered = session::recover_crashed_sessions()?;
            if recovered.is_empty() {
                eprintln!("No crashed sessions found.");
                return Ok(());
            }

            eprintln!(
                "Recovered {} crashed session(s) from the last crash window.",
                recovered.len()
            );

            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;
            let mut spawned = 0usize;
            let mut warned_no_terminal = false;

            for session_id in recovered {
                let mut session_cwd = cwd.clone();
                if let Ok(sess) = session::Session::load(&session_id) {
                    if let Some(dir) = sess.working_dir.as_deref() {
                        if std::path::Path::new(dir).is_dir() {
                            session_cwd = std::path::PathBuf::from(dir);
                        }
                    }
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
mod tests {
    #[cfg(unix)]
    use super::{spawn_resume_in_new_terminal, spawn_selfdev_in_new_terminal};
    #[cfg(unix)]
    use crate::platform::set_permissions_executable;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::path::Path;
    #[cfg(unix)]
    use std::thread;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    #[cfg(unix)]
    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    #[cfg(unix)]
    impl EnvVarGuard {
        fn set_path(key: &'static str, value: &Path) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }

        fn set_value(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    #[cfg(unix)]
    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = self.prev.take() {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[cfg(unix)]
    fn write_fake_handterm(temp: &tempfile::TempDir, output_path: &Path) {
        let script_path = temp.path().join("handterm");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" > {}\nprintf '%s\\n' \"$@\" >> {}\n",
            output_path.display(),
            output_path.display()
        );
        fs::write(&script_path, script).expect("write fake handterm script");
        set_permissions_executable(&script_path).expect("make fake handterm executable");
    }

    #[cfg(unix)]
    fn wait_for_lines(path: &Path, min_lines: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(content) = fs::read_to_string(path) {
                let lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
                if lines.len() >= min_lines {
                    return lines;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "timed out waiting for launcher output at {}",
            path.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_resume_in_new_terminal_uses_handterm_exec_mode() {
        let temp = tempfile::tempdir().expect("temp dir");
        let output_path = temp.path().join("resume-launch.txt");
        write_fake_handterm(&temp, &output_path);
        let path = format!(
            "{}:{}",
            temp.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _path_guard = EnvVarGuard::set_value("PATH", &path);
        let _term_guard = EnvVarGuard::set_value("JCODE_TERMINAL", "handterm");

        let exe = temp.path().join("jcode-bin");
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("create cwd");

        let launched =
            spawn_resume_in_new_terminal(&exe, "ses_test_123", &cwd).expect("spawn should work");
        assert!(launched);

        let lines = wait_for_lines(&output_path, 5);
        assert_eq!(lines[0], cwd.to_string_lossy());
        assert_eq!(lines[1], "--standalone");
        assert_eq!(lines[2], "--backend");
        assert_eq!(lines[3], "gpu");
        assert_eq!(lines[4], "--exec");
        assert!(lines[5].contains("--resume"));
        assert!(lines[5].contains("ses_test_123"));
        assert!(lines[5].contains(exe.to_string_lossy().as_ref()));
    }

    #[cfg(unix)]
    #[test]
    fn spawn_selfdev_in_new_terminal_uses_handterm_exec_mode() {
        let temp = tempfile::tempdir().expect("temp dir");
        let output_path = temp.path().join("selfdev-launch.txt");
        write_fake_handterm(&temp, &output_path);
        let path = format!(
            "{}:{}",
            temp.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let _path_guard = EnvVarGuard::set_value("PATH", &path);
        let _term_guard = EnvVarGuard::set_value("JCODE_TERMINAL", "handterm");

        let exe = temp.path().join("jcode-bin");
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("create cwd");

        let launched = spawn_selfdev_in_new_terminal(&exe, "ses_selfdev_123", &cwd)
            .expect("spawn should work");
        assert!(launched);

        let lines = wait_for_lines(&output_path, 5);
        assert_eq!(lines[0], cwd.to_string_lossy());
        assert_eq!(lines[1], "--standalone");
        assert_eq!(lines[2], "--backend");
        assert_eq!(lines[3], "gpu");
        assert_eq!(lines[4], "--exec");
        assert!(lines[5].contains("--resume"));
        assert!(lines[5].contains("ses_selfdev_123"));
        assert!(lines[5].contains("self-dev"));
        assert!(lines[5].contains(exe.to_string_lossy().as_ref()));
    }
}
