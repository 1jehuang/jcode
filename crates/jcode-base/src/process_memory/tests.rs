use super::*;

#[test]
fn windows_memory_metrics_keep_private_commit_separate() {
    let snapshot = windows_memory_snapshot(4096, 8192, 16384);
    assert_eq!(snapshot.rss_bytes, Some(4096));
    assert_eq!(snapshot.peak_rss_bytes, Some(8192));
    assert_eq!(snapshot.virtual_bytes, None);
    assert_eq!(snapshot.thread_count, None);
    assert_eq!(snapshot.main_stack_bytes, None);
    let os = snapshot.os.as_ref().expect("Windows memory details");
    assert_eq!(os.private_commit_bytes, Some(16384));
    assert_eq!(os.private_dirty_bytes, None);
    assert_eq!(os.pss_bytes, None);
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["os"]["private_commit_bytes"], 16384);
    assert!(json["virtual_bytes"].is_null());
}

#[cfg(windows)]
#[test]
fn windows_native_snapshot_reports_working_set_and_records_history() {
    let source = "windows_native_memory_smoke";
    let snapshot = snapshot_with_source(source);
    let rss = snapshot.rss_bytes.expect("current working set");
    assert!(rss > 0);
    assert!(snapshot.peak_rss_bytes.expect("peak working set") >= rss);
    assert!(snapshot.os.as_ref().unwrap().private_commit_bytes.unwrap() > 0);
    assert_eq!(snapshot.virtual_bytes, None);
    assert!(
        history(MAX_HISTORY_SAMPLES).iter().any(|entry| {
            entry.source == source && entry.snapshot.rss_bytes == snapshot.rss_bytes
        })
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_snapshot_preserves_resident_virtual_and_history_metrics() {
    let source = "linux_memory_regression";
    let snapshot = snapshot_with_source(source);
    assert!(snapshot.rss_bytes.expect("VmRSS") > 0);
    assert!(snapshot.peak_rss_bytes.expect("VmHWM") >= snapshot.rss_bytes.unwrap());
    assert!(snapshot.virtual_bytes.expect("VmSize") >= snapshot.rss_bytes.unwrap());
    let os = snapshot.os.as_ref().expect("Linux memory details");
    assert_eq!(os.private_commit_bytes, None);
    let json = serde_json::to_value(&snapshot).unwrap();
    assert!(json["os"].get("private_commit_bytes").is_none());
    assert!(history(MAX_HISTORY_SAMPLES).iter().any(|entry| {
        entry.source == source && entry.snapshot.virtual_bytes == snapshot.virtual_bytes
    }));
}

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
    let ran =
        release_retained_heap_debounced("unit_test_second", std::time::Duration::from_secs(3600));
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

/// Guards the musl build (issue #645). `mallopt`/`malloc_trim` are glibc
/// extensions, so every allocator path that calls them must be gated on
/// `target_env = "gnu"`, not on `target_os = "linux"` alone, and the
/// corresponding fallback must widen to match.
///
/// This asserts the *shape* of the gating rather than any runtime value:
/// exactly one of the glibc arm and the no-op arm may be active for a given
/// target. If someone re-widens the glibc gate to bare `target_os`, musl
/// selects both arms here and this fails; if a fallback is narrowed by
/// mistake, musl selects neither and this also fails.
#[test]
fn glibc_only_allocator_paths_are_gated_on_gnu_not_just_linux() {
    let glibc_arm_active = cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        not(feature = "jemalloc")
    ));
    let fallback_arm_active = cfg!(all(
        not(all(target_os = "linux", target_env = "gnu")),
        not(feature = "jemalloc")
    ));

    if cfg!(feature = "jemalloc") {
        // jemalloc supersedes both system-allocator arms.
        assert!(!glibc_arm_active);
        assert!(!fallback_arm_active);
    } else {
        assert_ne!(
            glibc_arm_active, fallback_arm_active,
            "exactly one system-allocator arm must be active: \
             glibc={glibc_arm_active} fallback={fallback_arm_active}"
        );
    }

    // On a musl target the glibc-only arm must never be the active one,
    // since the symbols it references do not exist there.
    if cfg!(all(target_os = "linux", target_env = "musl")) {
        assert!(
            !glibc_arm_active,
            "musl must not select the glibc allocator arm: it references \
             mallopt/malloc_trim, which musl does not provide (issue #645)"
        );
    }
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
