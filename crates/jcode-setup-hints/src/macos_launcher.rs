use super::{
    MacTerminalKind, SetupHintsState, effective_macos_terminal, escape_applescript_text,
    escape_shell_single_quotes, launch_command_for_macos_terminal, paused_jcode_shell_command,
    save_preferred_macos_terminal,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const MACOS_APP_ICON_FILE_NAME: &str = "Jcode.icns";
const MACOS_APP_ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/app-icons/Jcode.icns"
));
const MACOS_NOTIFICATION_APP_NAME: &str = "Jcode Notifications.app";
const MACOS_NOTIFICATION_EXECUTABLE: &str = "jcode-notification-broker";
const MACOS_NOTIFICATION_VERSION_MARKER: &str = "jcode-broker-version";

pub(super) fn should_refresh_macos_app_launcher(state: &SetupHintsState) -> bool {
    match (
        macos_app_launcher_dir(),
        legacy_macos_app_launcher_dir(),
        macos_notification_broker_dir(),
    ) {
        (Ok(app_dir), Ok(legacy_app_dir), Ok(broker_dir)) => {
            should_refresh_macos_app_launcher_paths(state, &app_dir, &legacy_app_dir)
                || !macos_notification_broker_is_valid(&broker_dir)
        }
        _ => !state.desktop_shortcut_created,
    }
}

pub(super) fn install_macos_app_launcher() -> Result<(PathBuf, MacTerminalKind)> {
    let app_dir = macos_app_launcher_dir()?;
    let legacy_app_dir = legacy_macos_app_launcher_dir()?;

    if app_dir.exists() && !macos_app_launcher_is_valid(&app_dir) {
        remove_path_if_exists(&app_dir)?;
    }
    if legacy_app_dir != app_dir && legacy_app_dir.exists() {
        remove_path_if_exists(&legacy_app_dir)?;
    }

    let contents_dir = app_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    let resources_dir = contents_dir.join("Resources");
    std::fs::create_dir_all(&macos_dir)?;
    std::fs::create_dir_all(&resources_dir)?;

    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy().into_owned();
    let terminal = effective_macos_terminal();
    let launcher_path = macos_dir.join("jcode-launcher");
    let launcher_script = macos_launcher_script(terminal, &exe_path, &app_dir);
    std::fs::write(&launcher_path, launcher_script)?;
    std::fs::write(
        resources_dir.join(MACOS_APP_ICON_FILE_NAME),
        MACOS_APP_ICON_BYTES,
    )?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let info_plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Jcode</string>
    <key>CFBundleDisplayName</key>
    <string>Jcode</string>
    <key>CFBundleIdentifier</key>
    <string>com.jcode.launcher</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleExecutable</key>
    <string>jcode-launcher</string>
    <key>CFBundleIconFile</key>
    <string>{icon_file}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
</dict>
</plist>
"#,
        version = jcode_build_meta::version(),
        icon_file = MACOS_APP_ICON_FILE_NAME,
    );
    std::fs::write(contents_dir.join("Info.plist"), info_plist)?;

    install_macos_notification_broker(&exe)?;

    if !macos_app_launcher_is_valid(&app_dir) {
        anyhow::bail!(
            "launcher bundle is incomplete after setup: {}",
            app_dir.display()
        );
    }

    register_macos_app_launcher(&app_dir);
    save_preferred_macos_terminal(terminal)?;
    Ok((app_dir, terminal))
}

fn macos_app_launcher_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join("Applications").join("Jcode.app"))
}

fn legacy_macos_app_launcher_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join("Applications").join("jcode.app"))
}

fn macos_notification_broker_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join("Applications").join(MACOS_NOTIFICATION_APP_NAME))
}

fn macos_notification_broker_executable_path(app_dir: &Path) -> PathBuf {
    app_dir
        .join("Contents")
        .join("MacOS")
        .join(MACOS_NOTIFICATION_EXECUTABLE)
}

fn macos_notification_broker_marker_path(app_dir: &Path) -> PathBuf {
    app_dir
        .join("Contents")
        .join("Resources")
        .join(MACOS_NOTIFICATION_VERSION_MARKER)
}

