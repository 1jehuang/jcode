//! Read-only cloud diagnostics. This module deliberately bypasses Target's SSH
//! helpers: managed ssh_args() writes keys/config and its proxy wakes machines.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_PROBLEMS: usize = 100;
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 32 * 1024;

#[derive(Deserialize)]
struct LoggedProblem {
    ts: String,
    version: String,
    command: String,
    #[serde(rename = "session_id")]
    _session_id: String,
    exit_code: Option<i32>,
    stderr: String,
}

#[derive(Debug, Serialize)]
struct Problem {
    ts: String,
    version: String,
    command: String,
    session_id: &'static str,
    exit_code: Option<i32>,
    stderr: &'static str,
}

#[derive(Default, Serialize)]
struct Problems {
    problems: Vec<Problem>,
    malformed_lines: usize,
    truncated: bool,
    log_status: &'static str,
    // Free-form logs can contain arbitrary credentials, not only recognized key
    // prefixes. Never echo them, even when they appear in metadata fields.
    redaction: &'static str,
}

fn failure_kind(stderr: &str) -> &'static str {
    let text = stderr.to_ascii_lowercase();
    if text.contains("permission denied") || text.contains("unauthorized") {
        "authentication failed (details withheld)"
    } else if text.contains("timed out") || text.contains("timeout") {
        "connection timed out (details withheld)"
    } else if text.contains("connection refused") || text.contains("could not resolve") {
        "host unreachable (details withheld)"
    } else if text.contains("unrecognized") || text.contains("too old") {
        "remote protocol mismatch (details withheld)"
    } else {
        "cloud operation failed (details withheld)"
    }
}

fn safe_problem(raw: LoggedProblem) -> Problem {
    let ts = chrono::DateTime::parse_from_rfc3339(&raw.ts)
        .map(|ts| ts.to_rfc3339())
        .unwrap_or_else(|_| "unknown".into());
    let version = if raw.version.len() <= 40
        && raw.version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        raw.version
    } else {
        "withheld".into()
    };
    let command = match raw.command.split_whitespace().next().unwrap_or("") {
        "/cloud" => "/cloud".into(),
        "/local" => "/local".into(),
        "/mobile" => "/mobile".into(),
        command @ ("move" | "return" | "status" | "where" | "attach" | "prepare" | "doctor"
        | "problems" | "start" | "stop") => command.to_string(),
        _ => "cloud operation".into(),
    };
    Problem {
        ts,
        version,
        command,
        session_id: "[withheld]",
        exit_code: raw.exit_code,
        stderr: failure_kind(&raw.stderr),
    }
}

fn read_problems(path: &Path, limit: usize) -> Problems {
    let mut report = Problems {
        log_status: "missing",
        redaction: "Session identifiers and free-form diagnostic text are withheld to protect credentials.",
        ..Problems::default()
    };
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                report.log_status = "unreadable";
            }
            return report;
        }
    };
    let result = (|| -> std::io::Result<Vec<u8>> {
        let length = file.metadata()?.len();
        let start = length.saturating_sub(MAX_LOG_BYTES);
        report.truncated = start > 0;
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
        if start > 0 {
            // The first record may begin before the bounded tail.
            bytes = bytes
                .splitn(2, |b| *b == b'\n')
                .nth(1)
                .unwrap_or(&[])
                .to_vec();
        }
        Ok(bytes)
    })();
    let Ok(bytes) = result else {
        report.log_status = "unreadable";
        return report;
    };
    report.log_status = "read";
    let limit = limit.min(MAX_PROBLEMS);
    let mut latest = VecDeque::with_capacity(limit);
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let record = if line.len() > MAX_LINE_BYTES {
            None
        } else {
            match serde_json::from_slice::<LoggedProblem>(line) {
                Ok(record) => Some(record),
                Err(_) => {
                    report.malformed_lines += 1;
                    continue;
                }
            }
        };
        match record {
            Some(record) if limit > 0 => {
                if latest.len() == limit {
                    latest.pop_front();
                }
                latest.push_back(safe_problem(record));
            }
            Some(_) => (),
            None => report.malformed_lines += 1,
        }
    }
    report.problems = latest.into_iter().rev().collect();
    report
}

pub(super) fn problems(limit: usize, json: bool) -> Result<()> {
    let report = read_problems(
        &crate::storage::jcode_dir()?.join("logs/cloud-problems.jsonl"),
        limit,
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Cloud problems: {} (log: {}, malformed: {}, bounded tail: {})",
            report.problems.len(),
            report.log_status,
            report.malformed_lines,
            report.truncated
        );
        for problem in &report.problems {
            println!(
                "{}  {}  exit {:?}  {}",
                problem.ts, problem.command, problem.exit_code, problem.stderr
            );
        }
        println!("{}", report.redaction);
    }
    Ok(())
}

