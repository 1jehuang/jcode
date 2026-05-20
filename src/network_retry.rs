use std::time::Duration;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkWaitPlan {
    pub reason: String,
    pub listener_summary: String,
}

pub fn classify_network_interruption(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = Some(error);
    while let Some(err) = current {
        let text = err.to_string().to_ascii_lowercase();
        parts.push(text);
        current = err.source();
    }
    classify_text(&parts.join(" | "))
}

pub fn classify_message(message: &str) -> Option<String> {
    classify_text(&message.to_ascii_lowercase())
}

fn classify_text(text: &str) -> Option<String> {
    let network_markers = [
        "connection reset",
        "connection aborted",
        "connection refused",
        "broken pipe",
        "network is unreachable",
        "network unreachable",
        "host is down",
        "no route to host",
        "not connected",
        "dns error",
        "failed to lookup address",
        "temporary failure in name resolution",
        "name or service not known",
        "operation timed out",
        "timed out",
        "timeout",
        "error trying to connect",
        "connection closed before message completed",
        "unexpected eof",
        "end of file before message completed",
    ];
    if network_markers.iter().any(|marker| text.contains(marker)) {
        return Some("the network connection appears to have dropped".to_string());
    }
    None
}

pub fn wait_plan() -> NetworkWaitPlan {
    #[cfg(target_os = "linux")]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary:
                "listening for Linux netlink changes via `ip monitor`; also verifying with reconnect probes"
                    .to_string(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        return NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary:
                "listening for macOS route/interface changes via `route -n monitor`; also verifying with reconnect probes"
                    .to_string(),
        };
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        NetworkWaitPlan {
            reason: "stream interrupted by a likely network disconnect".to_string(),
            listener_summary: "waiting with lightweight reconnect probes".to_string(),
        }
    }
}

pub async fn wait_until_probably_online() {
    let mut delay = Duration::from_secs(1);
    loop {
        if probe_connectivity().await {
            return;
        }
        wait_for_platform_change_or_delay(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

pub async fn is_probably_online() -> bool {
    probe_connectivity().await
}

async fn probe_connectivity() -> bool {
    let client = crate::provider::shared_http_client();
    let request = client
        .head("https://www.gstatic.com/generate_204")
        .timeout(Duration::from_secs(5));
    matches!(request.send().await, Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 204)
}

async fn wait_for_platform_change_or_delay(delay: Duration) {
    #[cfg(target_os = "linux")]
    {
        if command_exists("ip").await {
            let fut = wait_for_command_output("ip", &["monitor", "link", "address", "route"]);
            let _ = timeout(delay, fut).await;
            return;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if command_exists("route").await {
            let fut = wait_for_command_output("route", &["-n", "monitor"]);
            let _ = timeout(delay, fut).await;
            return;
        }
    }
    sleep(delay).await;
}

async fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "command -v {} >/dev/null 2>&1",
            shell_escape(command)
        ))
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

fn shell_escape(value: &str) -> String {
    value.replace('\'', "'\\''")
}

async fn wait_for_command_output(command: &str, args: &[&str]) {
    let mut command_builder = Command::new(command);
    command_builder
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match command_builder.spawn() {
        Ok(child) => child,
        Err(_) => return,
    };
    if let Some(mut stdout) = child.stdout.take() {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 1];
        let _ = stdout.read(&mut buf).await;
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_network_errors() {
        assert!(classify_message("connection reset by peer").is_some());
        assert!(classify_message("temporary failure in name resolution").is_some());
        assert!(classify_message("network is unreachable").is_some());
        assert!(classify_message("401 unauthorized").is_none());
    }
}