fn macos_notification_broker_icon_path(app_dir: &Path) -> PathBuf {
    app_dir
        .join("Contents")
        .join("Resources")
        .join(MACOS_APP_ICON_FILE_NAME)
}

fn macos_notification_broker_is_valid(app_dir: &Path) -> bool {
    app_dir.is_dir()
        && app_dir.join("Contents").join("Info.plist").is_file()
        && macos_notification_broker_executable_path(app_dir).is_file()
        && macos_notification_broker_icon_path(app_dir).is_file()
        && std::fs::read_to_string(macos_notification_broker_marker_path(app_dir))
            .is_ok_and(|version| version.trim() == jcode_build_meta::version())
}

/// Install a faceless Notification Center owner using the already-built jcode
/// executable as a multicall binary. Hard-linking keeps the generated helper
/// tiny and architecture-correct for both Intel and Apple Silicon artifacts;
/// copying is the reliable fallback when ~/Applications is on another volume.
fn install_macos_notification_broker(jcode_executable: &Path) -> Result<PathBuf> {
    let app_dir = macos_notification_broker_dir()?;
    let parent = app_dir
        .parent()
        .context("notification broker bundle has no parent")?;
    std::fs::create_dir_all(parent)?;

    let staging = parent.join(format!(
        ".Jcode Notifications.app.installing-{}",
        std::process::id()
    ));
    remove_path_if_exists(&staging)?;
    let contents = staging.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;

    let broker_executable = macos.join(MACOS_NOTIFICATION_EXECUTABLE);
    if std::fs::hard_link(jcode_executable, &broker_executable).is_err() {
        std::fs::copy(jcode_executable, &broker_executable).with_context(|| {
            format!(
                "failed to install notification broker executable from {}",
                jcode_executable.display()
            )
        })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&broker_executable, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::write(
        resources.join(MACOS_APP_ICON_FILE_NAME),
        MACOS_APP_ICON_BYTES,
    )?;
    std::fs::write(
        resources.join(MACOS_NOTIFICATION_VERSION_MARKER),
        format!("{}\n", jcode_build_meta::version()),
    )?;
    std::fs::write(contents.join("Info.plist"), macos_notification_info_plist())?;

    if !macos_notification_broker_is_valid(&staging) {
        remove_path_if_exists(&staging)?;
        anyhow::bail!(
            "notification broker bundle is incomplete after setup: {}",
            staging.display()
        );
    }

    remove_path_if_exists(&app_dir)?;
    std::fs::rename(&staging, &app_dir).with_context(|| {
        format!(
            "failed to publish notification broker bundle at {}",
            app_dir.display()
        )
    })?;
    register_macos_app_launcher(&app_dir);
    Ok(app_dir)
}

fn macos_notification_info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Jcode Notifications</string>
    <key>CFBundleDisplayName</key>
    <string>Jcode Notifications</string>
    <key>CFBundleIdentifier</key>
    <string>com.jcode.notifications</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleExecutable</key>
    <string>{executable}</string>
    <key>CFBundleIconFile</key>
    <string>{icon}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSMultipleInstancesProhibited</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#,
        version = jcode_build_meta::version(),
        executable = MACOS_NOTIFICATION_EXECUTABLE,
        icon = MACOS_APP_ICON_FILE_NAME,
    )
}

fn macos_app_launcher_info_plist_path(app_dir: &Path) -> PathBuf {
    app_dir.join("Contents").join("Info.plist")
}

fn macos_app_launcher_executable_path(app_dir: &Path) -> PathBuf {
    app_dir
        .join("Contents")
        .join("MacOS")
        .join("jcode-launcher")
}

fn macos_app_launcher_icon_path(app_dir: &Path) -> PathBuf {
    app_dir
        .join("Contents")
        .join("Resources")
        .join(MACOS_APP_ICON_FILE_NAME)
}

fn macos_app_launcher_is_valid(app_dir: &Path) -> bool {
    app_dir.is_dir()
        && macos_app_launcher_info_plist_path(app_dir).is_file()
        && macos_app_launcher_executable_path(app_dir).is_file()
        && macos_app_launcher_icon_path(app_dir).is_file()
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect existing path {}", path.display()))?;
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove directory {}", path.display()))?;
    } else {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove file {}", path.display()))?;
    }
    Ok(())
}

