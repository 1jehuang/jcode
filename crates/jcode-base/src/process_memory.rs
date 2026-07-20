use crate::logging;
use anyhow::{Result, anyhow};
#[cfg(feature = "jemalloc")]
use libc::c_char;
use serde::Serialize;
use std::collections::VecDeque;
#[cfg(feature = "jemalloc")]
use std::ffi::CString;
use std::path::Path;
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
    /// Number of OS threads (`Threads:` in `/proc/self/status`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_count: Option<u64>,
    /// Main thread stack size (`VmStk:` in `/proc/self/status`). Auxiliary
    /// thread stacks live in anonymous mappings and are not included here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_stack_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<OsProcessMemoryInfo>,
    pub allocator: AllocatorInfo,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OsProcessMemoryInfo {
    pub pss_bytes: Option<u64>,
    /// Proportional set size of anonymous mappings (`Pss_Anon:` in
    /// smaps_rollup): heap + thread stacks + other private anon memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_anon_bytes: Option<u64>,
    /// Proportional set size of file-backed mappings (`Pss_File:`): mostly
    /// the executable text/rodata and shared libraries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_file_bytes: Option<u64>,
    /// Proportional set size of shmem mappings (`Pss_Shmem:`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pss_shmem_bytes: Option<u64>,
    /// Bytes backed by transparent huge pages (`AnonHugePages:`); a subset of
    /// anon memory that amplifies allocator retention (one live allocation
    /// pins a whole 2MB page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anon_huge_pages_bytes: Option<u64>,
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
    pub tuning: Option<AllocatorTuningInfo>,
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

#[derive(Debug, Clone, Default, Serialize)]
pub struct AllocatorTuningInfo {
    pub available: bool,
    pub background_thread: Option<bool>,
    pub max_background_threads: Option<u64>,
    pub arena_count: Option<u64>,
    pub initialized_arenas: Option<u64>,
    pub dirty_decay_ms: Option<i64>,
    pub muzzy_decay_ms: Option<i64>,
    pub retain: Option<bool>,
    pub tcache_enabled: Option<bool>,
    pub tcache_max_bytes: Option<u64>,
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
    let source = source.into();
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        logging::warn(&format!(
            "process memory snapshot source={source} missing /proc/self/status; using defaults"
        ));
        let snapshot = ProcessMemorySnapshot::default();
        record_snapshot(source, snapshot.clone());
        return snapshot;
    };

    let snapshot = ProcessMemorySnapshot {
        rss_bytes: parse_proc_status_value_bytes(&status, "VmRSS:"),
        peak_rss_bytes: parse_proc_status_value_bytes(&status, "VmHWM:"),
        virtual_bytes: parse_proc_status_value_bytes(&status, "VmSize:"),
        thread_count: parse_proc_status_count(&status, "Threads:"),
        main_stack_bytes: parse_proc_status_value_bytes(&status, "VmStk:"),
        os: read_linux_memory_info(&status),
        allocator: allocator_info(),
    };
    logging::debug(&format!(
        "process memory snapshot source={source} rss={:?} peak_rss={:?} virtual={:?} allocator={}",
        snapshot.rss_bytes,
        snapshot.peak_rss_bytes,
        snapshot.virtual_bytes,
        snapshot.allocator.name
    ));
    record_snapshot(source, snapshot.clone());
    snapshot
}

#[cfg(not(target_os = "linux"))]
pub fn snapshot_with_source(source: impl Into<String>) -> ProcessMemorySnapshot {
    let source = source.into();
    logging::debug(&format!(
        "process memory snapshot source={source} using default non-linux implementation"
    ));
    let snapshot = ProcessMemorySnapshot::default();
    record_snapshot(source, snapshot.clone());
    snapshot
}

pub fn history(limit: usize) -> Vec<ProcessMemoryHistoryEntry> {
    let Ok(history) = memory_history().lock() else {
        logging::error("process memory history lock poisoned; returning empty history");
        return Vec::new();
    };
    history.iter().rev().take(limit).cloned().collect()
}

