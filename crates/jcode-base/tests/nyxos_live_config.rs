//! Guards the nyxos relay latency mitigation applied 2026-09-23.
//!
//! The nyxos relay (api.nyxos.shop) has a bimodal latency profile: healthy
//! channels answer in 2-8s, but a stalled channel never recovers (>200s
//! observed). The fix is to fail fast and retry, and to pin the slow reasoning
//! models to `reasoning_effort=low` (the relay ignores `thinking=disabled`).
//!
//! Measured before/after (streamed, agent-sized prompt, n=6 each, 6/6 success
//! both ways): glm-5.2 p50 24.5s -> 6.8s, kimi-k3 p50 34.4s -> 7.6s.
//!
//! This test reads the live user config, so it is skipped when that config or
//! the nyxos profile is absent (CI, other machines).

use jcode_base::config::Config;

#[test]
fn nyxos_latency_mitigation_is_configured() {
    let Some(path) = Config::path() else {
        eprintln!("skip: no config path");
        return;
    };
    if !path.exists() {
        eprintln!("skip: no live config at {}", path.display());
        return;
    }

    // Authoritative check: the real loader must accept the file. A malformed
    // config silently degrades to defaults in normal runs, which would hide a
    // broken edit.
    let cfg = Config::load_strict().expect("live config.toml must parse");

    let Some(nyx) = cfg.providers.get("opencode-nyxos") else {
        eprintln!("skip: no opencode-nyxos profile on this machine");
        return;
    };

    // The relay only honors top-level `reasoning_effort`, so the profile must
    // be allowed to send it.
    assert_eq!(
        nyx.supports_reasoning_effort,
        Some(true),
        "nyxos must send top-level reasoning_effort; the relay ignores thinking=disabled"
    );

    // The GLM/Kimi reasoning models burn hidden reasoning tokens by default.
    for id in ["glm-5.2", "glm-5.3-flash", "kimi-k3"] {
        let model = nyx
            .models
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("nyxos model {id} should be configured"));
        assert_eq!(
            model.reasoning_effort.as_deref(),
            Some("low"),
            "{id} must pin reasoning_effort=low to avoid multi-minute silent reasoning"
        );
    }

    // Fail-fast budget. Must stay well under the >200s stall so a retry can
    // land on a healthy channel, but above legitimate high-effort thinking
    // (effort multipliers scale this up further: high=2x, xhigh=3x, max=4x).
    let idle = cfg.provider.stream_idle_timeout_secs;
    assert!(
        (45..=120).contains(&idle),
        "stream_idle_timeout_secs={idle} should be 45..=120 so stalled channels are retried, not waited out"
    );

    // Retries are what actually recover latency; a stalled channel never heals.
    assert!(
        cfg.provider.max_retries >= 3,
        "max_retries={} too low to escape a stalled relay channel",
        cfg.provider.max_retries
    );

    // Backoff must not reintroduce the latency the short timeout removed.
    assert!(
        cfg.provider.retry_backoff_cap_secs <= 15,
        "retry_backoff_cap_secs={} would negate the fail-fast timeout",
        cfg.provider.retry_backoff_cap_secs
    );
}
