//! Platform setup hints shown on startup.
//!
//! - Windows: suggest Alt+; hotkey setup and Alacritty install.
//! - macOS: detect suboptimal terminal and offer guided Ghostty setup via jcode.
//! - Linux: create a .desktop launcher file.
//!
//! Each nudge can be dismissed permanently with "Don't ask again".
//! State is persisted in `~/.jcode/setup_hints.json`.

use crate::storage;
#[cfg(any(test, target_os = "macos"))]
use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
#[cfg(any(test, target_os = "macos"))]
use std::fmt;
#[cfg(any(windows, target_os = "macos"))]
use std::io::Write;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupHintsState {
    pub launch_count: u64,
    pub hotkey_configured: bool,
    pub hotkey_dismissed: bool,
    #[serde(alias = "wezterm_configured")]
    pub alacritty_configured: bool,
    #[serde(alias = "wezterm_dismissed")]
    pub alacritty_dismissed: bool,
    #[serde(default)]
    pub desktop_shortcut_created: bool,
    #[serde(default)]
    pub startup_spawn_hint_dismissed: bool,
    pub mac_ghostty_guided: bool,
    pub mac_ghostty_dismissed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StartupHints {
    pub auto_send_message: Option<String>,
    pub status_notice: Option<String>,
    pub display_message: Option<(String, String)>,
}

impl StartupHints {
    fn none() -> Option<Self> {
        None
    }

    #[cfg(target_os = "macos")]
    fn is_empty(&self) -> bool {
        self.auto_send_message.is_none()
            && self.status_notice.is_none()
            && self.display_message.is_none()
    }

    fn with_spawn_notice(message: String) -> Self {
        Self {
            auto_send_message: None,
            status_notice: Some(message.clone()),
            display_message: Some(("Launch".to_string(), message)),
        }
    }

    fn with_status_and_display(
        status_notice: String,
        title: impl Into<String>,
        display_message: String,
    ) -> Self {
        Self {
            auto_send_message: None,
            status_notice: Some(status_notice),
            display_message: Some((title.into(), display_message)),
        }
    }
}

impl SetupHintsState {
    fn path() -> Result<PathBuf> {
        Ok(storage::jcode_dir()?.join("setup_hints.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| storage::read_json(&p).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        storage::write_json(&path, self)
    }
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacTerminalKind {
    Ghostty,
    Iterm2,
    AppleTerminal,
    WezTerm,
    Warp,
    Alacritty,
    Vscode,
    Unknown,
}

#[cfg(any(test, target_os = "macos"))]
impl MacTerminalKind {
    fn label(self) -> &'static str {
        match self {
            Self::Ghostty => "Ghostty",
            Self::Iterm2 => "iTerm2",
            Self::AppleTerminal => "Terminal.app",
            Self::WezTerm => "WezTerm",
            Self::Warp => "Warp",
            Self::Alacritty => "Alacritty",
            Self::Vscode => "VS Code terminal",
            Self::Unknown => "your current terminal",
        }
    }

    fn cli_value(self) -> &'static str {
        match self {
            Self::Ghostty => "ghostty",
            Self::Iterm2 => "iterm2",
            Self::AppleTerminal => "terminal",
            Self::WezTerm => "wezterm",
            Self::Warp => "warp",
            Self::Alacritty => "alacritty",
            Self::Vscode => "vscode",
            Self::Unknown => "terminal",
        }
    }

    fn from_cli_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ghostty" => Some(Self::Ghostty),
            "iterm2" | "iterm" => Some(Self::Iterm2),
            "terminal" | "terminal.app" | "apple_terminal" => Some(Self::AppleTerminal),
            "wezterm" => Some(Self::WezTerm),
            "warp" => Some(Self::Warp),
            "alacritty" => Some(Self::Alacritty),
            "vscode" | "code" => Some(Self::Vscode),
            _ => None,
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
impl fmt::Display for MacTerminalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MacTerminalPreference {
    terminal: String,
}

#[cfg(any(test, target_os = "macos"))]
fn mac_terminal_pref_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("preferred_terminal.json"))
}

#[cfg(any(test, target_os = "macos"))]
fn load_preferred_macos_terminal() -> Option<MacTerminalKind> {
    let path = mac_terminal_pref_path().ok()?;
    let pref: MacTerminalPreference = storage::read_json(&path).ok()?;
    MacTerminalKind::from_cli_value(&pref.terminal)
}

#[cfg(any(test, target_os = "macos"))]
fn save_preferred_macos_terminal(terminal: MacTerminalKind) -> Result<()> {
    let path = mac_terminal_pref_path()?;
    storage::write_json(
        &path,
        &MacTerminalPreference {
            terminal: terminal.cli_value().to_string(),
        },
    )
}

#[cfg(any(test, target_os = "macos"))]
fn effective_macos_terminal() -> MacTerminalKind {
    load_preferred_macos_terminal().unwrap_or_else(detect_macos_terminal)
}

#[cfg(any(test, target_os = "macos"))]
fn detect_macos_terminal() -> MacTerminalKind {
    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();

    if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || std::env::var("GHOSTTY_BIN_DIR").is_ok()
        || term_program == "ghostty"
        || term.contains("ghostty")
    {
        return MacTerminalKind::Ghostty;
    }

    match term_program.as_str() {
        "iterm.app" => MacTerminalKind::Iterm2,
        "apple_terminal" => MacTerminalKind::AppleTerminal,
        "wezterm" => MacTerminalKind::WezTerm,
        "vscode" => MacTerminalKind::Vscode,
        _ => {
            if term.contains("alacritty") {
                MacTerminalKind::Alacritty
            } else if term.contains("warp") {
                MacTerminalKind::Warp
            } else {
                MacTerminalKind::Unknown
            }
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
fn is_ghostty_installed() -> bool {
    if std::path::Path::new("/Applications/Ghostty.app").exists() {
        return true;
    }

    if let Some(home) = dirs::home_dir() {
        if home.join("Applications/Ghostty.app").exists() {
            return true;
        }
    }

    std::process::Command::new("which")
        .arg("ghostty")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Detect which terminal the user is currently running in (Windows).
#[cfg(windows)]
fn detect_terminal() -> &'static str {
    if std::env::var("WT_SESSION").is_ok() {
        "windows-terminal"
    } else if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        "wezterm"
    } else if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        "alacritty"
    } else {
        "unknown"
    }
}

/// Check if Alacritty is installed.
#[cfg(windows)]
fn is_alacritty_installed() -> bool {
    std::process::Command::new("where")
        .arg("alacritty")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if winget is available (Windows).
#[cfg(windows)]
fn is_winget_available() -> bool {
    std::process::Command::new("where")
        .arg("winget")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Find the full path to Alacritty binary.
#[cfg(windows)]
fn find_alacritty_path() -> Option<String> {
    let candidates = [
        r"C:\Program Files\Alacritty\alacritty.exe",
        r"C:\Program Files (x86)\Alacritty\alacritty.exe",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = format!(r"{}\Microsoft\WinGet\Links\alacritty.exe", local);
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    let output = std::process::Command::new("where")
        .arg("alacritty")
        .output()
        .ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = stdout.lines().next() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(any(test, target_os = "macos"))]
fn mac_hotkey_support_dir() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("hotkey"))
}

#[cfg(any(test, target_os = "macos"))]
fn mac_hotkey_launch_agent_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join("com.jcode.hotkey.plist"))
}

#[cfg(any(test, target_os = "macos"))]
fn escape_shell_single_quotes(input: &str) -> String {
    input.replace('\'', r#"'\''"#)
}

#[cfg(any(test, target_os = "macos"))]
fn escape_applescript_text(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(any(test, target_os = "macos"))]
fn paused_jcode_shell_command(exe_path: &str) -> String {
    let escaped_exe = escape_shell_single_quotes(exe_path);
    format!(
        "if [ ! -x '{exe}' ]; then printf 'jcode executable not found.\\n'; exit 127; fi; '{exe}'; status=$?; if [ \"$status\" -ne 0 ]; then printf '\\nJcode exited with status %s.\\n' \"$status\"; printf 'Press Enter to close... '; read -r _; fi; exit \"$status\"",
        exe = escaped_exe,
    )
}

#[cfg(any(test, target_os = "macos"))]
fn launch_command_for_macos_terminal(terminal: MacTerminalKind, shell_command: &str) -> String {
    let escaped_shell = escape_shell_single_quotes(shell_command);
    match terminal {
        MacTerminalKind::Ghostty => {
            format!("/usr/bin/open -na Ghostty --args -e /bin/bash -lc '{escaped_shell}'")
        }
        MacTerminalKind::Alacritty => {
            format!("/usr/bin/open -na Alacritty --args -e /bin/bash -lc '{escaped_shell}'")
        }
        MacTerminalKind::WezTerm => format!(
            "/usr/bin/open -na WezTerm --args start --always-new-process -- /bin/bash -lc '{escaped_shell}'"
        ),
        MacTerminalKind::Iterm2 => format!(
            "/usr/bin/osascript <<'APPLESCRIPT'\ntell application \"iTerm2\"\n    create window with default profile command \"{}\"\n    activate\nend tell\nAPPLESCRIPT",
            escape_applescript_text(shell_command)
        ),
        MacTerminalKind::AppleTerminal
        | MacTerminalKind::Warp
        | MacTerminalKind::Vscode
        | MacTerminalKind::Unknown => format!(
            "/usr/bin/osascript <<'APPLESCRIPT'\ntell application \"Terminal\"\n    activate\n    do script \"{}\"\nend tell\nAPPLESCRIPT",
            escape_applescript_text(shell_command)
        ),
    }
}

#[cfg(any(test, target_os = "macos"))]
fn launch_script_for_macos_terminal(terminal: MacTerminalKind, shell_command: &str) -> String {
    format!(
        "#!/bin/bash\nset -e\n{}\n",
        launch_command_for_macos_terminal(terminal, shell_command)
    )
}

#[cfg(target_os = "macos")]
fn install_macos_hotkey_listener(
    preferred_terminal: Option<MacTerminalKind>,
) -> Result<MacTerminalKind> {
    let terminal = preferred_terminal.unwrap_or_else(effective_macos_terminal);
    let hotkey_dir = mac_hotkey_support_dir()?;
    std::fs::create_dir_all(&hotkey_dir)?;

    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy().into_owned();
    let shell_command = paused_jcode_shell_command(&exe_path);

    let launch_script_path = hotkey_dir.join("launch_jcode.sh");
    std::fs::write(
        &launch_script_path,
        launch_script_for_macos_terminal(terminal, &shell_command),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launch_script_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let plist_path = mac_hotkey_launch_agent_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let plist = format!(
        r#"<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
    <key>Label</key>
    <string>com.jcode.hotkey</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>setup-hotkey</string>
        <string>--listen-macos-hotkey</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout_path}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_path}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>JCODE_PREFERRED_TERMINAL</key>
        <string>{terminal}</string>
    </dict>
</dict>
</plist>
"#,
        exe = exe_path,
        stdout_path = hotkey_dir.join("mac_hotkey.out.log").display(),
        stderr_path = hotkey_dir.join("mac_hotkey.err.log").display(),
        terminal = terminal.cli_value(),
    );
    std::fs::write(&plist_path, plist)?;

    save_preferred_macos_terminal(terminal)?;

    let _ = std::process::Command::new("launchctl")
        .args(["unload", plist_path.to_string_lossy().as_ref()])
        .status();
    let status = std::process::Command::new("launchctl")
        .args(["load", "-w", plist_path.to_string_lossy().as_ref()])
        .status()
        .context("failed to load jcode LaunchAgent")?;
    if !status.success() {
        anyhow::bail!("launchctl load failed with exit code {:?}", status.code());
    }

    Ok(terminal)
}

#[cfg(target_os = "macos")]
fn startup_spawn_notice(state: &SetupHintsState) -> Option<String> {
    if !state.hotkey_configured || state.startup_spawn_hint_dismissed {
        return None;
    }
    Some(format!(
        "Press Alt+; from anywhere to open jcode in {}.",
        effective_macos_terminal().label()
    ))
}

#[cfg(not(target_os = "macos"))]
fn startup_spawn_notice(_state: &SetupHintsState) -> Option<String> {
    None
}

fn startup_hints_for_launch(state: &SetupHintsState) -> Option<StartupHints> {
    let spawn_notice = startup_spawn_notice(state);

    if state.launch_count == 1 {
        let mut message = "Tip: use `/alignment left` to switch to Claude-style left alignment, or press `Alt+C` to toggle left/centered for the current session.".to_string();

        if let Some(spawn_notice) = spawn_notice {
            message.push_str("\n\n");
            message.push_str(&spawn_notice);
        }

        return Some(StartupHints::with_status_and_display(
            "Tip: `/alignment left` or Alt+C toggles alignment.".to_string(),
            "Alignment",
            message,
        ));
    }

    if state.launch_count <= 3 {
        let config_path = crate::config::Config::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.jcode/config.toml".to_string());

        let mut message = format!(
            "You can hotswap text alignment with `Alt+C` (left-aligned ↔ centered).\n\nTo save it permanently, use `/alignment centered` or `/alignment left`. You can also change it in `{}` with `display.centered = true` or `display.centered = false`.",
            config_path
        );

        if let Some(spawn_notice) = spawn_notice {
            message.push_str("\n\n");
            message.push_str(&spawn_notice);
        }

        return Some(StartupHints::with_status_and_display(
            "Tip: Alt+C toggles left/center alignment.".to_string(),
            "Welcome",
            message,
        ));
    }

    spawn_notice.map(StartupHints::with_spawn_notice)
}

/// Create a global Alt+; hotkey using a background PowerShell listener.
///
/// Windows .lnk shortcut hotkeys only support Ctrl+Alt+<letter/number/Fkey>,
/// so Alt+; requires a different approach: a small PowerShell script that calls
/// the Win32 RegisterHotKey API and listens for WM_HOTKEY messages.
///
/// The script is placed in ~/.jcode/hotkey/ and a startup shortcut is created
/// so it runs automatically on login.
#[cfg(windows)]
fn create_hotkey_shortcut(use_alacritty: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy();

    let (launch_exe, launch_args) = if use_alacritty {
        let alacritty_path = find_alacritty_path().unwrap_or_else(|| "alacritty".to_string());
        (alacritty_path, format!("-e \"{}\"", exe_path))
    } else {
        (
            "wt.exe".to_string(),
            format!("-p \"Command Prompt\" \"{}\"", exe_path),
        )
    };

    let hotkey_dir = storage::jcode_dir()?.join("hotkey");
    std::fs::create_dir_all(&hotkey_dir)?;

    let _ = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Process powershell, pwsh -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -like '*jcode-hotkey*' } | Stop-Process -Force -ErrorAction SilentlyContinue",
        ])
        .output();

    let ps1_path = hotkey_dir.join("jcode-hotkey.ps1");
    let ps1_content = format!(
        r#"# jcode Alt+; global hotkey listener
# Auto-generated by jcode setup-hotkey. Runs at login via startup shortcut.
# Uses RegisterHotKey Win32 API to capture Alt+Semicolon globally.

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class HotKeyHelper {{
    [DllImport("user32.dll")]
    public static extern bool RegisterHotKey(IntPtr hWnd, int id, uint fsModifiers, uint vk);
    [DllImport("user32.dll")]
    public static extern bool UnregisterHotKey(IntPtr hWnd, int id);
    [DllImport("user32.dll")]
    public static extern int GetMessage(out MSG lpMsg, IntPtr hWnd, uint wMsgFilterMin, uint wMsgFilterMax);
    [StructLayout(LayoutKind.Sequential)]
    public struct MSG {{
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public int pt_x;
        public int pt_y;
    }}
}}
"@

$MOD_ALT = 0x0001
$MOD_NOREPEAT = 0x4000
$VK_OEM_1 = 0xBA  # semicolon/colon key
$WM_HOTKEY = 0x0312
$HOTKEY_ID = 0x4A43  # "JC"

if (-not [HotKeyHelper]::RegisterHotKey([IntPtr]::Zero, $HOTKEY_ID, $MOD_ALT -bor $MOD_NOREPEAT, $VK_OEM_1)) {{
    Write-Error "Failed to register Alt+; hotkey (another program may have claimed it)"
    exit 1
}}

try {{
    $msg = New-Object HotKeyHelper+MSG
    while ([HotKeyHelper]::GetMessage([ref]$msg, [IntPtr]::Zero, $WM_HOTKEY, $WM_HOTKEY) -ne 0) {{
        if ($msg.message -eq $WM_HOTKEY -and $msg.wParam.ToInt32() -eq $HOTKEY_ID) {{
            Start-Process '{launch_exe}' -ArgumentList '{launch_args}'
        }}
    }}
}} finally {{
    [HotKeyHelper]::UnregisterHotKey([IntPtr]::Zero, $HOTKEY_ID)
}}
"#,
        launch_exe = launch_exe,
        launch_args = launch_args,
    );

    std::fs::write(&ps1_path, &ps1_content)?;

    let startup_dir = format!(
        "{}\\Microsoft\\Windows\\Start Menu\\Programs\\Startup",
        std::env::var("APPDATA").unwrap_or_else(|_| "C:\\Users\\Default\\AppData\\Roaming".into())
    );

    let vbs_path = hotkey_dir.join("jcode-hotkey-launcher.vbs");
    let vbs_content = format!(
        "Set objShell = CreateObject(\"WScript.Shell\")\nobjShell.Run \"powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"\"{}\"\"\", 0, False\n",
        ps1_path.to_string_lossy()
    );
    std::fs::write(&vbs_path, &vbs_content)?;

    let create_startup_lnk = format!(
        r#"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut("{startup_dir}\jcode-hotkey.lnk")
$shortcut.TargetPath = "wscript.exe"
$shortcut.Arguments = '"{vbs_path}"'
$shortcut.Description = "jcode Alt+; hotkey listener"
$shortcut.WindowStyle = 7
$shortcut.Save()
Write-Output "OK"
"#,
        startup_dir = startup_dir,
        vbs_path = vbs_path.to_string_lossy(),
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &create_startup_lnk])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create startup shortcut: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains("OK") {
        anyhow::bail!("Startup shortcut creation did not confirm success");
    }

    let start_output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!(
                "Start-Process wscript.exe -ArgumentList '\"{}\"' -WindowStyle Hidden",
                vbs_path.to_string_lossy()
            ),
        ])
        .output();

    if let Err(e) = start_output {
        eprintln!(
            "  \x1b[33m⚠\x1b[0m  Could not start hotkey listener now: {}",
            e
        );
        eprintln!("    It will start automatically on next login.");
    }

    Ok(())
}

