//! Read-only native SSH machine telemetry. Rendering only clones a cached snapshot.
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub enum CloudProbeState {
    Loading,
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudStatusInfo {
    pub host: String,
    pub remote_dir: String,
    pub local_dir: Option<String>,
    pub local_is_source: bool,
    pub started_at: SystemTime,
    pub last_success: Option<SystemTime>,
    pub probe_duration_ms: Option<u64>,
    pub state: CloudProbeState,
    pub machine: Option<CloudMachineStats>,
    pub last_transfer_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudMachineStats {
    pub hostname: Option<String>,
    pub cpu_percent: Option<f64>,
    pub cpu_count: Option<u32>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub disk_available_bytes: Option<u64>,
    pub uptime_secs: Option<u64>,
}

// NUL separates independent sources. Missing tools/files only remove their own fields.
const SCRIPT: &str = r#"
export LC_ALL=C
printf 'jcode-machine-v1\000'
hostname 2>/dev/null || :
printf '\000'
awk '/^cpu/ {print}' /proc/stat 2>/dev/null || :
printf '\000'
cat /proc/meminfo 2>/dev/null || :
printf '\000'
df -Pk -- "$1" 2>/dev/null | awk 'NR == 2 {print $2, $4}'
printf '\000'
cat /proc/uptime 2>/dev/null || :
printf '\000'
"#;

#[derive(Debug, Clone, Copy, PartialEq)]
struct CpuTicks {
    total: u64,
    idle: u64,
}

fn cpu_percent(previous: CpuTicks, current: CpuTicks) -> Option<f64> {
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    if total == 0 || idle > total {
        return None;
    }
    Some((total - idle) as f64 * 100.0 / total as f64)
}

fn parse(
    bytes: &[u8],
    previous: Option<CpuTicks>,
) -> Option<(CloudMachineStats, Option<CpuTicks>)> {
    let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
    if fields.len() != 7 || fields[0] != b"jcode-machine-v1" || !fields[6].is_empty() {
        return None;
    }
    let mut stats = CloudMachineStats {
        hostname: std::str::from_utf8(fields[1])
            .ok()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
            .map(str::to_owned),
        ..Default::default()
    };
    let mut ticks = None;
    if let Ok(text) = std::str::from_utf8(fields[2]) {
        let mut count = 0u32;
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let Some(name) = words.next() else { continue };
            if name == "cpu" {
                let values: Option<Vec<u64>> = words.take(8).map(|s| s.parse().ok()).collect();
                if let Some(v) = values.filter(|v| v.len() >= 4) {
                    ticks = v
                        .iter()
                        .try_fold(0u64, |sum, v| sum.checked_add(*v))
                        .and_then(|total| {
                            Some(CpuTicks {
                                total,
                                idle: v[3].checked_add(*v.get(4).unwrap_or(&0))?,
                            })
                        });
                }
            } else if name
                .strip_prefix("cpu")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            {
                count = count.saturating_add(1);
            }
        }
        stats.cpu_count = (count > 0).then_some(count);
    }
    stats.cpu_percent = previous.zip(ticks).and_then(|(a, b)| cpu_percent(a, b));
    if let Ok(text) = std::str::from_utf8(fields[3]) {
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let key = words.next();
            let value = words.next().and_then(|s| s.parse::<u64>().ok());
            let value = if words.next() == Some("kB") {
                value.and_then(|v| v.checked_mul(1024))
            } else {
                None
            };
            match key {
                Some("MemTotal:") => stats.memory_total_bytes = value,
                Some("MemAvailable:") => stats.memory_available_bytes = value,
                _ => {}
            }
        }
    }
    if let Ok(text) = std::str::from_utf8(fields[4]) {
        let mut words = text.split_whitespace();
        stats.disk_total_bytes = words
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024));
        stats.disk_available_bytes = words
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024));
    }
    stats.uptime_secs = std::str::from_utf8(fields[5])
        .ok()
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| {
            let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
            if !fraction.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            whole.parse().ok()
        });
    Some((stats, ticks))
}