pub fn allocator_info() -> AllocatorInfo {
    #[cfg(feature = "jemalloc")]
    {
        let stats = jemalloc_stats();
        let profiling = jemalloc_profiling_info();
        AllocatorInfo {
            name: "jemalloc",
            stats_available: stats.is_some(),
            stats,
            tuning: jemalloc_tuning_info(),
            profiling,
        }
    }

    #[cfg(not(feature = "jemalloc"))]
    {
        let stats = glibc_malloc_stats();
        AllocatorInfo {
            name: "system",
            stats_available: stats.is_some(),
            stats,
            tuning: None,
            profiling: None,
        }
    }
}

/// Read glibc malloc statistics via `mallinfo2` (glibc >= 2.33).
///
/// This does not attribute memory to app structures, but it splits process
/// heap into "live" (bytes the app currently holds) and "retained" (bytes
/// freed by the app but kept by the allocator), which is the distinction that
/// matters when diagnosing unattributed RSS.
///
/// `mallinfo2` is resolved with `dlsym` instead of linked directly: release
/// binaries are built against a glibc 2.17 (manylinux2014) baseline where the
/// symbol does not exist, so a direct call fails to link. At runtime on a
/// modern glibc the lookup succeeds and stats work as before; on an old glibc
/// this returns `None` and callers already treat stats as unavailable.
#[cfg(all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc")))]
fn glibc_malloc_stats() -> Option<AllocatorStats> {
    // Mirrors glibc's `struct mallinfo2` (all fields `size_t`).
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Mallinfo2 {
        arena: libc::size_t,
        ordblks: libc::size_t,
        smblks: libc::size_t,
        hblks: libc::size_t,
        hblkhd: libc::size_t,
        usmblks: libc::size_t,
        fsmblks: libc::size_t,
        uordblks: libc::size_t,
        fordblks: libc::size_t,
        keepcost: libc::size_t,
    }
    type Mallinfo2Fn = unsafe extern "C" fn() -> Mallinfo2;

    static MALLINFO2: std::sync::OnceLock<Option<Mallinfo2Fn>> = std::sync::OnceLock::new();
    let mallinfo2 = (*MALLINFO2.get_or_init(|| {
        // Safety: dlsym with a NUL-terminated literal; the default namespace
        // (RTLD_DEFAULT) searches the already-loaded glibc.
        let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"mallinfo2".as_ptr()) };
        if sym.is_null() {
            None
        } else {
            // Safety: glibc's mallinfo2 has exactly this signature.
            Some(unsafe { std::mem::transmute::<*mut libc::c_void, Mallinfo2Fn>(sym) })
        }
    }))?;

    // Totals are summed across all arenas by modern glibc.
    // uordblks: in-use arena bytes; fordblks: freed-but-retained arena bytes;
    // hblkhd: mmap-backed allocation bytes; arena: total sbrk/mmap arena size.
    let info = unsafe { mallinfo2() };
    let live = (info.uordblks as u64).saturating_add(info.hblkhd as u64);
    let mapped = (info.arena as u64).saturating_add(info.hblkhd as u64);
    Some(AllocatorStats {
        allocated_bytes: Some(live),
        active_bytes: Some(info.uordblks as u64),
        metadata_bytes: None,
        resident_bytes: None,
        mapped_bytes: Some(mapped),
        retained_bytes: Some(info.fordblks as u64),
    })
}

#[cfg(all(
    not(all(target_os = "linux", target_env = "gnu")),
    not(feature = "jemalloc")
))]
fn glibc_malloc_stats() -> Option<AllocatorStats> {
    None
}

