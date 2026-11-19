//! `/desktop`: open the current session in Jcode Desktop.
//!
//! Desktop accepts `--single-panel --session=<id>`. The first launch becomes
//! the shared single-panel host, and later launches forward their arguments
//! over its socket and exit, so this always opens one new window showing the
//! session without disturbing the user's main workspace.

use std::path::{Path, PathBuf};

use super::{App, DisplayMessage};

/// Environment override for the Desktop executable. The kcode spelling wins;
/// the jcode spelling stays accepted so existing scripts and shell profiles
/// keep working.
const DESKTOP_BIN_ENV: &str = "KCODE_DESKTOP_BIN";
const LEGACY_DESKTOP_BIN_ENV: &str = "JCODE_DESKTOP_BIN";

/// Working directory handed to the Desktop app.
const DESKTOP_WORKING_DIR_ENV: &str = "KCODE_DESKTOP_WORKING_DIR";
const LEGACY_DESKTOP_WORKING_DIR_ENV: &str = "JCODE_DESKTOP_WORKING_DIR";

/// Desktop executable names, in priority order. `kcode-desktop` is this build's
/// own name; `jcode-desktop` is what the shipped Desktop packages are called, so
/// a kcode CLI can still drive an installed Jcode Desktop.
fn desktop_binary_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["kcode-desktop.exe", "jcode-desktop.exe"]
    } else {
        &["kcode-desktop", "jcode-desktop"]
    }
}

/// The name to mention in user-facing messages.
fn desktop_binary_name() -> &'static str {
    desktop_binary_names()[0]
}

fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Candidate locations for the Desktop executable, in priority order.
fn desktop_binary_candidates(
    env_override: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    path_var: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = env_override {
        out.push(path);
    }
    // Desktop packages ship the CLI next to the desktop executable.
    if let Some(dir) = current_exe.as_deref().and_then(Path::parent) {
        out.extend(desktop_binary_names().iter().map(|name| dir.join(name)));
    }
    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(&path_var) {
            out.extend(desktop_binary_names().iter().map(|name| dir.join(name)));
        }
    }
    if cfg!(target_os = "macos") {
        let app_dirs = ["KCode.app", "Jcode.app"];
        for app in app_dirs {
            let macos = PathBuf::from("/Applications")
                .join(app)
                .join("Contents/MacOS");
            out.extend(desktop_binary_names().iter().map(|name| macos.join(name)));
        }
        if let Some(home) = &home {
            for app in app_dirs {
                let macos = home.join("Applications").join(app).join("Contents/MacOS");
                out.extend(desktop_binary_names().iter().map(|name| macos.join(name)));
            }
        }
    }
    out
}

pub(super) fn find_desktop_binary() -> Option<PathBuf> {
    let env_override = std::env::var_os(DESKTOP_BIN_ENV)
        .or_else(|| std::env::var_os(LEGACY_DESKTOP_BIN_ENV))
        .map(PathBuf::from);
    desktop_binary_candidates(
        env_override,
        std::env::current_exe().ok(),
        std::env::var_os("PATH"),
        dirs::home_dir(),
    )
    .into_iter()
    .find(|path| is_executable(path))
}

pub(super) fn desktop_launch_args(session_id: &str) -> Vec<String> {
    vec![
        "--single-panel".to_string(),
        format!("--session={session_id}"),
    ]
}

fn spawn_detached(
    binary: &Path,
    args: &[String],
    working_dir: Option<&Path>,
) -> std::io::Result<()> {
    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = working_dir {
        command
            .env(DESKTOP_WORKING_DIR_ENV, dir)
            .env(LEGACY_DESKTOP_WORKING_DIR_ENV, dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Keep Desktop alive after this terminal closes.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn()?;
    // Reap in the background so the forwarding launcher never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub(super) fn handle_desktop_command(app: &mut App, trimmed: &str) -> bool {
    let mut words = trimmed.split_whitespace();
    if words.next() != Some("/desktop") {
        return false;
    }
    if words.next().is_some() {
        app.push_display_message(DisplayMessage::error(
            "Usage: /desktop (opens this session in Jcode Desktop)".to_string(),
        ));
        return true;
    }

    let session_id = super::commands::active_session_id(app);
    if !app.is_remote && app.session.id == session_id {
        // Desktop reads the transcript from disk, so flush it first.
        let _ = app.session.save();
    }

    let Some(binary) = find_desktop_binary() else {
        app.push_display_message(DisplayMessage::error(format!(
            "Jcode Desktop was not found. Install it from https://jcode.sh/desktop, \
             put `{}` on PATH, or set {DESKTOP_BIN_ENV}.",
            desktop_binary_name()
        )));
        return true;
    };

    let working_dir = app.session.working_dir.as_deref().map(Path::new);
    match spawn_detached(&binary, &desktop_launch_args(&session_id), working_dir) {
        Ok(()) => {
            app.push_display_message(DisplayMessage::system(format!(
                "Opening session `{session_id}` in Jcode Desktop."
            )));
            app.set_status_notice("Opened in Desktop");
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to launch Jcode Desktop ({}): {error}",
            binary.display()
        ))),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_open_session_in_single_panel_window() {
        assert_eq!(
            desktop_launch_args("session_abc"),
            ["--single-panel", "--session=session_abc"]
        );
    }

    #[test]
    fn candidates_prefer_override_then_sibling_then_path() {
        let path = std::env::join_paths(["/p1", "/p2"]).unwrap();
        let candidates = desktop_binary_candidates(
            Some(PathBuf::from("/override/jd")),
            Some(PathBuf::from("/bundle/jcode")),
            Some(path),
            None,
        );
        let names = desktop_binary_names();
        assert_eq!(candidates[0], PathBuf::from("/override/jd"));
        assert_eq!(candidates[1], Path::new("/bundle").join(names[0]));
        assert_eq!(candidates[2], Path::new("/bundle").join(names[1]));
        assert_eq!(candidates[3], Path::new("/p1").join(names[0]));
        assert_eq!(candidates[4], Path::new("/p1").join(names[1]));
        assert_eq!(candidates[5], Path::new("/p2").join(names[0]));
        assert_eq!(candidates[6], Path::new("/p2").join(names[1]));
        assert_eq!(candidates.len(), 7);
    }

    /// Location still ranks above name: every name in one directory is tried
    /// before the next directory, and `kcode-desktop` is tried first.
    #[test]
    fn kcode_desktop_name_is_tried_before_the_jcode_one() {
        let names = desktop_binary_names();
        assert!(names[0].starts_with("kcode-desktop"));
        assert!(names[1].starts_with("jcode-desktop"));
        let candidates =
            desktop_binary_candidates(None, Some(PathBuf::from("/bundle/kcode")), None, None);
        assert_eq!(candidates[0], Path::new("/bundle").join(names[0]));
        assert_eq!(candidates[1], Path::new("/bundle").join(names[1]));
    }
}
