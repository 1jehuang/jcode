use anyhow::{Result, anyhow};
use serde::Serialize;
use std::collections::VecDeque;
#[cfg(feature = "jemalloc-prof")]
use std::ffi::CString;
use std::path::Path;
#[cfg(feature = "jemalloc-prof")]
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const MAX_HISTORY_SAMPLES: usize = 512;

#[cfg(feature = "jemalloc")]
struct JemallocStatsMibs {
    epoch: tikv_jemalloc_ctl::epoch_mib,
    allocated: tikv_jemalloc_ctl::stats::allocated_mib,
    active: tikv_jemalloc_ctl::stats::active_mib,
    metadata: tikv_jemalloc_ctl::stats::metadata_mib,
    resident: tikv_jemalloc_ctl::stats::resident_mib,
    mapped: tikv_jemalloc_ctl::stats::mapped_mib,
    retained: tikv_jemalloc_ctl::stats::retained_mib,
}

#[cfg(feature = "jemalloc-prof")]
struct JemallocProfilingMibs {
    enabled: tikv_jemalloc_ctl::profiling::prof_mib,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProcessMemorySnapshot {
    pub rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<OsProcessMemoryInfo>,
    pub allocator: AllocatorInfo,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OsProcessMemoryInfo {
    pub pss_bytes: Option<u64>,
    pub rss_anon_bytes: Option<u64>,
    pub rss_file_bytes: Option<u64>,
    pub rss_shmem_bytes: Option<u64>,
    pub private_clean_bytes: Option<u64>,
    pub private_dirty_bytes: Option<u64>,
    pub shared_clean_bytes: Option<u64>,
    pub shared_dirty_bytes: Option<u64>,
    pub swap_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AllocatorInfo {
    pub name: &'static str,
    pub stats_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<AllocatorStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiling: Option<AllocatorProfilingInfo>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AllocatorStats {
    pub allocated_bytes: Option<u64>,
    pub active_bytes: Option<u64>,
    pub metadata_bytes: Option<u64>,
    pub resident_bytes: Option<u64>,
    pub mapped_bytes: Option<u64>,
    pub retained_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AllocatorProfilingInfo {
    pub available: bool,
    pub enabled: Option<bool>,
}

impl Default for AllocatorInfo {
    fn default() -> Self {
        allocator_info()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessMemoryHistoryEntry {
    pub timestamp_ms: u128,
    pub source: String,
    pub snapshot: ProcessMemorySnapshot,
}

static MEMORY_HISTORY: OnceLock<Mutex<VecDeque<ProcessMemoryHistoryEntry>>> = OnceLock::new();

fn memory_history() -> &'static Mutex<VecDeque<ProcessMemoryHistoryEntry>> {
    MEMORY_HISTORY.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_HISTORY_SAMPLES)))
}

#[cfg(target_os = "linux")]
pub fn snapshot() -> ProcessMemorySnapshot {
    snapshot_with_source("snapshot")
}

#[cfg(not(target_os = "linux"))]
pub fn snapshot() -> ProcessMemorySnapshot {
    snapshot_with_source("snapshot")
}

#[cfg(target_os = "linux")]
pub fn snapshot_with_source(source: impl Into<String>) -> ProcessMemorySnapshot {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        let snapshot = ProcessMemorySnapshot::default();
        record_snapshot(source.into(), snapshot.clone());
        return snapshot;
    };

    let snapshot = ProcessMemorySnapshot {
        rss_bytes: parse_proc_status_value_bytes(&status, "VmRSS:"),
        peak_rss_bytes: parse_proc_status_value_bytes(&status, "VmHWM:"),
        virtual_bytes: parse_proc_status_value_bytes(&status, "VmSize:"),
        os: read_linux_memory_info(&status),
        allocator: allocator_info(),
    };
    record_snapshot(source.into(), snapshot.clone());
    snapshot
}

#[cfg(not(target_os = "linux"))]
pub fn snapshot_with_source(source: impl Into<String>) -> ProcessMemorySnapshot {
    let snapshot = ProcessMemorySnapshot::default();
    record_snapshot(source.into(), snapshot.clone());
    snapshot
}

pub fn history(limit: usize) -> Vec<ProcessMemoryHistoryEntry> {
    let Ok(history) = memory_history().lock() else {
        return Vec::new();
    };
    history.iter().rev().take(limit).cloned().collect()
}

pub fn allocator_info() -> AllocatorInfo {
    #[cfg(feature = "jemalloc")]
    {
        let stats = jemalloc_stats();
        let profiling = jemalloc_profiling_info();
        return AllocatorInfo {
            name: "jemalloc",
            stats_available: stats.is_some(),
            stats,
            profiling,
        };
    }

    #[cfg(not(feature = "jemalloc"))]
    {
        AllocatorInfo {
            name: "system",
            stats_available: false,
            stats: None,
            profiling: None,
        }
    }
}

pub fn set_allocator_profiling_active(active: bool) -> Result<()> {
    #[cfg(feature = "jemalloc-prof")]
    {
        unsafe {
            tikv_jemalloc_ctl::raw::write(b"prof.active\0", active)
                .map_err(|e| anyhow!("failed to update jemalloc prof.active: {}", e))
        }
    }

    #[cfg(not(feature = "jemalloc-prof"))]
    {
        let _ = active;
        Err(anyhow!(
            "jemalloc profiling controls unavailable: rebuild with --features jemalloc-prof"
        ))
    }
}

pub fn dump_allocator_profile(path: Option<&Path>) -> Result<PathBuf> {
    #[cfg(feature = "jemalloc-prof")]
    {
        let output_path = match path {
            Some(path) => path.to_path_buf(),
            None => default_heap_profile_path()?,
        };

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let c_path = CString::new(output_path.to_string_lossy().as_bytes())
            .map_err(|_| anyhow!("heap profile path contains NUL byte"))?;

        unsafe {
            tikv_jemalloc_ctl::raw::write(b"prof.dump\0", c_path.as_ptr())
                .map_err(|e| anyhow!("failed to dump jemalloc heap profile: {}", e))?;
        }

        Ok(output_path)
    }

    #[cfg(not(feature = "jemalloc-prof"))]
    {
        let _ = path;
        Err(anyhow!(
            "jemalloc heap dumps unavailable: rebuild with --features jemalloc-prof"
        ))
    }
}

pub fn set_allocator_profile_prefix(prefix: &str) -> Result<()> {
    #[cfg(feature = "jemalloc-prof")]
    {
        let c_prefix =
            CString::new(prefix).map_err(|_| anyhow!("heap profile prefix contains NUL byte"))?;
        unsafe {
            tikv_jemalloc_ctl::raw::write(b"prof.prefix\0", c_prefix.as_ptr())
                .map_err(|e| anyhow!("failed to update jemalloc prof.prefix: {}", e))
        }
    }

    #[cfg(not(feature = "jemalloc-prof"))]
    {
        let _ = prefix;
        Err(anyhow!(
            "jemalloc heap profiling unavailable: rebuild with --features jemalloc-prof"
        ))
    }
}

pub fn estimate_json_bytes<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn record_snapshot(source: String, snapshot: ProcessMemorySnapshot) {
    let Ok(mut history) = memory_history().lock() else {
        return;
    };
    if history.len() >= MAX_HISTORY_SAMPLES {
        history.pop_front();
    }
    history.push_back(ProcessMemoryHistoryEntry {
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
        source,
        snapshot,
    });
}

#[cfg(target_os = "linux")]
fn read_linux_memory_info(status: &str) -> Option<OsProcessMemoryInfo> {
    let smaps = std::fs::read_to_string("/proc/self/smaps_rollup").ok();
    let info = OsProcessMemoryInfo {
        pss_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Pss:")),
        rss_anon_bytes: parse_proc_status_value_bytes(status, "RssAnon:"),
        rss_file_bytes: parse_proc_status_value_bytes(status, "RssFile:"),
        rss_shmem_bytes: parse_proc_status_value_bytes(status, "RssShmem:"),
        private_clean_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Private_Clean:")),
        private_dirty_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Private_Dirty:")),
        shared_clean_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Shared_Clean:")),
        shared_dirty_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Shared_Dirty:")),
        swap_bytes: parse_proc_status_value_bytes(status, "VmSwap:").or_else(|| {
            smaps
                .as_deref()
                .and_then(|text| parse_proc_value_bytes(text, "Swap:"))
        }),
    };

    if info.pss_bytes.is_none()
        && info.rss_anon_bytes.is_none()
        && info.rss_file_bytes.is_none()
        && info.rss_shmem_bytes.is_none()
        && info.private_clean_bytes.is_none()
        && info.private_dirty_bytes.is_none()
        && info.shared_clean_bytes.is_none()
        && info.shared_dirty_bytes.is_none()
        && info.swap_bytes.is_none()
    {
        None
    } else {
        Some(info)
    }
}

#[cfg(feature = "jemalloc-prof")]
fn default_heap_profile_path() -> Result<PathBuf> {
    let base = crate::storage::jcode_dir()?.join("profiles").join("heap");
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let pid = std::process::id();
    Ok(base.join(format!("jcode-{}-{}.heap", pid, timestamp)))
}

#[cfg(feature = "jemalloc")]
fn jemalloc_stats() -> Option<AllocatorStats> {
    let mibs = jemalloc_stats_mibs()?;
    mibs.epoch.advance().ok()?;

    Some(AllocatorStats {
        allocated_bytes: mibs.allocated.read().ok().map(|value| value as u64),
        active_bytes: mibs.active.read().ok().map(|value| value as u64),
        metadata_bytes: mibs.metadata.read().ok().map(|value| value as u64),
        resident_bytes: mibs.resident.read().ok().map(|value| value as u64),
        mapped_bytes: mibs.mapped.read().ok().map(|value| value as u64),
        retained_bytes: mibs.retained.read().ok().map(|value| value as u64),
    })
}

#[cfg(feature = "jemalloc")]
fn jemalloc_stats_mibs() -> Option<&'static JemallocStatsMibs> {
    static MIBS: OnceLock<Option<JemallocStatsMibs>> = OnceLock::new();
    MIBS.get_or_init(|| {
        Some(JemallocStatsMibs {
            epoch: tikv_jemalloc_ctl::epoch::mib().ok()?,
            allocated: tikv_jemalloc_ctl::stats::allocated::mib().ok()?,
            active: tikv_jemalloc_ctl::stats::active::mib().ok()?,
            metadata: tikv_jemalloc_ctl::stats::metadata::mib().ok()?,
            resident: tikv_jemalloc_ctl::stats::resident::mib().ok()?,
            mapped: tikv_jemalloc_ctl::stats::mapped::mib().ok()?,
            retained: tikv_jemalloc_ctl::stats::retained::mib().ok()?,
        })
    })
    .as_ref()
}

#[cfg(feature = "jemalloc-prof")]
fn jemalloc_profiling_info() -> Option<AllocatorProfilingInfo> {
    let mibs = jemalloc_profiling_mibs()?;
    Some(AllocatorProfilingInfo {
        available: true,
        enabled: mibs.enabled.read().ok(),
    })
}

#[cfg(all(feature = "jemalloc", not(feature = "jemalloc-prof")))]
fn jemalloc_profiling_info() -> Option<AllocatorProfilingInfo> {
    Some(AllocatorProfilingInfo {
        available: false,
        enabled: None,
    })
}

#[cfg(feature = "jemalloc-prof")]
fn jemalloc_profiling_mibs() -> Option<&'static JemallocProfilingMibs> {
    static MIBS: OnceLock<Option<JemallocProfilingMibs>> = OnceLock::new();
    MIBS.get_or_init(|| {
        Some(JemallocProfilingMibs {
            enabled: tikv_jemalloc_ctl::profiling::prof::mib().ok()?,
        })
    })
    .as_ref()
}

