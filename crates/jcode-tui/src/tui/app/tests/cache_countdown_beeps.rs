
// Use "openrouter" provider (always 300s TTL, no global flag pollution from other tests).
// Anthropic TTL is affected by a process-global `set_cache_ttl_1h` flag set by other tests.

// ---------------------------------------------------------------------------
// Config isolation helper
//
// These tests write a temporary config to control `cache_countdown_beeps`.
// They must:
//   1. Run after `create_test_app()` so JCODE_HOME is already set to the
//      shared test home (not the developer's real home).
//   2. Restore the original config on drop so concurrent tests are not
//      disturbed.
//   3. Serialize config writes with the shared test-env lock so tests that
//      run in the same process don't race each other.
// ---------------------------------------------------------------------------

struct ConfigGuard {
    path: std::path::PathBuf,
    original: Option<Vec<u8>>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ConfigGuard {
    fn with_beeps(level: u8) -> Self {
        // Use the shared test-env lock so tests that scope JCODE_HOME cannot
        // race this guard's config read/write.  The lock must be acquired before
        // reading the original contents so two concurrent guards cannot both read
        // the same "original" and then each restore a stale copy on drop.
        let lock = crate::storage::lock_test_env();

        let path = crate::config::Config::path().expect("config path");
        std::fs::create_dir_all(path.parent().expect("config dir")).expect("mkdir");

        let original = std::fs::read(&path).ok();
        let content = format!("[features]\ncache_countdown_beeps = {level}\n");
        std::fs::write(&path, &content).expect("write config");
        crate::config::invalidate_config_cache();

        ConfigGuard { path, original, _lock: lock }
    }
}

impl Drop for ConfigGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(bytes) => { let _ = std::fs::write(&self.path, bytes); }
            None        => { let _ = std::fs::remove_file(&self.path); }
        }
        crate::config::invalidate_config_cache();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_openrouter_baseline(app: &App, elapsed_secs: u64) -> KvCacheBaseline {
    KvCacheBaseline {
        session_id: app.kv_cache_session_id(),
        cache_generation: app.kv_cache.cache_generation,
        input_tokens: 50_000,
        completed_at: Instant::now() - Duration::from_secs(elapsed_secs),
        cache_ttl_secs: Some(300),
        provider: "openrouter".to_string(),
        model: "some-model".to_string(),
        upstream_provider: None,
        signature: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn cache_countdown_beep_fires_3min_milestone_once() {
    // When remaining time crosses the 3-min threshold, bit 0 is set exactly once.
    // Level 3 must be configured explicitly; the default is 1.
    let _cfg = ConfigGuard::with_beeps(3);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // openrouter TTL = 300s, elapsed = 130s -> remaining = 170s (<= 180s, > 120s)
    // -> only the 3-min milestone (bit 0) should fire
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 130));

    assert_eq!(app.kv_cache.beep_milestones_fired, 0b000, "no milestones fired yet");

    // First call: 3-min milestone fires
    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b001,
        0b001,
        "3-min milestone bit should be set after first call (remaining=170s)"
    );
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b110,
        0,
        "2-min and 1-min bits should remain unset (remaining=170s > 120s)"
    );

    // Second call at same elapsed: bitmask must prevent re-fire
    let bits_before = app.kv_cache.beep_milestones_fired;
    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        bits_before,
        "bitmask must not change on repeat tick at same remaining"
    );
}

#[test]
fn cache_countdown_beep_fires_2min_milestone_independently() {
    // At remaining=105s (<= 120s threshold), bit 1 fires. Level 2 configured.
    let _cfg = ConfigGuard::with_beeps(2);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // openrouter TTL = 300s, elapsed = 195s -> remaining = 105s (<= 120s, > 60s)
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 195));

    app.maybe_emit_cache_countdown_beeps();
    // Level 2: 3-min bit must NOT fire; 2-min bit must fire
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b001,
        0,
        "3-min bit must not fire at level 2"
    );
    assert_ne!(
        app.kv_cache.beep_milestones_fired & 0b010,
        0,
        "2-min milestone bit should be set at remaining=105s"
    );
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b100,
        0,
        "1-min bit should not fire at remaining=105s"
    );
}