pub fn purge_allocator() -> Result<AllocatorTuningInfo> {
    #[cfg(feature = "jemalloc")]
    {
        logging::info("purging jemalloc allocator arenas");
        let _ = jemalloc_void_ctl("thread.idle");
        let arena_count = tikv_jemalloc_ctl::arenas::narenas::read()
            .map_err(|e| anyhow!("failed to read jemalloc arena count: {}", e))?;
        let mut initialized_arenas = 0u64;
        for arena_idx in 0..arena_count {
            if jemalloc_read_dynamic::<bool>(&format!("arena.{arena_idx}.initialized"))
                .unwrap_or(false)
            {
                initialized_arenas += 1;
                jemalloc_void_ctl(&format!("arena.{arena_idx}.purge"))?;
            }
        }

        Ok(jemalloc_tuning_info().unwrap_or(AllocatorTuningInfo {
            available: true,
            initialized_arenas: Some(initialized_arenas),
            ..AllocatorTuningInfo::default()
        }))
    }

    #[cfg(all(target_os = "linux", not(feature = "jemalloc")))]
    {
        // glibc has no arena purge API, but malloc_trim(0) walks all arenas
        // and returns freed pages to the OS (MADV_DONTNEED), which is the
        // equivalent retained-memory release.
        logging::info("purging glibc allocator via malloc_trim(0)");
        release_retained_heap("debug_allocator_purge");
        Ok(AllocatorTuningInfo {
            available: true,
            ..AllocatorTuningInfo::default()
        })
    }

    #[cfg(all(not(target_os = "linux"), not(feature = "jemalloc")))]
    {
        logging::warn("allocator purge requested but no purge mechanism is available");
        Err(anyhow!(
            "allocator purge unavailable on this platform: rebuild with --features jemalloc"
        ))
    }
}

pub fn set_allocator_decay_ms(dirty_ms: isize, muzzy_ms: isize) -> Result<AllocatorTuningInfo> {
    logging::info(&format!(
        "setting allocator decay dirty_ms={dirty_ms} muzzy_ms={muzzy_ms}"
    ));
    #[cfg(feature = "jemalloc")]
    {
        unsafe {
            tikv_jemalloc_ctl::raw::write(b"arenas.dirty_decay_ms\0", dirty_ms)
                .map_err(|e| anyhow!("failed to update arenas.dirty_decay_ms: {}", e))?;
            tikv_jemalloc_ctl::raw::write(b"arenas.muzzy_decay_ms\0", muzzy_ms)
                .map_err(|e| anyhow!("failed to update arenas.muzzy_decay_ms: {}", e))?;
        }

        let arena_count = tikv_jemalloc_ctl::arenas::narenas::read()
            .map_err(|e| anyhow!("failed to read jemalloc arena count: {}", e))?;
        for arena_idx in 0..arena_count {
            if jemalloc_read_dynamic::<bool>(&format!("arena.{arena_idx}.initialized"))
                .unwrap_or(false)
            {
                jemalloc_write_dynamic(&format!("arena.{arena_idx}.dirty_decay_ms"), dirty_ms)?;
                jemalloc_write_dynamic(&format!("arena.{arena_idx}.muzzy_decay_ms"), muzzy_ms)?;
            }
        }

        Ok(jemalloc_tuning_info().unwrap_or(AllocatorTuningInfo {
            available: true,
            dirty_decay_ms: Some(dirty_ms as i64),
            muzzy_decay_ms: Some(muzzy_ms as i64),
            ..AllocatorTuningInfo::default()
        }))
    }

    #[cfg(not(feature = "jemalloc"))]
    {
        let _ = (dirty_ms, muzzy_ms);
        logging::warn("allocator decay update requested but jemalloc feature is disabled");
        Err(anyhow!(
            "allocator decay controls unavailable: rebuild with --features jemalloc"
        ))
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
    #[derive(Default)]
    struct ByteCounter {
        bytes: usize,
    }

    impl std::io::Write for ByteCounter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(buffer.len());
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value)
        .map(|()| counter.bytes)
        .unwrap_or(0)
}

/// Return freed-but-retained heap pages to the OS.
///
/// glibc malloc keeps pages freed by large transient allocations (history
/// loads, provider payloads, render caches) inside its arenas, which shows up
/// as unattributed RSS that never shrinks. On jemalloc builds this purges all
/// arenas; on Linux system-allocator builds it calls `malloc_trim(0)`; on
/// other platforms it is a no-op.
pub fn release_retained_heap(reason: &str) {
    #[cfg(feature = "jemalloc")]
    {
        if let Err(err) = purge_allocator() {
            logging::info(&format!("jemalloc purge ({reason}) failed: {err}"));
        } else {
            logging::debug(&format!("jemalloc purge ({reason}) completed"));
        }
    }

    #[cfg(all(target_os = "linux", not(feature = "jemalloc")))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        let trimmed = unsafe { malloc_trim(0) };
        logging::debug(&format!(
            "malloc_trim ({reason}): {}",
            if trimmed == 1 {
                "released pages"
            } else {
                "no pages to release"
            }
        ));
    }

    #[cfg(all(not(target_os = "linux"), not(feature = "jemalloc")))]
    {
        let _ = reason;
    }

    // Whatever apparent retention remains after the release is the
    // unrecoverable floor (fragmentation residual); measure future growth
    // from it so retention-triggered callers stay quiet at steady state.
    record_post_trim_retention_baseline();
}