fn apply_result(
    info: &mut CloudStatusInfo,
    result: Option<CloudMachineStats>,
    elapsed: Duration,
    now: SystemTime,
) {
    info.probe_duration_ms = Some(elapsed.as_millis().min(u64::MAX as u128) as u64);
    if let Some(machine) = result {
        info.machine = Some(machine);
        info.last_success = Some(now);
        info.state = CloudProbeState::Ready;
    } else {
        // Keep last_success and machine to let the UI show explicitly stale values.
        info.state = CloudProbeState::Unavailable;
    }
}

#[cfg(not(test))]
pub(crate) fn gather() -> Option<CloudStatusInfo> {
    use std::sync::{Arc, Mutex, OnceLock};
    let host = super::ssh_remote_host()?;
    if host.is_empty()
        || host.starts_with('-')
        || host.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    static CACHE: OnceLock<Arc<Mutex<CloudStatusInfo>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        let remote_dir = std::env::var("JCODE_SSH_WORKING_DIR").unwrap_or_default();
        let cache = Arc::new(Mutex::new(CloudStatusInfo {
            host: host.clone(),
            remote_dir: remote_dir.clone(),
            local_dir: std::env::var("JCODE_SSH_LOCAL_WORKING_DIR").ok(),
            local_is_source: std::env::var("JCODE_SSH_LOCAL_IS_SOURCE").as_deref() == Ok("1"),
            started_at: SystemTime::now(),
            last_success: None,
            probe_duration_ms: None,
            state: CloudProbeState::Loading,
            machine: None,
            last_transfer_at: std::env::var("JCODE_SSH_LAST_TRANSFER_AT").ok(),
        }));
        let output = cache.clone();
        let config = std::env::var_os("JCODE_SSH_CONFIG").map(std::path::PathBuf::from);
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                output.lock().unwrap_or_else(|e| e.into_inner()).state =
                    CloudProbeState::Unavailable;
                crate::bus::Bus::global().publish(crate::bus::BusEvent::GitInfoRefreshed);
                return;
            };
            runtime.block_on(async move {
                let mut previous = None;
                loop {
                    let start = std::time::Instant::now();
                    let response = super::cloud_git::bounded_output(
                        super::cloud_git::script_command(
                            Some(&host),
                            config.as_deref(),
                            &remote_dir,
                            SCRIPT,
                        ),
                        Duration::from_secs(12),
                        256 * 1024,
                    )
                    .await;
                    let parsed = response
                        .filter(|(status, _)| status.success())
                        .and_then(|(_, bytes)| parse(&bytes, previous));
                    let machine = parsed.map(|(machine, ticks)| {
                        previous = ticks;
                        machine
                    });
                    apply_result(
                        &mut output.lock().unwrap_or_else(|e| e.into_inner()),
                        machine,
                        start.elapsed(),
                        SystemTime::now(),
                    );
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::GitInfoRefreshed);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
        });
        cache
    });
    Some(cache.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

#[cfg(test)]
pub(crate) fn gather() -> Option<CloudStatusInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_sample_and_individually_invalid_fields() {
        let previous = CpuTicks {
            total: 100,
            idle: 50,
        };
        let (stats, _) = parse(b"jcode-machine-v1\0machine\n\0cpu 50 0 30 100 20 0 0 0\ncpu0 1\n\0MemTotal: 200 kB\nMemAvailable: 80 kB\n\01000 600\0123.75 2\0", Some(previous)).unwrap();
        assert_eq!(stats.hostname.as_deref(), Some("machine"));
        assert_eq!(stats.cpu_percent, Some(30.0));
        assert_eq!(stats.memory_available_bytes, Some(80 * 1024));
        assert_eq!(stats.disk_total_bytes, Some(1000 * 1024));
        let (invalid, ticks) = parse(b"jcode-machine-v1\0host\0cpu 18446744073709551615 1 0 0\n\0MemTotal: 18446744073709551615 kB\nMemAvailable: 8 MB\n\0bad 2\0-1.0 0\0", None).unwrap();
        assert_eq!(invalid.hostname.as_deref(), Some("host"));
        assert_eq!(ticks, None);
        assert_eq!(invalid.memory_total_bytes, None);
        assert_eq!(invalid.memory_available_bytes, None);
        assert_eq!(invalid.disk_total_bytes, None);
        assert_eq!(invalid.disk_available_bytes, Some(2048));
        assert_eq!(invalid.uptime_secs, None);
        assert_eq!(
            cpu_percent(
                previous,
                CpuTicks {
                    total: 110,
                    idle: 100
                }
            ),
            None
        );
    }

    #[test]
    fn partial_invalid_and_cpu_deltas() {
        let (stats, ticks) = parse(b"jcode-machine-v1\0host\ncpu?\0cpu 10 0 20 70 0 0 0 0 100 100\ncpu0 1\ncpu1 1\n\0MemTotal: 10 kB\nMemAvailable: invalid kB\n\0100 20\0123.5 2\0", None).unwrap();
        assert_eq!(stats.hostname, None);
        assert_eq!(stats.cpu_count, Some(2));
        assert_eq!(stats.cpu_percent, None);
        assert_eq!(
            ticks,
            Some(CpuTicks {
                total: 100,
                idle: 70
            })
        );
        assert_eq!(stats.memory_total_bytes, Some(10240));
        assert_eq!(stats.memory_available_bytes, None);
        assert_eq!(stats.disk_available_bytes, Some(20480));
        assert_eq!(stats.uptime_secs, Some(123));
        assert_eq!(
            cpu_percent(
                ticks.unwrap(),
                CpuTicks {
                    total: 200,
                    idle: 95
                }
            ),
            Some(75.0)
        );
        assert_eq!(cpu_percent(ticks.unwrap(), ticks.unwrap()), None);
        assert_eq!(
            cpu_percent(ticks.unwrap(), CpuTicks { total: 1, idle: 0 }),
            None
        );
        assert!(parse(b"bad", None).is_none());
        let (empty, _) = parse(b"jcode-machine-v1\0\0\0\0\0\0", None).unwrap();
        assert_eq!(empty, CloudMachineStats::default());
        assert!(gather().is_none());
    }
    #[test]
    fn failure_retains_stale_success() {
        let now = SystemTime::UNIX_EPOCH;
        let mut info = CloudStatusInfo {
            host: "host".into(),
            remote_dir: "/repo".into(),
            local_dir: None,
            local_is_source: false,
            started_at: now,
            last_success: None,
            probe_duration_ms: None,
            state: CloudProbeState::Loading,
            machine: None,
            last_transfer_at: None,
        };
        apply_result(
            &mut info,
            Some(CloudMachineStats::default()),
            Duration::from_millis(20),
            now,
        );
        assert_eq!(info.state, CloudProbeState::Ready);
        apply_result(
            &mut info,
            None,
            Duration::from_secs(12),
            now + Duration::from_secs(20),
        );
        assert_eq!(info.state, CloudProbeState::Unavailable);
        assert_eq!(info.last_success, Some(now));
        assert!(info.machine.is_some());
        assert_eq!(info.probe_duration_ms, Some(12000));
    }
    #[tokio::test]
    async fn bounded_transport_timeout_and_output_cap() {
        let cmd = super::super::cloud_git::script_command(None, None, "/", "sleep 30");
        let start = std::time::Instant::now();
        assert!(
            super::super::cloud_git::bounded_output(cmd, Duration::from_millis(40), 100)
                .await
                .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let cmd = super::super::cloud_git::script_command(None, None, "/", "printf '123456'");
        assert!(
            super::super::cloud_git::bounded_output(cmd, Duration::from_secs(1), 5)
                .await
                .is_none()
        );
        // Test fixed script framing without sampling the local host's process data.
        assert!(SCRIPT.contains("jcode-machine-v1"));
    }
}
