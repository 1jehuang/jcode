use super::*;
use std::io::Read;

/// Tests that touch the process-wide uplink estimate must not interleave.
static UPLINK_TEST_LOCK: Mutex<()> = Mutex::new(());

fn jsonish(len: usize) -> Vec<u8> {
    let unit = br#"{"role":"user","content":[{"type":"text","text":"fn main() { println!(\"hello\"); }"}]},"#;
    unit.iter().copied().cycle().take(len).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn json_uploads_match_original_wire_bytes_and_headers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _guard = UPLINK_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut state = 0x12345678u32;
    let varied: String = (0..1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            b"0123456789abcdef"[(state >> 28) as usize] as char
        })
        .collect();
    for payload in ["x".repeat(1024 * 1024), varied] {
        let body = serde_json::json!({"messages": [{"role": "user", "content": payload}]});
        let raw = serde_json::to_vec(&body).unwrap();
        for (mode, endpoint) in [
            (
                CompressionMode::Off,
                "https://api.anthropic.com/v1/messages",
            ),
            (
                CompressionMode::Always,
                "https://api.anthropic.com/v1/messages",
            ),
            (
                CompressionMode::Always,
                "https://api.openai.com/v1/responses",
            ),
        ] {
            let encoding = endpoint_encoding(endpoint);
            let (_, compressed) = decide(&raw, mode, encoding, DEFAULT_UPLINK_BPS);
            let expected_encoding = compressed
                .as_ref()
                .map(|_| encoding.unwrap().header_value());
            let expected = compressed.unwrap_or_else(|| raw.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();
                let mut buffer = [0u8; 8192];
                let header_end = loop {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(n, 0, "upload ended before headers");
                    received.extend_from_slice(&buffer[..n]);
                    if let Some(index) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8(received[..header_end].to_vec())
                    .unwrap()
                    .to_ascii_lowercase();
                assert!(headers.contains("content-type: application/json\r\n"));
                assert!(!headers.contains("transfer-encoding:"));
                assert!(headers.contains(&format!("content-length: {}\r\n", expected.len())));
                if let Some(encoding) = expected_encoding {
                    assert!(headers.contains(&format!("content-encoding: {encoding}\r\n")));
                } else {
                    assert!(!headers.contains("content-encoding:"));
                }
                while received.len() - header_end < expected.len() {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(n, 0, "upload ended before body");
                    received.extend_from_slice(&buffer[..n]);
                }
                assert_eq!(&received[header_end..], expected);
                socket.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let request = json_body(
                client.post(format!("http://{address}/upload")),
                endpoint,
                &body,
                mode,
            )
            .unwrap();
            let response = tokio::time::timeout(Duration::from_secs(10), request.send())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
            server.await.unwrap();
            assert_eq!(
                serde_json::to_vec(&body).unwrap(),
                raw,
                "retry input changed"
            );
        }
    }
    reset_uplink_for_tests(None);
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
        decide(&big, CompressionMode::Off, Some(BodyEncoding::Gzip), 1e6).0,
        Decision::Raw("disabled")
    );
    assert_eq!(
        decide(&big, CompressionMode::Auto, None, 1e6).0,
        Decision::Raw("endpoint")
    );
    assert_eq!(
        decide(
            &jsonish(8_000),
            CompressionMode::Always,
            Some(BodyEncoding::Gzip),
            1e6
        )
        .0,
        Decision::Raw("small")
    );
    assert_eq!(
        decide(&big, CompressionMode::Auto, Some(BodyEncoding::Gzip), 2e9).0,
        Decision::Raw("fast-uplink")
    );
    let (decision, bytes) = decide(&big, CompressionMode::Auto, Some(BodyEncoding::Gzip), 10e6);
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
        decide(
            &noise,
            CompressionMode::Auto,
            Some(BodyEncoding::Gzip),
            10e6
        )
        .0,
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

#[test]
fn endpoint_encodings_match_live_probes() {
    assert_eq!(
        endpoint_encoding("https://api.openai.com/v1/responses"),
        Some(BodyEncoding::Zstd)
    );
    assert_eq!(
        endpoint_encoding("https://api.anthropic.com/v1/messages?beta=true"),
        Some(BodyEncoding::Gzip)
    );
    assert_eq!(
        endpoint_encoding("https://chatgpt.com/backend-api/codex/responses"),
        Some(BodyEncoding::Gzip)
    );
    // chat/completions rejected both gzip and zstd live.
    assert_eq!(
        endpoint_encoding("https://api.openai.com/v1/chat/completions"),
        None
    );
    assert_eq!(
        endpoint_encoding("https://api.openai.com.evil.example/v1/responses"),
        None
    );
}

#[test]
fn zstd_roundtrips_and_beats_gzip_ratio() {
    // Varied, conversation-like JSON: realistic 2-6x ratio, far below the cap.
    let mut x = 0x2545F4914F6CDD1Du64;
    let mut big = String::from("[");
    while big.len() < 400_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        big.push_str(&format!(
            r#"{{"type":"function_call_output","call_id":"call_{x:016x}","output":"line {} of file_{}.rs: let v{} = {};"}},"#,
            x % 977,
            x % 53,
            x % 31,
            x % 100_003
        ));
    }
    let big = big.into_bytes();
    let (decision, bytes) = decide(
        &big,
        CompressionMode::Always,
        Some(BodyEncoding::Zstd),
        10e6,
    );
    let Decision::Zstd { compressed_len } = decision else {
        panic!("{decision:?}");
    };
    let bytes = bytes.unwrap();
    assert_eq!(bytes.len(), compressed_len);
    assert!(big.len() / compressed_len < MAX_ZSTD_EXPANSION);
    assert_eq!(zstd::bulk::decompress(&bytes, big.len()).unwrap(), big);
    let gzip_len = gzip_fast(&big).unwrap().len();
    assert!(
        compressed_len < gzip_len,
        "zstd-1 {compressed_len} vs gzip-1 {gzip_len}"
    );
}

#[test]
fn zstd_respects_openai_expansion_limit() {
    // Pure repetition compresses far past OpenAI's ~100x expansion limit
    // (live probe: 82x accepted, 142x rejected with HTTP 413).
    let padding = vec![b'a'; 2_000_000];
    assert_eq!(
        decide(
            &padding,
            CompressionMode::Always,
            Some(BodyEncoding::Zstd),
            1e6
        )
        .0,
        Decision::Raw("expansion-limit")
    );
    // gzip endpoints have no such limit.
    assert!(matches!(
        decide(
            &padding,
            CompressionMode::Always,
            Some(BodyEncoding::Gzip),
            1e6
        )
        .0,
        Decision::Gzip { .. }
    ));
}