static LAST_HEAP_RELEASE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Debounced [`release_retained_heap`]: skips the release when one already ran
/// within `min_interval`. Returns true when a release was performed.
pub fn release_retained_heap_debounced(reason: &str, min_interval: std::time::Duration) -> bool {
    use std::sync::atomic::Ordering;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let last_ms = LAST_HEAP_RELEASE_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_ms) < min_interval.as_millis() as u64 {
        return false;
    }
    if LAST_HEAP_RELEASE_MS
        .compare_exchange(last_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }
    release_retained_heap(reason);
    true
}

/// Default apparent-retention growth threshold that triggers a background trim.
pub const DEFAULT_RETENTION_TRIM_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

/// Post-trim apparent-retention baseline (bytes). Updated after every
/// [`release_retained_heap`] and ratcheted down when current apparent
/// retention falls below it, so growth is always measured from the floor.
static POST_TRIM_APPARENT_RETENTION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Resident anonymous memory not accounted for by live allocator bytes:
/// freed-but-still-resident heap pages plus fragmentation overhead. This is
/// the memory a trim/purge can plausibly return to the OS, measured from the
/// OS side (RssAnon) minus the allocator's live bytes.
///
/// Allocator-reported "retained/free" counters are the wrong trigger metric
/// on glibc: `malloc_trim` releases the physical pages behind free chunks
/// (MADV_DONTNEED) but the chunks remain in `fordblks`, so that counter never
/// drops after a trim and a threshold on it re-fires forever.
#[cfg(target_os = "linux")]
fn apparent_heap_retention_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rss_anon = parse_proc_status_value_bytes(&status, "RssAnon:")?;
    let live = allocator_info().stats.as_ref()?.allocated_bytes?;
    Some(rss_anon.saturating_sub(live))
}

#[cfg(not(target_os = "linux"))]
fn apparent_heap_retention_bytes() -> Option<u64> {
    None
}