#[cfg(target_os = "linux")]
fn parse_proc_status_value_bytes(status: &str, key: &str) -> Option<u64> {
    parse_proc_value_bytes(status, key)
}

#[cfg(target_os = "linux")]
fn parse_proc_value_bytes(status: &str, key: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(key) {
            return None;
        }
        let value = trimmed.trim_start_matches(key).trim();
        let mut parts = value.split_whitespace();
        let number = parts.next()?.parse::<u64>().ok()?;
        let unit = parts.next().unwrap_or("kB");
        Some(match unit {
            "kB" | "KB" | "kb" => number.saturating_mul(1024),
            "mB" | "MB" | "mb" => number.saturating_mul(1024 * 1024),
            "gB" | "GB" | "gb" => number.saturating_mul(1024 * 1024 * 1024),
            _ => number,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocator_info_matches_enabled_allocator_features() {
        let info = allocator_info();
        if cfg!(feature = "jemalloc") {
            assert_eq!(info.name, "jemalloc");
            assert_eq!(info.stats_available, info.stats.is_some());
            assert!(info.profiling.is_some());
        } else {
            assert_eq!(info.name, "system");
            assert!(!info.stats_available);
            assert!(info.stats.is_none());
            assert!(info.profiling.is_none());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_proc_value_bytes_handles_kib_and_mib_units() {
        let text = "Pss:               123 kB\nMapped:            2 MB\nRetained:          1 GB\n";
        assert_eq!(parse_proc_value_bytes(text, "Pss:"), Some(123 * 1024));
        assert_eq!(
            parse_proc_value_bytes(text, "Mapped:"),
            Some(2 * 1024 * 1024)
        );
        assert_eq!(
            parse_proc_value_bytes(text, "Retained:"),
            Some(1024 * 1024 * 1024)
        );
    }
}
