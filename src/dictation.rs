use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::process::{Command, Stdio};
use tokio::time::{Duration, timeout};

#[derive(Debug, Clone)]
pub struct DictationRun {
    pub text: String,
    pub mode: crate::protocol::TranscriptMode,
}

pub async fn run_configured() -> Result<DictationRun> {
    let cfg = crate::config::config().dictation.clone();
    let command = cfg.command.trim();
    if command.is_empty() {
        anyhow::bail!(
            "Dictation is not configured. Set `[dictation].command` in `~/.jcode/config.toml`."
        );
    }

    let text = run_command(command, cfg.timeout_secs).await?;
    Ok(DictationRun {
        text,
        mode: cfg.mode,
    })
}

pub async fn run_command(command: &str, timeout_secs: u64) -> Result<String> {
    let mut child = shell_command(command);
    child.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = child
        .spawn()
        .with_context(|| format!("failed to start `{}`", command))?;

    let output = if timeout_secs == 0 {
        child
            .wait_with_output()
            .await
            .context("failed to wait for dictation command")?
    } else {
        timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
            .with_context(|| format!("dictation command timed out after {}s", timeout_secs))?
            .context("failed to wait for dictation command")?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            anyhow::bail!("dictation command exited with {}", output.status);
        }
        anyhow::bail!(stderr);
    }

    let transcript = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .trim()
        .to_string();
    if transcript.is_empty() {
        anyhow::bail!("dictation command returned an empty transcript");
    }

    Ok(transcript)
}

pub fn type_text(text: &str) -> Result<()> {
    let status = Command::new("wtype")
        .arg("--")
        .arg(text)
        .status()
        .context("failed to launch `wtype`")?;
    if !status.success() {
        anyhow::bail!("`wtype` exited with {}", status);
    }
    Ok(())
}

pub fn focused_jcode_session() -> Result<Option<String>> {
    let Some(window) = focused_window_niri()? else {
        return Ok(None);
    };
    Ok(resolve_session_for_window_pid(window.pid))
}

#[derive(Debug, Deserialize)]
struct NiriFocusedWindow {
    pid: u32,
    #[allow(dead_code)]
    title: Option<String>,
    #[allow(dead_code)]
    app_id: Option<String>,
}

fn focused_window_niri() -> Result<Option<NiriFocusedWindow>> {
    let output = Command::new("niri")
        .args(["msg", "-j", "focused-window"])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(None);
    }

    let window: NiriFocusedWindow =
        serde_json::from_str(trimmed).context("failed to parse `niri msg -j focused-window`")?;
    Ok(Some(window))
}

fn resolve_session_for_window_pid(window_pid: u32) -> Option<String> {
    let children = proc_children_map().ok()?;
    let mut queue = VecDeque::from([window_pid]);

    while let Some(pid) = queue.pop_front() {
        if let Some(session_id) = crate::session::find_active_session_id_by_pid(pid) {
            return Some(session_id);
        }
        if let Some(next) = children.get(&pid) {
            queue.extend(next.iter().copied());
        }
    }

    None
}

fn proc_children_map() -> Result<HashMap<u32, Vec<u32>>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let proc_dir = std::fs::read_dir("/proc").context("failed to read /proc")?;

    for entry in proc_dir {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(pid) = file_name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };

        let status_path = entry.path().join("status");
        let Ok(status) = std::fs::read_to_string(status_path) else {
            continue;
        };
        let Some(ppid) = parse_ppid(&status) else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
    }

    Ok(children)
}

fn parse_ppid(status: &str) -> Option<u32> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("PPid:")?;
        value.trim().parse::<u32>().ok()
    })
}

fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc").arg(command);
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_ppid, run_command};

    #[test]
    fn parse_ppid_from_proc_status() {
        let status = "Name:\tbash\nState:\tS (sleeping)\nPPid:\t1234\n";
        assert_eq!(parse_ppid(status), Some(1234));
    }

    #[tokio::test]
    async fn run_command_trims_trailing_newlines() {
        let text = run_command("printf 'hello from test\\n'", 5)
            .await
            .expect("dictation command should succeed");
        assert_eq!(text, "hello from test");
    }
}