fn register_macos_app_launcher(app_dir: &Path) {
    let _ = std::process::Command::new("touch").arg(app_dir).status();

    let lsregister = Path::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    );
    if lsregister.exists() {
        let _ = std::process::Command::new(lsregister)
            .args(["-f", app_dir.to_string_lossy().as_ref()])
            .status();
    }

    let _ = std::process::Command::new("mdimport").arg(app_dir).status();
}

fn should_refresh_macos_app_launcher_paths(
    state: &SetupHintsState,
    app_dir: &Path,
    legacy_app_dir: &Path,
) -> bool {
    !state.desktop_shortcut_created
        || !macos_app_launcher_is_valid(app_dir)
        || path_exists_with_exact_name(legacy_app_dir)
}

/// Check that `path` exists under its exact byte-for-byte file name.
///
/// macOS system volumes are case-insensitive by default, so a plain
/// `Path::exists()` on `jcode.app` also matches `Jcode.app`. The legacy-bundle
/// check needs an exact-name match or the launcher would refresh itself on
/// every launch once the new bundle exists.
fn path_exists_with_exact_name(path: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return path.exists();
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };
    entries
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.file_name() == name)
}

fn macos_launcher_script(terminal: MacTerminalKind, exe_path: &str, app_dir: &Path) -> String {
    let app_dir_escaped = escape_shell_single_quotes(&app_dir.to_string_lossy());
    let exe_path_escaped = escape_shell_single_quotes(exe_path);
    let shell_command = paused_jcode_shell_command(exe_path);
    let launch_command = launch_command_for_macos_terminal(terminal, &shell_command);
    let missing_message = escape_applescript_text(&format!(
        "Jcode could not launch because the executable was not found.\n\nExpected path:\n{}\n\nTry reinstalling jcode or rerun:\njcode setup-launcher",
        exe_path
    ));
    let terminal_failure_message = escape_applescript_text(&format!(
        "Jcode could not open {}.\n\nTry rerunning:\njcode setup-launcher\n\nLauncher log:\n~/.jcode/launcher/macos-launcher.log",
        terminal.label()
    ));

    format!(
        r#"#!/bin/bash
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
LOG_DIR="$HOME/.jcode/launcher"
LOG_FILE="$LOG_DIR/macos-launcher.log"
mkdir -p "$LOG_DIR" >/dev/null 2>&1 || true

show_missing_executable() {{
  /usr/bin/osascript <<'APPLESCRIPT' >/dev/null 2>&1 || true
display alert "Jcode launch failed" message "{missing_message}" as critical
APPLESCRIPT
}}

show_terminal_launch_failure() {{
  /usr/bin/osascript <<'APPLESCRIPT' >/dev/null 2>&1 || true
display alert "Jcode launch failed" message "{terminal_failure_message}" as critical
APPLESCRIPT
}}

if [ ! -x '{exe_path_escaped}' ]; then
  printf '[%s] missing executable: {exe_path}\n' "$(date '+%Y-%m-%d %H:%M:%S')" >>"$LOG_FILE" 2>&1
  show_missing_executable
  exit 1
fi

{launch_command} >>"$LOG_FILE" 2>&1
status=$?
if [ "$status" -ne 0 ]; then
  printf '[%s] terminal launch failed for {terminal_name} (status %s)\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$status" >>"$LOG_FILE" 2>&1
  show_terminal_launch_failure
  exit "$status"
fi

/usr/bin/touch '{app_dir_escaped}' >/dev/null 2>&1 || true
exit 0
"#,
        missing_message = missing_message,
        terminal_failure_message = terminal_failure_message,
        exe_path = exe_path,
        exe_path_escaped = exe_path_escaped,
        terminal_name = terminal.label(),
        launch_command = launch_command,
        app_dir_escaped = app_dir_escaped,
    )
}

#[cfg(test)]
#[path = "macos_launcher_tests.rs"]
mod macos_launcher_tests;