#[derive(Serialize)]
struct Doctor {
    read_only: bool,
    account: &'static str,
    machine_state: String,
    machine_ready: Option<bool>,
    ssh: &'static str,
    remote_version: Option<String>,
    moved_sessions: usize,
    local_sessions_with_migration_history: usize,
    unreadable_leases: usize,
    recent_failures: Problems,
    notes: Vec<&'static str>,
}

fn lease_counts(root: &Path) -> (usize, usize, usize) {
    let Ok(entries) = std::fs::read_dir(root.join("session_leases")) else {
        return (0, 0, usize::from(root.join("session_leases").exists()));
    };
    let (mut away, mut here, mut bad) = (0, 0, 0);
    for entry in entries {
        let Ok(entry) = entry else {
            bad += 1;
            continue;
        };
        if entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let lease = (|| -> anyhow::Result<crate::storage::SessionLease> {
            let file = std::fs::File::open(entry.path())?;
            let mut bytes = Vec::new();
            file.take(64 * 1024).read_to_end(&mut bytes)?;
            Ok(serde_json::from_slice(&bytes)?)
        })();
        match lease {
            Ok(lease) if lease.away_host.is_some() => away += 1,
            Ok(_) => here += 1,
            Err(_) => bad += 1,
        }
    }
    (away, here, bad)
}

// No Target::run_remote, ssh_command, master_is_live, or custom transport here.
// All can execute provisioning or user-supplied side effects. Disable proxy
// execution on ordinary SSH aliases too, since an alias may point at managed SSH.
fn stop_probe(child: &mut std::process::Child) {
    if let Err(error) = child.kill() {
        crate::logging::info(&format!("Could not stop cloud version probe: {error}"));
    }
    if let Err(error) = child.wait() {
        crate::logging::info(&format!("Could not reap cloud version probe: {error}"));
    }
}

fn version_probe(host: &str, binary: &str) -> (&'static str, Option<String>) {
    if host.starts_with('-') || host.trim().is_empty() {
        return ("invalid target", None);
    }
    let mut command = Command::new("ssh");
    command.args([
        "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5",
        "-o", "ConnectionAttempts=1", "-o", "ProxyCommand=none",
        "-o", "ProxyJump=none", "-o", "ControlMaster=no",
        "-o", "ControlPath=none", "-o", "StrictHostKeyChecking=yes",
        "-o", "UpdateHostKeys=no", "-o", "PermitLocalCommand=no",
        "-o", "ClearAllForwardings=yes", "--",
    ]).arg(host).arg(format!(
        "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {} --no-update --no-selfdev --version",
        super::shell_quote(binary)
    )).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return ("SSH unavailable", None);
    };
    let Some(stdout) = child.stdout.take() else {
        stop_probe(&mut child);
        return ("probe failed", None);
    };
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(4097).read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                stop_probe(&mut child);
                break None;
            }
        }
    };
    let bytes = match reader.join() {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => return ("probe output unreadable", None),
        Err(_) => return ("probe reader failed", None),
    };
    match status {
        Some(status) if status.success() => ("reachable", safe_version(&bytes)),
        Some(status) if status.code() == Some(255) => {
            ("unreachable or authentication failed", None)
        }
        Some(_) => ("remote version command failed", None),
        None => ("probe timed out or failed", None),
    }
}

fn safe_version(bytes: &[u8]) -> Option<String> {
    // Only emit the numeric release, never arbitrary SSH/banner/program output.
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.trim(),
        Err(_) => {
            crate::logging::info("Cloud version probe returned non-UTF-8 output (withheld)");
            return None;
        }
    };
    let version = text.strip_prefix("jcode ")?.split_whitespace().next()?;
    let release = version.trim_start_matches('v').split('-').next()?;
    (release.len() <= 40
        && release.split('.').count() == 3
        && release
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
    .then(|| format!("jcode {release}"))
}

