use anyhow::{Context, Result};
use std::io::{self, Write};
use std::process::Command as ProcessCommand;
use std::sync::Arc;

use crate::{
    id, logging, provider, replay, server, session, startup_profile, tool, tui, video_export,
};

pub use super::hot_exec::{check_for_updates, get_repo_dir, run_auto_update, run_update};
pub use super::hot_exec::{hot_rebuild, hot_reload, hot_update};

use super::terminal::{
    cleanup_tui_runtime, init_tui_runtime, set_current_session, spawn_session_signal_watchers,
};

pub async fn run_tui(
    provider: Arc<dyn provider::Provider>,
    registry: tool::Registry,
    resume_session: Option<String>,
    debug_socket: bool,
    startup_message: Option<String>,
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
    } else if let Some(msg) = startup_message {
        app.queue_startup_message(msg);
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

    let will_exec = run_result.reload_session.is_some()
        || run_result.rebuild_session.is_some()
        || run_result.update_session.is_some();
    if !will_exec {
        cleanup_tui_runtime(&tui_runtime, true);
    } else {
        cleanup_tui_runtime(&tui_runtime, false);
    }

    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    if let Some(ref rebuild_session_id) = run_result.rebuild_session {
        hot_rebuild(rebuild_session_id)?;
    }

    if let Some(ref update_session_id) = run_result.update_session {
        hot_update(update_session_id)?;
    }

    if run_result.reload_session.is_none()
        && run_result.rebuild_session.is_none()
        && run_result.update_session.is_none()
    {
        eprintln!();
        eprintln!(
            "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
            session_name
        );
        eprintln!("  jcode --resume {}", session_id);
        eprintln!();
    }

    Ok(())
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
    startup_message: Option<String>,
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
        let icon = id::session_icon(&session_name);
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!("{} jcode {}", icon, session_name))
        );
    } else {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("jcode"));
    }
    startup_profile::mark("terminal_title");

    let mut app = tui::App::new_for_remote(resume_session.clone());
    if server_spawning {
        app.set_server_spawning();
    }
    startup_profile::mark("app_new_for_remote");
    if resume_session.is_none() {
        if let Some(msg) = startup_message {
            app.queue_startup_message(msg);
        }
    }

    startup_profile::mark("pre_run_remote");
    startup_profile::report_to_log();

    let result = app.run_remote(terminal).await;

    let run_result = result?;

    let will_exec = run_result.reload_session.is_some()
        || run_result.rebuild_session.is_some()
        || run_result.update_session.is_some();
    if !will_exec {
        cleanup_tui_runtime(&tui_runtime, true);
    } else {
        cleanup_tui_runtime(&tui_runtime, false);
    }

    if let Some(code) = run_result.exit_code {
        std::process::exit(code);
    }

    if let Some(ref reload_session_id) = run_result.reload_session {
        hot_reload(reload_session_id)?;
    }

    if let Some(ref rebuild_session_id) = run_result.rebuild_session {
        hot_rebuild(rebuild_session_id)?;
    }

    if let Some(ref update_session_id) = run_result.update_session {
        hot_update(update_session_id)?;
    }

    if run_result.reload_session.is_none()
        && run_result.rebuild_session.is_none()
        && run_result.update_session.is_none()
    {
        if let Some(ref session_id) = run_result.session_id {
            let session_name = id::extract_session_name(session_id)
                .map(|s| s.to_string())
                .unwrap_or_else(|| session_id.clone());
            eprintln!();
            eprintln!(
                "\x1b[33mSession \x1b[1m{}\x1b[0m\x1b[33m - to resume:\x1b[0m",
                session_name
            );
            eprintln!("  jcode --resume {}", session_id);
            eprintln!();
        }
    }

    Ok(())
}

pub async fn run_replay_command(
    session_id_or_path: &str,
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

    let mut candidates: Vec<String> = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        if !term.trim().is_empty() {
            candidates.push(term);
        }
    }

    #[cfg(target_os = "macos")]
    {
        candidates.extend(
            ["alacritty", "kitty", "wezterm", "iterm2", "terminal"]
                .iter()
                .map(|s| s.to_string()),
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        candidates.extend(
            [
                "alacritty",
                "kitty",
                "wezterm",
                "gnome-terminal",
                "konsole",
                "xterm",
                "foot",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
    }

    for term in candidates {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match term.as_str() {
            "kitty" => {
                cmd.args(["--title", "jcode resume", "-e"])
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

        if cmd.spawn().is_ok() {
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

    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if alacritty_available {
        let status = Command::new("alacritty")
            .args(["-e"])
            .arg(exe)
            .arg("--resume")
            .arg(session_id)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if status.is_ok() {
            return Ok(true);
        }
    }

    let wezterm_gui = find_wezterm_gui_binary();

    if let Some(ref wezterm_bin) = wezterm_gui {
        let status = Command::new(wezterm_bin)
            .args([
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
            .stderr(Stdio::null())
            .spawn();
        if status.is_ok() {
            return Ok(true);
        }
    }

    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    if wt_available {
        let status = Command::new("wt.exe")
            .args([
                "-p",
                "Command Prompt",
                &exe.to_string_lossy(),
                "--resume",
                session_id,
            ])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn list_sessions() -> Result<()> {
    match tui::session_picker::pick_session()? {
        Some(tui::session_picker::PickerResult::Selected(session_id)) => {
            let exe = std::env::current_exe()?;
            let cwd = std::env::current_dir()?;

            let err = crate::platform::replace_process(
                ProcessCommand::new(&exe)
                    .arg("--resume")
                    .arg(&session_id)
                    .current_dir(cwd),
            );

            Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
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
                            eprintln!("No supported terminal emulator found. Run these commands manually:");
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