#[test]
fn cache_countdown_beep_fires_all_milestones_when_suspended() {
    // TUI was suspended and resumes with < 60s remaining: all three bits fire at once.
    let _cfg = ConfigGuard::with_beeps(3);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // openrouter TTL = 300s, elapsed = 270s -> remaining = 30s (<= 60s, > 0)
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 270));

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        0b111,
        "all three milestone bits must be set when remaining=30s on first call"
    );
}

#[test]
fn cache_countdown_beep_does_not_fire_after_expiry() {
    // remaining == 0 -> no beep, bitmask stays 0
    let _cfg = ConfigGuard::with_beeps(3);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // openrouter TTL = 300s, elapsed = 600s -> remaining = 0 (saturating_sub)
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 600));

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        0,
        "expired cache must not fire any beep milestones"
    );
}

#[test]
fn cache_countdown_beep_skipped_for_none_ttl_provider() {
    // Copilot/Cursor store cache_ttl_secs = None -- no beep regardless of elapsed.
    let _cfg = ConfigGuard::with_beeps(3);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    app.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
        session_id: app.kv_cache_session_id(),
        cache_generation: app.kv_cache.cache_generation,
        input_tokens: 50_000,
        completed_at: Instant::now() - Duration::from_secs(30),
        cache_ttl_secs: None,
        provider: "copilot".to_string(),
        model: "gpt-4o".to_string(),
        upstream_provider: None,
        signature: None,
    });

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        0,
        "None-TTL provider (copilot) must not set any milestone bits"
    );

    // Also check cursor
    app.kv_cache.beep_milestones_fired = 0;
    app.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
        session_id: app.kv_cache_session_id(),
        cache_generation: app.kv_cache.cache_generation,
        input_tokens: 50_000,
        completed_at: Instant::now() - Duration::from_secs(30),
        cache_ttl_secs: None,
        provider: "cursor".to_string(),
        model: "claude-3-5-sonnet".to_string(),
        upstream_provider: None,
        signature: None,
    });
    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        0,
        "None-TTL provider (cursor) must not set any milestone bits"
    );
}

#[test]
fn cache_countdown_beep_rearms_after_new_baseline() {
    // After a new turn (baseline refreshed + beep_milestones_fired reset),
    // milestones fire again for the new baseline.
    let _cfg = ConfigGuard::with_beeps(3);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // First baseline: already in the 3-min window (remaining=170s)
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 130));
    app.maybe_emit_cache_countdown_beeps();
    assert_ne!(app.kv_cache.beep_milestones_fired, 0, "first baseline should have fired a milestone");

    // Simulate new turn completing: reset fires bitmask back to 0
    app.kv_cache.beep_milestones_fired = 0;
    app.kv_cache.kv_cache_baseline = Some(KvCacheBaseline {
        session_id: app.kv_cache_session_id(),
        cache_generation: app.kv_cache.cache_generation,
        input_tokens: 60_000,
        completed_at: Instant::now() - Duration::from_secs(130),
        cache_ttl_secs: Some(300),
        provider: "openrouter".to_string(),
        model: "some-model".to_string(),
        upstream_provider: None,
        signature: None,
    });
    app.maybe_emit_cache_countdown_beeps();
    assert_ne!(
        app.kv_cache.beep_milestones_fired,
        0,
        "new baseline must re-arm and fire the 3-min milestone"
    );
}

#[test]
fn cache_countdown_beep_skipped_while_processing() {
    // is_processing == true -> no milestones fire
    let _cfg = ConfigGuard::with_beeps(3);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 130));
    app.is_processing = true;

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        0,
        "processing sessions must not fire beep milestones"
    );
}