pub(super) fn doctor(target: &super::Target, json: bool) -> Result<()> {
    let root = crate::storage::jcode_dir()?;
    let signed_in = super::super::cloud_managed::account_available();
    let (moved, local, bad) = lease_counts(&root);
    let mut report = Doctor {
        read_only: true,
        account: if signed_in {
            "configured (not independently verified)"
        } else {
            "not configured"
        },
        machine_state: "unavailable".into(),
        machine_ready: None,
        ssh: "not checked",
        remote_version: None,
        moved_sessions: moved,
        local_sessions_with_migration_history: local,
        unreadable_leases: bad,
        recent_failures: read_problems(&root.join("logs/cloud-problems.jsonl"), 10),
        notes: vec![],
    };
    if signed_in {
        match super::super::cloud_managed::status() {
            Ok(reply) => {
                report.machine_state = match reply.state.as_str() {
                    "running" | "ready" | "stopped" | "stopping" | "starting" | "pending"
                    | "sleeping" | "provisioning" | "absent" | "none" | "not_created" => {
                        reply.state
                    }
                    _ => "unknown".into(),
                };
                report.machine_ready = Some(reply.ready);
            }
            Err(_) => report
                .notes
                .push("Read-only machine status request failed. Error details withheld."),
        }
    }
    match target.resolve_host() {
        Ok(host) if super::super::cloud_managed::is_managed(&host) => {
            report.ssh = "skipped (managed proxy can wake or provision)";
            report.notes.push("Managed SSH and remote version were not probed. Doctor never wakes machines or creates SSH credentials/config.");
        }
        Ok(_) if target.transport.is_some() => {
            report.ssh = "skipped (custom transport cannot be guaranteed read-only)";
        }
        Ok(host) => {
            (report.ssh, report.remote_version) = version_probe(&host, &target.remote_binary());
            report.notes.push("SSH probes disable proxies, forwarding, multiplexing, and host-key updates. Proxy-only aliases require a separate connectivity check.");
        }
        Err(_) => report
            .notes
            .push("No target configured. Pass --host or sign in to a Jcode account."),
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Cloud doctor (read-only)\nAccount: {}\nAccount machine: {}\nSSH: {}\nRemote version: {}\nMoved sessions: {}\nLocal migration history: {}\nUnreadable leases: {}\nRecent failures: {} (log: {})",
            report.account,
            report.machine_state,
            report.ssh,
            report.remote_version.as_deref().unwrap_or("not available"),
            report.moved_sessions,
            report.local_sessions_with_migration_history,
            report.unreadable_leases,
            report.recent_failures.problems.len(),
            report.recent_failures.log_status
        );
        for problem in &report.recent_failures.problems {
            println!(
                "Recent failure: {}  {}  exit {:?}  {}",
                problem.ts, problem.command, problem.exit_code, problem.stderr
            );
        }
        for note in report.notes {
            println!("Note: {note}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(exit: i32) -> String {
        serde_json::json!({
            "ts": "2026-11-06T10:34:29Z", "version": "0.99.714",
            "command": "/cloud --host secret-host", "session_id": "session_secret", "exit_code": exit,
            "stderr": "Bearer secret-token permission denied"
        })
        .to_string()
    }

    #[test]
    fn reads_real_tui_schema_newest_first_and_skips_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("problems.jsonl");
        std::fs::write(&path, format!("{}\ninvalid\n{}\n", line(1), line(2))).unwrap();
        let report = read_problems(&path, 1);
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.problems[0].exit_code, Some(2));
        assert_eq!(report.problems[0].command, "/cloud");
        assert_eq!(report.malformed_lines, 1);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("secret-token"));
        assert!(!json.contains("session_secret"));
        assert!(!json.contains("secret-host"));
        assert!(json.contains("authentication failed"));
    }

    #[test]
    fn missing_zero_and_extreme_limits_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        assert_eq!(read_problems(&path, 10).log_status, "missing");
        std::fs::write(&path, (0..110).map(line).collect::<Vec<_>>().join("\n")).unwrap();
        assert!(read_problems(&path, 0).problems.is_empty());
        assert_eq!(
            read_problems(&path, usize::MAX).problems.len(),
            MAX_PROBLEMS
        );
    }

    #[test]
    fn oversized_and_partial_lines_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n{{",
                "x".repeat(MAX_LOG_BYTES as usize + 100),
                line(7)
            ),
        )
        .unwrap();
        let report = read_problems(&path, 5);
        assert!(report.truncated);
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.malformed_lines, 1);
    }

    #[test]
    fn arbitrary_metadata_and_remote_output_are_not_echoed() {
        let mut raw: LoggedProblem = serde_json::from_str(&line(1)).unwrap();
        raw.ts = "secret-token".into();
        raw.version = "secret-token".into();
        raw.command = "move --key secret-token".into();
        let json = serde_json::to_string(&safe_problem(raw)).unwrap();
        assert!(!json.contains("secret-token"));
        assert_eq!(
            safe_version(b"jcode v0.99.714-dev (secret-token)"),
            Some("jcode 0.99.714".into())
        );
        assert!(safe_version(b"Bearer secret-token").is_none());
        assert!(safe_version(b"jcode \xff").is_none());
    }

    #[test]
    fn lease_summary_is_local_only_and_does_not_create_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(lease_counts(dir.path()), (0, 0, 0));
        assert!(!dir.path().join("session_leases").exists());
        std::fs::create_dir(dir.path().join("session_leases")).unwrap();
        for (id, host) in [("away", Some("remote")), ("here", None)] {
            let lease = serde_json::json!({ "session_id": id, "epoch": 1, "away_host": host, "updated_at": "now" });
            std::fs::write(
                dir.path().join(format!("session_leases/{id}.json")),
                lease.to_string(),
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("session_leases/bad.json"), "invalid").unwrap();
        assert_eq!(lease_counts(dir.path()), (1, 1, 1));
    }
}