/// Refresh the post-trim baseline from the current apparent retention.
fn record_post_trim_retention_baseline() {
    if let Some(apparent) = apparent_heap_retention_bytes() {
        POST_TRIM_APPARENT_RETENTION.store(apparent, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Pure trigger decision for retention-based trimming: has apparent retention
/// grown at least `threshold` bytes above the post-trim `baseline`?
fn retention_growth_exceeds(apparent: u64, baseline: u64, threshold: u64) -> bool {
    apparent.saturating_sub(baseline) >= threshold
}

/// Release retained heap when apparent retention (RssAnon minus live
/// allocator bytes) has grown at least `threshold_bytes` above the post-trim
/// baseline. Intended for periodic (heartbeat) callers: cheap when below
/// threshold (one /proc/self/status read + allocator stats read), debounced
/// against other release paths when above it. Returns true when a release ran.
///
/// This closes the gap left by event-driven trims (turn completion, history
/// load): a server hosting many mostly-idle sessions can accumulate hundreds
/// of MB of freed-but-resident pages without ever hitting those event hooks.
/// Measuring *growth above the post-trim floor* keeps the watchdog quiet at
/// steady state: the unrecoverable fragmentation residual left after a trim
/// becomes the new baseline instead of re-triggering every cycle.
pub fn release_retained_heap_if_excessive(
    reason: &str,
    threshold_bytes: u64,
    min_interval: std::time::Duration,
) -> bool {
    use std::sync::atomic::Ordering;

    let Some(apparent) = apparent_heap_retention_bytes() else {
        // No OS-side metric available (non-Linux, or allocator stats missing):
        // fall back to the allocator-reported retained counter as an absolute
        // threshold. Coarse, but better than never trimming.
        let retained = allocator_info()
            .stats
            .and_then(|stats| stats.retained_bytes)
            .unwrap_or(0);
        if retained < threshold_bytes {
            return false;
        }
        return release_retained_heap_debounced(reason, min_interval);
    };

    // Ratchet the baseline down so growth is measured from the true floor
    // (e.g. after freed pages get reused into live memory).
    let mut baseline = POST_TRIM_APPARENT_RETENTION.load(Ordering::Relaxed);
    if apparent < baseline {
        POST_TRIM_APPARENT_RETENTION.store(apparent, Ordering::Relaxed);
        baseline = apparent;
    }

    if !retention_growth_exceeds(apparent, baseline, threshold_bytes) {
        return false;
    }

    let released = release_retained_heap_debounced(reason, min_interval);
    if released {
        let after = apparent_heap_retention_bytes().unwrap_or(apparent);
        logging::info(&format!(
            "retained-heap trim ({reason}): apparent retention {} MB grew {} MB above post-trim baseline {} MB (threshold {} MB); recovered ~{} MB",
            apparent / (1024 * 1024),
            (apparent - baseline) / (1024 * 1024),
            baseline / (1024 * 1024),
            threshold_bytes / (1024 * 1024),
            apparent.saturating_sub(after) / (1024 * 1024),
        ));
    }
    released
}

/// Retention trim threshold in bytes, from `JCODE_HEAP_RETENTION_TRIM_MB`
/// (in MiB), falling back to [`DEFAULT_RETENTION_TRIM_THRESHOLD_BYTES`].
/// `0` disables retention-triggered trimming (returns `u64::MAX`).
pub fn retention_trim_threshold_bytes() -> u64 {
    parse_retention_trim_threshold(
        std::env::var("JCODE_HEAP_RETENTION_TRIM_MB")
            .ok()
            .as_deref(),
    )
}

fn parse_retention_trim_threshold(value: Option<&str>) -> u64 {
    match value.and_then(|value| value.trim().parse::<u64>().ok()) {
        Some(0) => u64::MAX,
        Some(mb) => mb.saturating_mul(1024 * 1024),
        None => DEFAULT_RETENTION_TRIM_THRESHOLD_BYTES,
    }
}

fn record_snapshot(source: String, snapshot: ProcessMemorySnapshot) {
    let Ok(mut history) = memory_history().lock() else {
        logging::error("process memory history lock poisoned; dropping snapshot");
        return;
    };
    if history.len() >= MAX_HISTORY_SAMPLES {
        logging::debug("process memory history full; dropping oldest snapshot");
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
        pss_anon_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Pss_Anon:")),
        pss_file_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Pss_File:")),
        pss_shmem_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "Pss_Shmem:")),
        anon_huge_pages_bytes: smaps
            .as_deref()
            .and_then(|text| parse_proc_value_bytes(text, "AnonHugePages:")),
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
fn jemalloc_tuning_info() -> Option<AllocatorTuningInfo> {
    let arena_count = tikv_jemalloc_ctl::arenas::narenas::read().ok()?;
    let mut initialized_arenas = 0u64;
    for arena_idx in 0..arena_count {
        if jemalloc_read_dynamic::<bool>(&format!("arena.{arena_idx}.initialized")).unwrap_or(false)
        {
            initialized_arenas += 1;
        }
    }

    Some(AllocatorTuningInfo {
        available: true,
        background_thread: tikv_jemalloc_ctl::background_thread::read().ok(),
        max_background_threads: tikv_jemalloc_ctl::max_background_threads::read()
            .ok()
            .map(|value| value as u64),
        arena_count: Some(arena_count as u64),
        initialized_arenas: Some(initialized_arenas),
        dirty_decay_ms: unsafe {
            tikv_jemalloc_ctl::raw::read::<isize>(b"arenas.dirty_decay_ms\0")
        }
        .ok()
        .map(|value| value as i64),
        muzzy_decay_ms: unsafe {
            tikv_jemalloc_ctl::raw::read::<isize>(b"arenas.muzzy_decay_ms\0")
        }
        .ok()
        .map(|value| value as i64),
        retain: unsafe { tikv_jemalloc_ctl::raw::read::<bool>(b"opt.retain\0") }.ok(),
        tcache_enabled: unsafe { tikv_jemalloc_ctl::raw::read::<bool>(b"opt.tcache\0") }.ok(),
        tcache_max_bytes: unsafe { tikv_jemalloc_ctl::raw::read::<usize>(b"arenas.tcache_max\0") }
            .ok()
            .map(|value| value as u64),
    })
}

#[cfg(feature = "jemalloc")]
fn jemalloc_read_dynamic<T: Copy>(name: &str) -> Result<T> {
    let c_name = CString::new(name).map_err(|_| anyhow!("mallctl name contains NUL byte"))?;
    unsafe {
        tikv_jemalloc_ctl::raw::read(c_name.as_bytes_with_nul())
            .map_err(|e| anyhow!("failed to read jemalloc mallctl {}: {}", name, e))
    }
}

#[cfg(feature = "jemalloc")]
fn jemalloc_write_dynamic<T>(name: &str, value: T) -> Result<()> {
    let c_name = CString::new(name).map_err(|_| anyhow!("mallctl name contains NUL byte"))?;
    unsafe {
        tikv_jemalloc_ctl::raw::write(c_name.as_bytes_with_nul(), value)
            .map_err(|e| anyhow!("failed to update jemalloc mallctl {}: {}", name, e))
    }
}

#[cfg(feature = "jemalloc")]
fn jemalloc_void_ctl(name: &str) -> Result<()> {
    let c_name = CString::new(name).map_err(|_| anyhow!("mallctl name contains NUL byte"))?;
    unsafe {
        let err = tikv_jemalloc_sys::mallctl(
            c_name.as_ptr() as *const c_char,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        );
        if err != 0 {
            return Err(anyhow!(
                "failed to invoke jemalloc mallctl {}: {}",
                name,
                err
            ));
        }
    }
    Ok(())
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

/// Parse a unit-less `/proc` counter such as `Threads:\t10`.
#[cfg(target_os = "linux")]
fn parse_proc_status_count(status: &str, key: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(key)?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
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
    fn estimate_json_bytes_matches_serialized_length_without_buffering_output() {
        let value = serde_json::json!({
            "escaped": "line one\nline two\t\"quoted\"",
            "unicode": "RAM profile 🦊",
            "nested": [null, true, -42, {"payload": "x".repeat(64 * 1024)}],
        });

        assert_eq!(
            estimate_json_bytes(&value),
            serde_json::to_vec(&value)
                .expect("serialize reference value")
                .len()
        );
    }

    #[test]
    fn estimate_json_bytes_returns_zero_when_serialization_fails() {
        struct FailingSerialize;

        impl serde::Serialize for FailingSerialize {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("intentional failure"))
            }
        }

        assert_eq!(estimate_json_bytes(&FailingSerialize), 0);
    }

    #[test]
    fn release_retained_heap_is_safe_to_call() {
        // Allocate and drop a large transient buffer, then release. This must
        // not crash on any allocator configuration.
        let buffer = vec![0u8; 8 * 1024 * 1024];
        drop(buffer);
        release_retained_heap("unit_test");
    }

    #[test]
    fn release_retained_heap_debounced_skips_within_interval() {
        // First call resets the shared debounce clock; the immediate second
        // call within a long interval must be skipped.
        release_retained_heap_debounced("unit_test_first", std::time::Duration::ZERO);
        let ran = release_retained_heap_debounced(
            "unit_test_second",
            std::time::Duration::from_secs(3600),
        );
        assert!(
            !ran,
            "second call within debounce interval should be skipped"
        );
    }

    #[test]
    fn parse_retention_trim_threshold_handles_default_disable_and_values() {
        assert_eq!(
            parse_retention_trim_threshold(None),
            DEFAULT_RETENTION_TRIM_THRESHOLD_BYTES
        );
        assert_eq!(
            parse_retention_trim_threshold(Some("garbage")),
            DEFAULT_RETENTION_TRIM_THRESHOLD_BYTES
        );
        // 0 disables retention trimming entirely.
        assert_eq!(parse_retention_trim_threshold(Some("0")), u64::MAX);
        assert_eq!(
            parse_retention_trim_threshold(Some(" 128 ")),
            128 * 1024 * 1024
        );
    }

    #[test]
    fn release_retained_heap_if_excessive_skips_below_threshold() {
        // u64::MAX growth threshold can never be exceeded, so no release
        // should run regardless of current allocator state.
        let ran = release_retained_heap_if_excessive(
            "unit_test_below_threshold",
            u64::MAX,
            std::time::Duration::ZERO,
        );
        assert!(!ran, "release should not run below threshold");
    }

    #[test]
    fn retention_growth_trigger_measures_growth_above_baseline() {
        let mb = 1024 * 1024;
        // At or below baseline: no growth.
        assert!(!retention_growth_exceeds(100 * mb, 100 * mb, 64 * mb));
        assert!(!retention_growth_exceeds(50 * mb, 100 * mb, 64 * mb));
        // Growth below threshold stays quiet (the post-trim residual case).
        assert!(!retention_growth_exceeds(163 * mb, 100 * mb, 64 * mb));
        // Growth at/above threshold fires.
        assert!(retention_growth_exceeds(164 * mb, 100 * mb, 64 * mb));
        assert!(retention_growth_exceeds(300 * mb, 100 * mb, 64 * mb));
        // Threshold 0 always fires.
        assert!(retention_growth_exceeds(0, 0, 0));
    }

    #[cfg(all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc")))]
    #[test]
    fn release_retained_heap_if_excessive_runs_above_threshold_then_requires_regrowth() {
        // Threshold 0 means any growth (>= 0) triggers; with a zero debounce
        // the release must run and reset the baseline to the current level.
        let ran = release_retained_heap_if_excessive(
            "unit_test_above_threshold",
            0,
            std::time::Duration::ZERO,
        );
        assert!(ran, "release should run when growth exceeds threshold");

        // Immediately after the trim the baseline equals current apparent
        // retention, so a huge growth threshold cannot be met: steady state
        // must not re-trigger.
        let ran_again = release_retained_heap_if_excessive(
            "unit_test_steady_state",
            u64::MAX,
            std::time::Duration::ZERO,
        );
        assert!(
            !ran_again,
            "steady-state retention must not re-trigger the watchdog"
        );
    }

    #[test]
    fn allocator_info_matches_enabled_allocator_features() {
        let info = allocator_info();
        if cfg!(feature = "jemalloc") {
            assert_eq!(info.name, "jemalloc");
            assert_eq!(info.stats_available, info.stats.is_some());
            assert!(info.profiling.is_some());
        } else {
            assert_eq!(info.name, "system");
            assert_eq!(info.stats_available, info.stats.is_some());
            assert!(info.profiling.is_none());
        }
    }

    #[cfg(all(target_os = "linux", target_env = "gnu", not(feature = "jemalloc")))]
    #[test]
    fn glibc_malloc_stats_report_live_and_retained_bytes() {
        // Hold a live allocation so uordblks cannot be zero, then check the
        // mallinfo2-backed stats are populated and internally consistent.
        let held = vec![0u8; 1024 * 1024];
        let stats = glibc_malloc_stats().expect("mallinfo2 stats on glibc");
        assert!(
            stats.allocated_bytes.unwrap() > 0,
            "live bytes should be nonzero"
        );
        assert!(stats.retained_bytes.is_some());
        assert!(
            stats.mapped_bytes.unwrap() >= stats.active_bytes.unwrap(),
            "arena total should cover in-use arena bytes"
        );
        drop(held);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_proc_status_count_reads_unitless_counters() {
        let text = "Name:\tjcode\nThreads:\t10\nVmStk:\t     132 kB\n";
        assert_eq!(parse_proc_status_count(text, "Threads:"), Some(10));
        assert_eq!(parse_proc_status_count(text, "Missing:"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn snapshot_populates_thread_and_stack_and_pss_split_fields() {
        let snapshot = snapshot_with_source("unit_test_coverage_fields");
        assert!(
            snapshot.thread_count.unwrap_or(0) >= 1,
            "a live process has at least one thread"
        );
        assert!(
            snapshot.main_stack_bytes.unwrap_or(0) > 0,
            "main stack should be nonzero"
        );
        let os = snapshot.os.expect("linux os info");
        // smaps_rollup reports Pss_Anon/Pss_File on kernels >= 4.14; both
        // should be present and their sum should not exceed total PSS by more
        // than rounding.
        let pss = os.pss_bytes.expect("pss");
        let anon = os.pss_anon_bytes.expect("pss_anon");
        let file = os.pss_file_bytes.expect("pss_file");
        assert!(
            anon + file <= pss + 2 * 1024 * 1024,
            "pss split should be consistent"
        );
    }
}
