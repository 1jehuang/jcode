//! Platform setup hints shown on startup.
//!
//! - Windows: suggest Alt+; hotkey setup and WezTerm install.
//! - macOS: detect suboptimal terminal and offer guided Ghostty setup via jcode.
//!
//! Each nudge can be dismissed permanently with "Don't ask again".
//! State is persisted in `~/.jcode/setup_hints.json`.

use crate::storage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupHintsState {
    pub launch_count: u64,
    pub hotkey_configured: bool,
    pub hotkey_dismissed: bool,
    pub wezterm_configured: bool,
    pub wezterm_dismissed: bool,
    pub mac_ghostty_guided: bool,
    pub mac_ghostty_dismissed: bool,
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
}

#[cfg(target_os = "macos")]
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

#[cfg(not(target_os = "macos"))]
fn detect_macos_terminal() -> MacTerminalKind {
    MacTerminalKind::Unknown
}

#[cfg(target_os = "macos")]
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

#[cfg(not(target_os = "macos"))]
fn is_ghostty_installed() -> bool {
    false
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

#[cfg(not(windows))]
fn detect_terminal() -> &'static str {
    "non-windows"
}

/// Check if WezTerm is installed (Windows).
#[cfg(windows)]
fn is_wezterm_installed() -> bool {
    std::process::Command::new("where")
        .arg("wezterm")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_wezterm_installed() -> bool {
    false
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

#[cfg(not(windows))]
fn is_winget_available() -> bool {
    false
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
fn create_hotkey_shortcut(use_wezterm: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy();

    let (launch_exe, launch_args) = if use_wezterm {
        ("wezterm".to_string(), format!("start -- \"{}\"", exe_path))
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

#[cfg(not(windows))]
fn create_hotkey_shortcut(_use_wezterm: bool) -> Result<()> {
    anyhow::bail!("Hotkey setup is only supported on Windows")
}

/// Install WezTerm via winget.
#[cfg(windows)]
fn install_wezterm() -> Result<()> {
    eprintln!("  Installing WezTerm via winget...");
    eprintln!("  (Windows may ask for permission to install)\n");

    let status = std::process::Command::new("winget")
        .args([
            "install",
            "-e",
            "--id",
            "wez.wezterm",
            "--accept-source-agreements",
        ])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("winget install failed (exit code: {:?})", status.code())
    }
}

#[cfg(not(windows))]
fn install_wezterm() -> Result<()> {
    anyhow::bail!("WezTerm install is only supported on Windows via winget")
}

/// Read a single-character choice from the user.
fn read_choice() -> String {
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
    input.trim().to_lowercase()
}

/// Show the hotkey setup nudge. Returns true if something was set up.
fn nudge_hotkey(state: &mut SetupHintsState) -> bool {
    let terminal = detect_terminal();
    let using_wezterm = terminal == "wezterm" || is_wezterm_installed();

    let terminal_name = if using_wezterm {
        "WezTerm"
    } else {
        "Windows Terminal"
    };

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m \x1b[1m💡 Set up Alt+; to launch jcode from anywhere?\x1b[0m              \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m    Creates a global hotkey - no extra software needed.       \x1b[36m│\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m    Opens jcode in {:<39}    \x1b[36m│\x1b[0m",
        format!("{}.", terminal_name)
    );
    eprintln!("\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Set up   \x1b[90m[n]\x1b[0m Not now   \x1b[90m[d]\x1b[0m Don't ask again        \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            eprint!("\n");
            match create_hotkey_shortcut(using_wezterm) {
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
                    eprintln!("    You can set it up manually later with: \x1b[1mjcode setup-hotkey\x1b[0m");
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

/// Show the WezTerm install nudge. Returns true if WezTerm was installed.
fn nudge_wezterm(state: &mut SetupHintsState) -> bool {
    let terminal = detect_terminal();

    let current_terminal = match terminal {
        "windows-terminal" => "Windows Terminal",
        "alacritty" => "Alacritty",
        _ => "your current terminal",
    };

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m \x1b[1m💡 WezTerm gives jcode superpowers\x1b[0m                         \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m    {} can't render inline images.          \x1b[36m│\x1b[0m",
        format!("{:<30}", current_terminal)
    );
    eprintln!("\x1b[36m│\x1b[0m    WezTerm supports graphics, diagrams, and more.           \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Install   \x1b[90m[n]\x1b[0m Not now   \x1b[90m[d]\x1b[0m Don't ask again       \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            eprint!("\n");
            if !is_winget_available() {
                eprintln!("  \x1b[33m⚠\x1b[0m  winget not found. Install WezTerm manually:");
                eprintln!("     https://wezfurlong.org/wezterm/install/windows.html");
                eprintln!();
                eprintln!("     Or install winget first: https://aka.ms/getwinget");
                eprintln!();
                return false;
            }

            match install_wezterm() {
                Ok(()) => {
                    state.wezterm_configured = true;
                    let _ = state.save();
                    eprintln!("  \x1b[32m✓\x1b[0m WezTerm installed!");

                    if state.hotkey_configured {
                        eprintln!("  Updating hotkey to use WezTerm...");
                        match create_hotkey_shortcut(true) {
                            Ok(()) => {
                                eprintln!("  \x1b[32m✓\x1b[0m Hotkey updated: \x1b[1mAlt+;\x1b[0m → WezTerm + jcode");
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
                    eprintln!("  \x1b[31m✗\x1b[0m Failed to install WezTerm: {}", e);
                    eprintln!(
                        "    Install manually: https://wezfurlong.org/wezterm/install/windows.html"
                    );
                    eprintln!();
                    false
                }
            }
        }
        "d" | "dont" => {
            state.wezterm_dismissed = true;
            let _ = state.save();
            false
        }
        _ => false,
    }
}

/// Prompt the user to try out their new hotkey.
fn prompt_try_it_out(installed_wezterm: bool) {
    eprintln!("\x1b[32m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!("\x1b[32m│\x1b[0m \x1b[1m✨ All set! Try it out:\x1b[0m                                     \x1b[32m│\x1b[0m");
    eprintln!("\x1b[32m│\x1b[0m                                                             \x1b[32m│\x1b[0m");
    eprintln!("\x1b[32m│\x1b[0m    Press \x1b[1mAlt+;\x1b[0m from anywhere to launch jcode.                \x1b[32m│\x1b[0m");
    if installed_wezterm {
        eprintln!("\x1b[32m│\x1b[0m    It will open in \x1b[1mWezTerm\x1b[0m with full graphics support.      \x1b[32m│\x1b[0m");
    }
    eprintln!("\x1b[32m│\x1b[0m                                                             \x1b[32m│\x1b[0m");
    eprintln!("\x1b[32m│\x1b[0m    \x1b[90m(Starting jcode normally in 3 seconds...)\x1b[0m                 \x1b[32m│\x1b[0m");
    eprintln!("\x1b[32m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprintln!();

    std::thread::sleep(std::time::Duration::from_secs(3));
}

fn macos_guided_ghostty_message(current_terminal: MacTerminalKind) -> String {
    format!(
        "I want to upgrade my macOS terminal setup for jcode. Please guide me step-by-step, wait for confirmation between steps, and keep each step concise.\n\nCurrent terminal: {}\nGoal: install Ghostty and use it for jcode.\n\nPlease help me with:\n1) Detecting if Homebrew is installed (and installing it if missing)\n2) Installing Ghostty\n3) Launching Ghostty and setting it as my preferred terminal for jcode\n4) Optional: adding a macOS keyboard shortcut/launcher flow for jcode\n5) Verifying jcode runs in Ghostty and that inline images/graphics work\n\nAssume I am not an expert; provide exact commands and where to click in macOS settings when needed.",
        current_terminal.label()
    )
}

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
    eprintln!("\x1b[36m│\x1b[0m \x1b[1m💡 Better macOS terminal for jcode: Ghostty\x1b[0m                \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m    Current terminal: {:<37} \x1b[36m│\x1b[0m",
        format!("{}.", terminal.label())
    );
    if ghostty_installed {
        eprintln!("\x1b[36m│\x1b[0m    Ghostty is installed, but you are not using it now.      \x1b[36m│\x1b[0m");
    } else {
        eprintln!("\x1b[36m│\x1b[0m    Ghostty offers fast rendering and great jcode UX.         \x1b[36m│\x1b[0m");
    }
    eprintln!("\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m    Let jcode guide you through setup right now?             \x1b[36m│\x1b[0m");
    eprintln!("\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Yes      \x1b[90m[n]\x1b[0m Not now      \x1b[90m[d]\x1b[0m Don't ask again    \x1b[36m│\x1b[0m");
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
pub fn run_setup_hotkey() -> Result<()> {
    if !cfg!(windows) {
        eprintln!("Global hotkey setup is currently only supported on Windows.");
        eprintln!();
        eprintln!("On Linux/macOS, add a keybinding in your desktop environment:");
        eprintln!("  - niri: bindings in ~/.config/niri/config.kdl");
        eprintln!("  - GNOME: Settings > Keyboard > Custom Shortcuts");
        eprintln!("  - KDE: System Settings > Shortcuts > Custom Shortcuts");
        eprintln!("  - macOS: Shortcuts.app or System Settings > Keyboard > Shortcuts");
        return Ok(());
    }

    let mut state = SetupHintsState::load();
    let terminal = detect_terminal();
    let already_using_wezterm = terminal == "wezterm";

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

    if is_wezterm_installed() && !already_using_wezterm {
        eprintln!("  WezTerm: \x1b[32minstalled\x1b[0m");
    } else if already_using_wezterm {
        eprintln!("  WezTerm: \x1b[32mactive\x1b[0m");
    } else {
        eprintln!("  WezTerm: \x1b[90mnot installed\x1b[0m");
    }
    eprintln!();

    // Step 1: WezTerm
    let mut installed_wezterm = false;
    if !already_using_wezterm && !is_wezterm_installed() {
        eprintln!("  WezTerm provides the best jcode experience (inline images, graphics).");
        eprint!("  Install WezTerm? \x1b[32m[y]\x1b[0m/\x1b[90m[n]\x1b[0m: ");
        let _ = io::stderr().flush();
        let choice = read_choice();
        if choice == "y" || choice == "yes" {
            if !is_winget_available() {
                eprintln!("\n  \x1b[33m⚠\x1b[0m  winget not found. Install WezTerm manually:");
                eprintln!("     https://wezfurlong.org/wezterm/install/windows.html\n");
            } else {
                match install_wezterm() {
                    Ok(()) => {
                        state.wezterm_configured = true;
                        installed_wezterm = true;
                        eprintln!("  \x1b[32m✓\x1b[0m WezTerm installed!\n");
                    }
                    Err(e) => {
                        eprintln!("  \x1b[31m✗\x1b[0m Install failed: {}\n", e);
                    }
                }
            }
        }
        eprintln!();
    }

    // Step 2: Hotkey
    let use_wezterm = already_using_wezterm || is_wezterm_installed();
    let terminal_name = if use_wezterm {
        "WezTerm"
    } else {
        "Windows Terminal"
    };

    eprintln!(
        "  Setting up \x1b[1mAlt+;\x1b[0m → {} + jcode...",
        terminal_name
    );

    match create_hotkey_shortcut(use_wezterm) {
        Ok(()) => {
            state.hotkey_configured = true;
            let _ = state.save();
            eprintln!("  \x1b[32m✓\x1b[0m Created hotkey (\x1b[1mAlt+;\x1b[0m)");
            eprintln!();
            prompt_try_it_out(installed_wezterm);
        }
        Err(e) => {
            eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
        }
    }

    Ok(())
}

/// Main entry point: check if we should show setup hints.
///
/// Called early in startup, before the TUI is initialized.
/// Returns an optional startup user message to auto-send in jcode.
///
/// - Windows: On every 3rd launch, can show hotkey + WezTerm nudges.
/// - macOS: On every 3rd launch, can suggest Ghostty and optionally hand off
///   to AI-guided setup by returning a prebuilt prompt.
pub fn maybe_show_setup_hints() -> Option<String> {
    if !cfg!(windows) && !cfg!(target_os = "macos") {
        return None;
    }

    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return None;
    }

    let mut state = SetupHintsState::load();
    state.launch_count += 1;
    let _ = state.save();

    if state.launch_count % 3 != 0 {
        return None;
    }

    if cfg!(target_os = "macos") {
        if !state.mac_ghostty_guided && !state.mac_ghostty_dismissed {
            return nudge_macos_ghostty(&mut state);
        }
        return None;
    }

    let terminal = detect_terminal();
    let already_using_wezterm = terminal == "wezterm";

    if already_using_wezterm {
        state.wezterm_configured = true;
        state.wezterm_dismissed = true;
        let _ = state.save();
    }

    let mut did_setup_hotkey = false;
    let mut did_install_wezterm = false;

    // Show hotkey nudge first (if still relevant).
    if !state.hotkey_configured && !state.hotkey_dismissed {
        did_setup_hotkey = nudge_hotkey(&mut state);
    }

    // Then show WezTerm nudge in the same launch (if still relevant).
    if !state.wezterm_configured && !state.wezterm_dismissed && !already_using_wezterm {
        did_install_wezterm = nudge_wezterm(&mut state);
    }

    // End-of-setup nudge to validate Alt+; if we created or updated the hotkey path.
    if did_setup_hotkey || (did_install_wezterm && state.hotkey_configured) {
        prompt_try_it_out(did_install_wezterm);
    }

    None
}