/// Install Alacritty via winget.
#[cfg(windows)]
fn install_alacritty() -> Result<()> {
    eprintln!("  Installing Alacritty via winget...");
    eprintln!("  (Windows may ask for permission to install)\n");

    let status = std::process::Command::new("winget")
        .args([
            "install",
            "-e",
            "--id",
            "Alacritty.Alacritty",
            "--accept-source-agreements",
        ])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("winget install failed (exit code: {:?})", status.code())
    }
}

/// Read a single-character choice from the user.
#[cfg(any(windows, target_os = "macos"))]
fn read_choice() -> String {
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
    input.trim().to_lowercase()
}

/// Show the hotkey setup nudge. Returns true if something was set up.
#[cfg(windows)]
fn nudge_hotkey(state: &mut SetupHintsState) -> bool {
    let terminal = detect_terminal();
    let using_alacritty = terminal == "alacritty" || is_alacritty_installed();

    let terminal_name = if using_alacritty {
        "Alacritty"
    } else {
        "Windows Terminal"
    };

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m \x1b[1m💡 Set up Alt+; to launch jcode from anywhere?\x1b[0m              \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Creates a global hotkey - no extra software needed.       \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Opens jcode in {:<39}    \x1b[36m│\x1b[0m",
        format!("{}.", terminal_name)
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Set up   \x1b[90m[n]\x1b[0m Not now   \x1b[90m[d]\x1b[0m Don't ask again        \x1b[36m│\x1b[0m"
    );
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            eprint!("\n");
            match create_hotkey_shortcut(using_alacritty) {
                Ok(()) => {
                    state.hotkey_configured = true;
                    let _ = state.save();
                    eprintln!(
                        "  \x1b[32m✓\x1b[0m Created hotkey (\x1b[1mAlt+;\x1b[0m) → {} + jcode",
                        terminal_name
                    );
                    eprintln!();
                    true
                }
                Err(e) => {
                    eprintln!("  \x1b[31m✗\x1b[0m Failed to create hotkey: {}", e);
                    eprintln!(
                        "    You can set it up manually later with: \x1b[1mjcode setup-hotkey\x1b[0m"
                    );
                    eprintln!();
                    false
                }
            }
        }
        "d" | "dont" => {
            state.hotkey_dismissed = true;
            let _ = state.save();
            false
        }
        _ => false,
    }
}

/// Show the Alacritty install nudge. Returns true if Alacritty was installed.
#[cfg(windows)]
fn nudge_alacritty(state: &mut SetupHintsState) -> bool {
    let terminal = detect_terminal();

    let current_terminal = match terminal {
        "windows-terminal" => "Windows Terminal",
        "wezterm" => "WezTerm",
        _ => "your current terminal",
    };

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m \x1b[1m💡 Alacritty: the fastest terminal for jcode\x1b[0m               \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    {:<55} \x1b[36m│\x1b[0m",
        format!("You're using {}.", current_terminal)
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Alacritty is GPU-accelerated with the lowest latency.    \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Install   \x1b[90m[n]\x1b[0m Not now   \x1b[90m[d]\x1b[0m Don't ask again       \x1b[36m│\x1b[0m"
    );
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            eprint!("\n");
            if !is_winget_available() {
                eprintln!("  \x1b[33m⚠\x1b[0m  winget not found. Install Alacritty manually:");
                eprintln!("     https://alacritty.org/");
                eprintln!();
                eprintln!("     Or install winget first: https://aka.ms/getwinget");
                eprintln!();
                return false;
            }

            match install_alacritty() {
                Ok(()) => {
                    state.alacritty_configured = true;
                    let _ = state.save();
                    eprintln!("  \x1b[32m✓\x1b[0m Alacritty installed!");

                    if state.hotkey_configured {
                        eprintln!("  Updating hotkey to use Alacritty...");
                        match create_hotkey_shortcut(true) {
                            Ok(()) => {
                                eprintln!(
                                    "  \x1b[32m✓\x1b[0m Hotkey updated: \x1b[1mAlt+;\x1b[0m → Alacritty + jcode"
                                );
                            }
                            Err(e) => {
                                eprintln!("  \x1b[33m⚠\x1b[0m  Could not update hotkey: {}", e);
                            }
                        }
                    }
                    eprintln!();
                    true
                }
                Err(e) => {
                    eprintln!("  \x1b[31m✗\x1b[0m Failed to install Alacritty: {}", e);
                    eprintln!("    Install manually: https://alacritty.org/");
                    eprintln!();
                    false
                }
            }
        }
        "d" | "dont" => {
            state.alacritty_dismissed = true;
            let _ = state.save();
            false
        }
        _ => false,
    }
}

/// Prompt the user to try out their new hotkey.
#[cfg(windows)]
fn prompt_try_it_out(installed_alacritty: bool) {
    eprintln!("\x1b[32m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[32m│\x1b[0m \x1b[1m✨ All set! Try it out:\x1b[0m                                     \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m                                                             \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m    Press \x1b[1mAlt+;\x1b[0m from anywhere to launch jcode.                \x1b[32m│\x1b[0m"
    );
    if installed_alacritty {
        eprintln!(
            "\x1b[32m│\x1b[0m    It will open in \x1b[1mAlacritty\x1b[0m for maximum performance.    \x1b[32m│\x1b[0m"
        );
    }
    eprintln!(
        "\x1b[32m│\x1b[0m                                                             \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m    \x1b[90m(Starting jcode normally in 3 seconds...)\x1b[0m                 \x1b[32m│\x1b[0m"
    );
    eprintln!("\x1b[32m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprintln!();

    std::thread::sleep(std::time::Duration::from_secs(3));
}

#[cfg(target_os = "macos")]
fn macos_guided_ghostty_message(current_terminal: MacTerminalKind) -> String {
    format!(
        "I want to upgrade my macOS terminal setup for jcode. Please guide me step-by-step, wait for confirmation between steps, and keep each step concise.\n\nCurrent terminal: {}\nGoal: install Ghostty and use it for jcode.\n\nPlease help me with:\n1) Detecting if Homebrew is installed (and installing it if missing)\n2) Installing Ghostty\n3) Launching Ghostty and setting it as my preferred terminal for jcode\n4) Optional: adding a macOS keyboard shortcut/launcher flow for jcode\n5) Verifying jcode runs in Ghostty and that inline images/graphics work\n\nAssume I am not an expert; provide exact commands and where to click in macOS settings when needed.",
        current_terminal.label()
    )
}

#[cfg(target_os = "macos")]
fn nudge_macos_ghostty(state: &mut SetupHintsState) -> Option<String> {
    let terminal = detect_macos_terminal();
    let using_ghostty = terminal == MacTerminalKind::Ghostty;
    let ghostty_installed = is_ghostty_installed();

    if using_ghostty {
        state.mac_ghostty_guided = true;
        state.mac_ghostty_dismissed = true;
        let _ = state.save();
        return None;
    }

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m \x1b[1m💡 Better macOS terminal for jcode: Ghostty\x1b[0m                \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Current terminal: {:<37} \x1b[36m│\x1b[0m",
        format!("{}.", terminal.label())
    );
    if ghostty_installed {
        eprintln!(
            "\x1b[36m│\x1b[0m    Ghostty is installed, but you are not using it now.      \x1b[36m│\x1b[0m"
        );
    } else {
        eprintln!(
            "\x1b[36m│\x1b[0m    Ghostty offers fast rendering and great jcode UX.         \x1b[36m│\x1b[0m"
        );
    }
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Let jcode guide you through setup right now?             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Yes      \x1b[90m[n]\x1b[0m Not now      \x1b[90m[d]\x1b[0m Don't ask again    \x1b[36m│\x1b[0m"
    );
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            state.mac_ghostty_guided = true;
            let _ = state.save();
            Some(macos_guided_ghostty_message(terminal))
        }
        "d" | "dont" => {
            state.mac_ghostty_dismissed = true;
            let _ = state.save();
            None
        }
        _ => None,
    }
}

/// Manual `jcode setup-hotkey` command.
///
/// Runs the full interactive setup flow regardless of launch count.
pub fn run_setup_hotkey(_listen_macos_hotkey: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if _listen_macos_hotkey {
            return run_macos_hotkey_listener();
        }

        let mut state = SetupHintsState::load();
        let terminal = effective_macos_terminal();
        eprintln!("\x1b[1mjcode setup-hotkey\x1b[0m");
        eprintln!();
        eprintln!("  Preferred terminal: {}", terminal.label());
        eprintln!("  Installing a LaunchAgent so Alt+; opens jcode from anywhere.");
        eprintln!();

        match install_macos_hotkey_listener(Some(terminal)) {
            Ok(installed_terminal) => {
                state.hotkey_configured = true;
                state.hotkey_dismissed = true;
                let _ = state.save();
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Created hotkey (\x1b[1mAlt+;\x1b[0m) → {} + jcode",
                    installed_terminal.label()
                );
                eprintln!();
                eprintln!(
                    "  Press \x1b[1mAlt+;\x1b[0m from anywhere to open jcode in {}.",
                    installed_terminal.label()
                );
                return Ok(());
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                anyhow::bail!("macOS hotkey setup failed: {}", e);
            }
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("Global hotkey setup is currently only supported on Windows.");
        eprintln!();
        eprintln!("On Linux/macOS, add a keybinding in your desktop environment:");
        eprintln!("  - niri: bindings in ~/.config/niri/config.kdl");
        eprintln!("  - GNOME: Settings > Keyboard > Custom Shortcuts");
        eprintln!("  - KDE: System Settings > Shortcuts > Custom Shortcuts");
        eprintln!("  - macOS: Shortcuts.app or System Settings > Keyboard > Shortcuts");
        return Ok(());
    }

    #[cfg(windows)]
    {
        let mut state = SetupHintsState::load();
        let terminal = detect_terminal();
        let already_using_alacritty = terminal == "alacritty";

        eprintln!("\x1b[1mjcode setup-hotkey\x1b[0m");
        eprintln!();

        eprintln!(
            "  Detected terminal: {}",
            match terminal {
                "windows-terminal" => "Windows Terminal",
                "wezterm" => "WezTerm",
                "alacritty" => "Alacritty",
                _ => "Unknown",
            }
        );

        if is_alacritty_installed() && !already_using_alacritty {
            eprintln!("  Alacritty: \x1b[32minstalled\x1b[0m");
        } else if already_using_alacritty {
            eprintln!("  Alacritty: \x1b[32mactive\x1b[0m");
        } else {
            eprintln!("  Alacritty: \x1b[90mnot installed\x1b[0m");
        }
        eprintln!();

        let mut installed_alacritty = false;
        if !already_using_alacritty && !is_alacritty_installed() {
            eprintln!(
                "  Alacritty is the fastest terminal emulator (GPU-accelerated, lowest latency)."
            );
            eprint!("  Install Alacritty? \x1b[32m[y]\x1b[0m/\x1b[90m[n]\x1b[0m: ");
            let _ = io::stderr().flush();
            let choice = read_choice();
            if choice == "y" || choice == "yes" {
                if !is_winget_available() {
                    eprintln!(
                        "\n  \x1b[33m⚠\x1b[0m  winget not found. Install Alacritty manually:"
                    );
                    eprintln!("     https://alacritty.org/\n");
                } else {
                    match install_alacritty() {
                        Ok(()) => {
                            state.alacritty_configured = true;
                            installed_alacritty = true;
                            eprintln!("  \x1b[32m✓\x1b[0m Alacritty installed!\n");
                        }
                        Err(e) => {
                            eprintln!("  \x1b[31m✗\x1b[0m Install failed: {}\n", e);
                        }
                    }
                }
            }
            eprintln!();
        }

        let use_alacritty = already_using_alacritty || is_alacritty_installed();
        let terminal_name = if use_alacritty {
            "Alacritty"
        } else {
            "Windows Terminal"
        };

        eprintln!(
            "  Setting up \x1b[1mAlt+;\x1b[0m → {} + jcode...",
            terminal_name
        );

        match create_hotkey_shortcut(use_alacritty) {
            Ok(()) => {
                state.hotkey_configured = true;
                let _ = state.save();
                eprintln!("  \x1b[32m✓\x1b[0m Created hotkey (\x1b[1mAlt+;\x1b[0m)");
                eprintln!();
                prompt_try_it_out(installed_alacritty);
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
            }
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn run_macos_hotkey_listener() -> Result<()> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::process::Command;

    let launch_script = mac_hotkey_support_dir()?.join("launch_jcode.sh");
    let manager =
        GlobalHotKeyManager::new().context("failed to initialize global hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::ALT), Code::Semicolon);
    manager
        .register(hotkey)
        .context("failed to register Alt+; hotkey")?;

    loop {
        if let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            if event.id == hotkey.id() && event.state == HotKeyState::Pressed {
                let _ = Command::new("sh").arg(&launch_script).spawn();
            }
        }
    }
}

/// Main entry point: check if we should show setup hints.
///
/// Called early in startup, before the TUI is initialized.
/// Returns optional structured startup hints for the TUI.
///
/// - Windows: On every 3rd launch, can show hotkey + Alacritty nudges.
/// - macOS: On every 3rd launch, can suggest Ghostty and optionally hand off
///   to AI-guided setup by returning a prebuilt prompt.
pub fn maybe_show_setup_hints() -> Option<StartupHints> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return StartupHints::none();
    }

    let mut state = SetupHintsState::load();
    state.launch_count += 1;
    let _ = state.save();

    if !state.desktop_shortcut_created {
        let _ = create_desktop_shortcut(&mut state);
    }

    let startup_hints = startup_hints_for_launch(&state);

    #[cfg(target_os = "macos")]
    {
        if state.launch_count % 3 != 0 {
            return startup_hints;
        }

        if !state.mac_ghostty_guided && !state.mac_ghostty_dismissed {
            let mut hints = startup_hints.unwrap_or_default();
            hints.auto_send_message = nudge_macos_ghostty(&mut state);
            return if hints.is_empty() { None } else { Some(hints) };
        }

        return startup_hints;
    }

    #[cfg(windows)]
    {
        if state.launch_count % 3 != 0 {
            return startup_hints;
        }

        let terminal = detect_terminal();
        let already_using_alacritty = terminal == "alacritty";

        if already_using_alacritty {
            state.alacritty_configured = true;
            state.alacritty_dismissed = true;
            let _ = state.save();
        }

        let mut did_setup_hotkey = false;
        let mut did_install_alacritty = false;

        if !state.hotkey_configured && !state.hotkey_dismissed {
            did_setup_hotkey = nudge_hotkey(&mut state);
        }

        if !state.alacritty_configured && !state.alacritty_dismissed && !already_using_alacritty {
            did_install_alacritty = nudge_alacritty(&mut state);
        }

        if did_setup_hotkey || (did_install_alacritty && state.hotkey_configured) {
            prompt_try_it_out(did_install_alacritty);
        }

        return startup_hints;
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        startup_hints
    }
}

#[cfg(target_os = "macos")]
fn macos_app_launcher_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join("Applications").join("Jcode.app"))
}

#[cfg(any(test, target_os = "macos"))]
fn macos_launcher_script(
    terminal: MacTerminalKind,
    exe_path: &str,
    app_dir: &std::path::Path,
) -> String {
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

#[cfg(target_os = "macos")]
fn install_macos_app_launcher() -> Result<(PathBuf, MacTerminalKind)> {
    let app_dir = macos_app_launcher_dir()?;
    let contents_dir = app_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    std::fs::create_dir_all(&macos_dir)?;

    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy().into_owned();
    let terminal = effective_macos_terminal();
    let launcher_path = macos_dir.join("jcode-launcher");
    let launcher_script = macos_launcher_script(terminal, &exe_path, &app_dir);
    std::fs::write(&launcher_path, launcher_script)?;

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
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
</dict>
</plist>
"#,
        version = env!("JCODE_VERSION")
    );
    std::fs::write(contents_dir.join("Info.plist"), info_plist)?;

    let _ = std::process::Command::new("touch").arg(&app_dir).status();
    save_preferred_macos_terminal(terminal)?;
    Ok((app_dir, terminal))
}

/// Manual `jcode setup-launcher` command.
pub fn run_setup_launcher() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut state = SetupHintsState::load();
        eprintln!("\x1b[1mjcode setup-launcher\x1b[0m");
        eprintln!();

        match install_macos_app_launcher() {
            Ok((app_dir, terminal)) => {
                state.desktop_shortcut_created = true;
                let _ = state.save();
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Installed launcher: {}",
                    app_dir.display()
                );
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Spotlight/Launchpad/Dock will launch jcode in {}",
                    terminal.label()
                );
                eprintln!();
                eprintln!("  Tip: pin Jcode.app to your Dock or launch it with Cmd+Space.");
                return Ok(());
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                anyhow::bail!("macOS launcher setup failed: {}", e);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Launcher setup is currently only supported on macOS.");
        Ok(())
    }
}

/// Create a desktop shortcut/launcher for jcode.
///
/// - Windows: creates a .lnk shortcut on the Desktop
/// - macOS: creates a jcode.app bundle in ~/Applications/
fn create_desktop_shortcut(state: &mut SetupHintsState) -> Result<()> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe()?;
        let exe_path = exe.to_string_lossy();

        let (target, args) = if is_alacritty_installed() {
            let alacritty = find_alacritty_path().unwrap_or_else(|| "alacritty".to_string());
            (alacritty, format!("-e \"{}\"", exe_path))
        } else {
            (exe_path.to_string(), String::new())
        };

        let desktop_dir =
            std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
        let shortcut_path = format!("{}\\Desktop\\jcode.lnk", desktop_dir);

        let ps_script = format!(
            r#"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut("{shortcut_path}")
$shortcut.TargetPath = "{target}"
$shortcut.Arguments = '{args}'
$shortcut.Description = "jcode - AI coding agent"
$shortcut.Save()
Write-Output "OK"
"#,
            shortcut_path = shortcut_path,
            target = target,
            args = args,
        );

        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_script])
            .output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("OK") {
                state.desktop_shortcut_created = true;
                let _ = state.save();
                crate::logging::info(&format!("Created desktop shortcut: {}", shortcut_path));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let (app_dir, _terminal) = install_macos_app_launcher()?;

        state.desktop_shortcut_created = true;
        let _ = state.save();

        crate::logging::info(&format!("Created macOS app bundle: {}", app_dir.display()));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        state.desktop_shortcut_created = true;
        let _ = state.save();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_shows_explicit_alignment_hint_first() {
        let state = SetupHintsState {
            launch_count: 1,
            ..SetupHintsState::default()
        };

        let hints = startup_hints_for_launch(&state).expect("expected startup hint");
        assert_eq!(
            hints.status_notice.as_deref(),
            Some("Tip: `/alignment left` or Alt+C toggles alignment.")
        );

        let (title, message) = hints.display_message.expect("expected display message");
        assert_eq!(title, "Alignment");
        assert!(message.contains("Alt+C"));
        assert!(message.contains("/alignment left"));
        assert!(message.contains("Claude-style left alignment"));
        assert!(!message.contains("display.centered = true"));
    }

    #[test]
    fn second_and_third_launches_include_alignment_tip() {
        let state = SetupHintsState {
            launch_count: 2,
            ..SetupHintsState::default()
        };

        let hints = startup_hints_for_launch(&state).expect("expected startup hint");
        assert_eq!(
            hints.status_notice.as_deref(),
            Some("Tip: Alt+C toggles left/center alignment.")
        );

        let (title, message) = hints.display_message.expect("expected display message");
        assert_eq!(title, "Welcome");
        assert!(message.contains("Alt+C"));
        assert!(message.contains("/alignment centered"));
        assert!(message.contains("/alignment left"));
        assert!(message.contains("display.centered = true"));
        assert!(message.contains("display.centered = false"));
    }

    #[test]
    fn launches_after_third_do_not_show_generic_alignment_tip() {
        let state = SetupHintsState {
            launch_count: 4,
            ..SetupHintsState::default()
        };

        assert!(startup_hints_for_launch(&state).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn first_three_launches_can_include_hotkey_notice_too() {
        let state = SetupHintsState {
            launch_count: 2,
            hotkey_configured: true,
            ..SetupHintsState::default()
        };

        let hints = startup_hints_for_launch(&state).expect("expected startup hint");
        let (_, message) = hints.display_message.expect("expected display message");
        assert!(message.contains("Alt+C"));
        assert!(message.contains("Alt+;"));
    }

    #[test]
    fn paused_jcode_shell_command_keeps_failures_visible() {
        let command = paused_jcode_shell_command("/tmp/jcode");
        assert!(command.contains("Press Enter to close"));
        assert!(command.contains("Jcode exited with status"));
        assert!(command.contains("jcode executable not found"));
    }

    #[test]
    fn macos_launcher_script_shows_alerts_and_uses_terminal_launcher() {
        let script = macos_launcher_script(
            MacTerminalKind::Ghostty,
            "/tmp/jcode",
            std::path::Path::new("/Users/test/Applications/Jcode.app"),
        );
        assert!(script.contains("display alert \"Jcode launch failed\""));
        assert!(script.contains("jcode setup-launcher"));
        assert!(script.contains("/usr/bin/open -na Ghostty"));
        assert!(script.contains("macos-launcher.log"));
    }
}
