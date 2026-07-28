//! Live memory readout for the top chrome row.
//!
//! Shows how much RAM this window (the client) and the jcode daemon (the
//! server hosting the session) are resident with, so a leak in either is
//! visible at a glance instead of needing `ps`. All decisions are pure
//! functions of plain inputs ([`vm_rss_bytes`], [`is_daemon_cmdline`],
//! [`format_bytes`]) with thin `/proc` wrappers around them, so the readout is
//! testable without a daemon or a window.
//!
//! RSS rather than anything fancier: it is the number `ps` and the user's own
//! intuition already agree on. The daemon serves every session out of one
//! process, so its figure is the whole server, not one session's slice.

use std::time::{Duration, Instant};

/// How often the readout refreshes. Reading two `/proc` files is microseconds,
/// but the figure only changes meaningfully over seconds, and a caption that
/// flickers per-frame reads as noise rather than as a fact.
const REFRESH: Duration = Duration::from_secs(2);

/// One sampled readout, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readout {
    /// This process's resident set.
    pub client_bytes: u64,
    /// The daemon's resident set, or `None` when no daemon was found.
    pub server_bytes: Option<u64>,
}

impl Readout {
    /// The caption drawn in the chrome row, e.g. `ui 105 MB · srv 428 MB`.
    pub fn caption(&self) -> String {
        match self.server_bytes {
            Some(server) => format!(
                "ui {} · srv {}",
                format_bytes(self.client_bytes),
                format_bytes(server)
            ),
            None => format!("ui {}", format_bytes(self.client_bytes)),
        }
    }
}

/// Whole megabytes below a gigabyte, one decimal above: `105 MB`, `1.4 GB`.
/// Sub-megabyte precision would flicker without informing anyone.
pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{} MB", mb.round() as u64)
    }
}

/// Parse `VmRSS` out of a `/proc/<pid>/status` body. Pure so the format is
/// pinned by a test rather than by whichever kernel runs the suite.
pub fn vm_rss_bytes(status: &str) -> Option<u64> {
    let rest = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// Whether a `/proc/<pid>/cmdline` argv is the jcode daemon. The daemon is
/// `jcode serve ...` wherever its binary lives (selfdev target, shared-server
/// channel, `$PATH`), so only the executable's name and the subcommand are
/// matched.
pub fn is_daemon_cmdline(args: &[&str]) -> bool {
    let Some(first) = args.first() else {
        return false;
    };
    let exe = first.rsplit('/').next().unwrap_or(first);
    exe == "jcode" && args.get(1).copied() == Some("serve")
}

fn rss_of(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    vm_rss_bytes(&status)
}

/// Scan `/proc` for the daemon's pid. A scan is cheap enough for how rarely it
/// runs: [`Sampler`] caches the answer and only rescans when the cached pid
/// stops answering (daemon restarted).
fn find_daemon_pid() -> Option<u32> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let args: Vec<&str> = cmdline
            .split(|byte| *byte == 0)
            .map(|arg| std::str::from_utf8(arg).unwrap_or(""))
            .collect();
        if is_daemon_cmdline(&args) {
            return Some(pid);
        }
    }
    None
}

/// Throttled `/proc` sampler. Owned by the app rather than the model so a
/// frame stays a pure function of the model and captures stay deterministic.
#[derive(Default)]
pub struct Sampler {
    last: Option<Instant>,
    daemon_pid: Option<u32>,
}

impl Sampler {
    /// A fresh readout when the refresh interval has elapsed, else `None`
    /// (keep showing the previous one).
    pub fn sample(&mut self, now: Instant) -> Option<Readout> {
        if let Some(last) = self.last
            && now.duration_since(last) < REFRESH
        {
            return None;
        }
        self.last = Some(now);
        let client_bytes = rss_of(std::process::id())?;
        // Reuse the cached daemon pid while it still answers; a dead or
        // recycled pid falls through to a rescan, so a daemon restart shows
        // the new process rather than freezing on the old figure.
        let server_bytes = self.daemon_pid.and_then(rss_of).or_else(|| {
            self.daemon_pid = find_daemon_pid();
            self.daemon_pid.and_then(rss_of)
        });
        Some(Readout {
            client_bytes,
            server_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_rss_is_parsed_from_a_status_body() {
        let status =
            "Name:\tjcode-desktop2\nVmPeak:\t 1319964 kB\nVmRSS:\t  101828 kB\nThreads:\t18\n";
        assert_eq!(vm_rss_bytes(status), Some(101_828 * 1024));
    }

    #[test]
    fn a_status_without_vm_rss_is_none() {
        assert_eq!(vm_rss_bytes("Name:\tkthreadd\nThreads:\t1\n"), None);
    }

    #[test]
    fn the_daemon_is_matched_by_name_and_subcommand_only() {
        assert!(is_daemon_cmdline(&[
            "/home/j/.jcode/builds/shared-server/jcode",
            "serve",
            "--socket",
            "/run/user/1000/jcode.sock",
        ]));
        assert!(is_daemon_cmdline(&["jcode", "serve"]));
        // The desktop client, the bridge, and an interactive `jcode` must not
        // count as the server.
        assert!(!is_daemon_cmdline(&["/x/jcode-desktop2"]));
        assert!(!is_daemon_cmdline(&["/x/jcode-harness-api-bridge"]));
        assert!(!is_daemon_cmdline(&["/x/jcode"]));
        assert!(!is_daemon_cmdline(&["/x/jcode", "self-dev"]));
        assert!(!is_daemon_cmdline(&[]));
    }

    #[test]
    fn bytes_format_as_whole_mb_then_fractional_gb() {
        assert_eq!(format_bytes(0), "0 MB");
        assert_eq!(format_bytes(105 * 1024 * 1024), "105 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5 GB");
    }

    #[test]
    fn the_caption_names_both_sides_and_degrades_without_a_daemon() {
        let both = Readout {
            client_bytes: 105 * 1024 * 1024,
            server_bytes: Some(428 * 1024 * 1024),
        };
        assert_eq!(both.caption(), "ui 105 MB · srv 428 MB");
        let alone = Readout {
            client_bytes: 105 * 1024 * 1024,
            server_bytes: None,
        };
        assert_eq!(alone.caption(), "ui 105 MB");
    }

    #[test]
    fn the_sampler_throttles_between_refreshes() {
        let mut sampler = Sampler::default();
        let start = Instant::now();
        // The first sample always reads (this test runs on Linux, where
        // /proc/self exists, so it must succeed).
        assert!(sampler.sample(start).is_some());
        // Inside the window nothing is re-read.
        assert!(sampler.sample(start + Duration::from_millis(100)).is_none());
        // Past the window it reads again.
        assert!(sampler.sample(start + REFRESH).is_some());
    }
}
