use super::*;
use std::io::Read;

/// Tests that touch the process-wide uplink estimate must not interleave.
static UPLINK_TEST_LOCK: Mutex<()> = Mutex::new(());

fn jsonish(len: usize) -> Vec<u8> {
    let unit = br#"{"role":"user","content":[{"type":"text","text":"fn main() { println!(\"hello\"); }"}]},"#;
    unit.iter().copied().cycle().take(len).collect()
}

#[test]
fn parses_modes() {
    assert_eq!(CompressionMode::parse("off"), Some(CompressionMode::Off));
    assert_eq!(
        CompressionMode::parse(" Auto "),
        Some(CompressionMode::Auto)
    );
    assert_eq!(
        CompressionMode::parse("always"),
        Some(CompressionMode::Always)
    );
    assert_eq!(CompressionMode::parse("bogus"), None);
}

#[test]
fn only_verified_endpoints_are_eligible() {
    assert!(endpoint_accepts_gzip(
        "https://api.anthropic.com/v1/messages"
    ));
    assert!(endpoint_accepts_gzip(
        "https://api.anthropic.com/v1/messages?beta=true"
    ));
    assert!(endpoint_accepts_gzip(
        "https://chatgpt.com/backend-api/codex/responses"
    ));
    assert!(!endpoint_accepts_gzip(
        "https://api.openai.com/v1/responses"
    ));
    assert!(!endpoint_accepts_gzip(
        "https://openrouter.ai/api/v1/chat/completions"
    ));
    assert!(!endpoint_accepts_gzip(
        "https://api.jcode.sh/v1/chat/completions"
    ));
    assert!(!endpoint_accepts_gzip(
        "http://api.anthropic.com/v1/messages"
    ));
    assert!(!endpoint_accepts_gzip(
        "https://api.anthropic.com.evil.example/v1/messages"
    ));
    assert!(!endpoint_accepts_gzip(
        "https://proxy.example/anthropic/v1/messages"
    ));
}

#[test]
fn break_even_matches_measurements() {
    // 270 KB at 2.6x on a 10 Mbit uplink saved ~170 ms in shaped benchmarks.
    let s = predicted_saving_secs(270_000, 104_000, 10e6);
    assert!((0.10..0.20).contains(&s), "{s}");
    // 16 KB on a fast link is noise.
    assert!(predicted_saving_secs(16_000, 7_000, 100e6) < MIN_SAVING.as_secs_f64());
    // On a gigabit uplink codec cost dominates.
    assert!(predicted_saving_secs(900_000, 450_000, 1e9) < 0.0);
}

#[test]
fn decide_respects_mode_size_and_endpoint() {
    let big = jsonish(400_000);
    assert_eq!(
        decide(&big, CompressionMode::Off, true, 1e6).0,
        Decision::Raw("disabled")
    );
    assert_eq!(
        decide(&big, CompressionMode::Auto, false, 1e6).0,
        Decision::Raw("endpoint")
    );
    assert_eq!(
        decide(&jsonish(8_000), CompressionMode::Always, true, 1e6).0,
        Decision::Raw("small")
    );
    assert_eq!(
        decide(&big, CompressionMode::Auto, true, 2e9).0,
        Decision::Raw("fast-uplink")
    );
    let (decision, bytes) = decide(&big, CompressionMode::Auto, true, 10e6);
    assert!(matches!(decision, Decision::Gzip { .. }), "{decision:?}");
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes.unwrap().as_slice())
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(out, big);
}

#[test]
fn incompressible_body_stays_raw_in_auto() {
    // xorshift noise does not compress.
    let mut x = 0x9E3779B97F4A7C15u64;
    let noise: Vec<u8> = (0..200_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    assert_eq!(
        decide(&noise, CompressionMode::Auto, true, 10e6).0,
        Decision::Raw("low-ratio")
    );
}

#[test]
fn sampler_records_rate_after_skip_window() {
    let _guard = UPLINK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_uplink_for_tests(None);
    let total = 512 * 1024;
    let mut sampler = UploadSampler::new(total);
    let mut start = 0;
    while start < total {
        let end = (start + SAMPLE_CHUNK_BYTES).min(total);
        sampler.on_pull(start, end);
        if start >= SAMPLE_SKIP_BYTES {
            std::thread::sleep(Duration::from_millis(2));
        }
        start = end;
    }
    let bps = uplink_estimate_bps().expect("sample recorded");
    // ~32 KB per 2+ ms is at most ~130 Mbit/s.
    assert!(bps > 1e6 && bps < 140e6, "{bps}");
    reset_uplink_for_tests(None);
}

#[test]
fn ewma_moves_toward_new_samples() {
    let _guard = UPLINK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_uplink_for_tests(Some(100e6));
    record_uplink_sample(10e6);
    let est = uplink_estimate_bps().unwrap();
    assert!(est < 100e6 && est > 10e6);
    reset_uplink_for_tests(None);
}