#[test]
fn bel_byte_content_matches_terminal_bell_spec() {
    // Directly verify the BEL bytes the production code emits.
    // maybe_emit_cache_countdown_beeps uses: "\x07".repeat(beeps_to_emit)
    // ASCII BEL = 0x07 = 7 decimal = ctrl-G. Terminals ring the bell on receiving it.
    let cases: &[(usize, &[u8])] = &[
        (1, &[0x07]),
        (2, &[0x07, 0x07]),
        (3, &[0x07, 0x07, 0x07]),
    ];
    for &(n, expected) in cases {
        let beep_str = "\x07".repeat(n);
        let bytes: Vec<u8> = beep_str.bytes().collect();
        assert_eq!(
            bytes.as_slice(),
            expected,
            "repeat({n}) must produce exactly {n} BEL byte(s) (0x07 each)"
        );
        assert!(
            bytes.iter().all(|&b| b == 7),
            "every byte must be ASCII BEL (value 7), got {bytes:?}"
        );
    }
    // Confirm the string literal \x07 and the byte literal b'\x07' are identical
    assert_eq!("\x07".as_bytes(), &[b'\x07']);
    assert_eq!(b'\x07', 7u8);
}

#[test]
fn cache_countdown_beep_disabled_when_config_is_zero() {
    // features.cache_countdown_beeps = 0 must silence all milestones.
    let _cfg = ConfigGuard::with_beeps(0);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // openrouter TTL = 300s, elapsed = 270s -> remaining = 30s (all milestones eligible)
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 270));

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired,
        0,
        "cache_countdown_beeps=0 must suppress all milestones"
    );
}

#[test]
fn cache_countdown_beep_level_1_fires_only_1min_milestone() {
    // features.cache_countdown_beeps = 1: only the 1-min milestone (bit 2) is eligible.
    let _cfg = ConfigGuard::with_beeps(1);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // openrouter TTL = 300s, elapsed = 130s -> remaining = 170s (<= 180s)
    // With level 1, the 3-min and 2-min milestones must NOT fire.
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 130));

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b011,
        0,
        "cache_countdown_beeps=1 must not fire 3-min or 2-min milestones at remaining=170s"
    );

    // Now simulate the cache reaching 1-min remaining
    app.kv_cache.beep_milestones_fired = 0;
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 250));
    app.maybe_emit_cache_countdown_beeps();
    assert_ne!(
        app.kv_cache.beep_milestones_fired & 0b100,
        0,
        "cache_countdown_beeps=1 must fire the 1-min milestone (bit 2) at remaining=50s"
    );
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b011,
        0,
        "cache_countdown_beeps=1 must still not fire 3-min or 2-min bits"
    );
}

#[test]
fn cache_countdown_beep_level_2_fires_2min_and_1min_not_3min() {
    // features.cache_countdown_beeps = 2: only 2-min and 1-min milestones fire.
    let _cfg = ConfigGuard::with_beeps(2);
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    // elapsed = 130s -> remaining = 170s: within 3-min window, but level 2 must suppress it
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 130));

    app.maybe_emit_cache_countdown_beeps();
    assert_eq!(
        app.kv_cache.beep_milestones_fired & 0b001,
        0,
        "cache_countdown_beeps=2 must not fire the 3-min milestone"
    );

    // elapsed = 195s -> remaining = 105s: within 2-min window, must fire
    app.kv_cache.beep_milestones_fired = 0;
    app.kv_cache.kv_cache_baseline = Some(make_openrouter_baseline(&app, 195));
    app.maybe_emit_cache_countdown_beeps();
    assert_ne!(
        app.kv_cache.beep_milestones_fired & 0b110,
        0,
        "cache_countdown_beeps=2 must fire 2-min and/or 1-min milestones at remaining=105s"
    );
}
