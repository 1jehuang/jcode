//! Adaptive request-body compression for LLM provider calls.
//!
//! Every turn re-sends the whole conversation, so request bodies routinely
//! reach hundreds of KB. On a slow uplink that upload sits directly on the
//! time-to-first-token path. gzip level 1 compresses session JSON 1.3-3x at
//! roughly 200 MB/s, so compression wins whenever the upload time saved exceeds
//! the encode cost plus the provider's decode cost.
//!
//! Whether it wins depends on the user's network, so the decision is dynamic:
//! large bodies are streamed in chunks and the rate at which hyper pulls them
//! (after the socket buffer fills) gives a passive estimate of upload
//! bandwidth. No probe traffic is ever sent.
//!
//! The pull-rate estimate is an upper bound: the kernel send buffer absorbs
//! some bytes at memory speed, and it autotunes larger on reused connections
//! (a shaped 8 Mbit link read as 8.7 Mbit on a fresh connection, drifting
//! toward ~20 Mbit). That bias only ever argues against compressing, and with
//! gzip-1 the break-even uplink for a 2x body is ~400 Mbit
//! (`saved_bits / bw == raw / CODEC_BYTES_PER_SEC`), so a 2-3x overestimate
//! changes decisions only on links that are already near gigabit.
//!
//! Measured break-even (Anthropic, shaped uplink, gzip-1):
//! - >= 50 Mbit/s: neutral within noise, even at 900 KB.
//! - 10 Mbit/s: +170 ms at 270 KB, +265 ms at 900 KB.
//! - 5 Mbit/s: +145 ms at 68 KB, +630 ms at 270 KB.
//! - Bodies under ~32 KB never benefit measurably.
//!
//! Only endpoints verified to accept `Content-Encoding: gzip` are eligible.
//! OpenAI's public API (`identity|zstd` only), OpenRouter, and the jcode router
//! reject gzip, and custom/proxy base URLs are never assumed to support it.

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// User-facing compression policy (`[provider] request_compression`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    /// Never compress.
    Off,
    /// Compress when the bandwidth model predicts a latency win.
    Auto,
    /// Compress every eligible body at or above the minimum size.
    Always,
}

impl CompressionMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "no" | "0" | "never" | "disabled" => Some(Self::Off),
            "auto" | "" | "adaptive" => Some(Self::Auto),
            "always" | "on" | "true" | "yes" | "1" | "force" => Some(Self::Always),
            _ => None,
        }
    }
}

/// Bodies smaller than this are never compressed: savings are below noise.
pub const MIN_COMPRESS_BYTES: usize = 32 * 1024;
/// Assumed uplink before any measurement exists. Typical fixed broadband;
/// conservative enough that fast links only compress very large bodies.
const DEFAULT_UPLINK_BPS: f64 = 50e6;
/// Combined client encode + provider decode throughput, in bytes per second.
/// flate2 level 1 measured ~200 MB/s on session JSON; halved for decode and
/// slower client CPUs.
const CODEC_BYTES_PER_SEC: f64 = 100e6;
/// Predicted saving required before compressing, to stay out of the noise.
const MIN_SAVING: Duration = Duration::from_millis(15);
/// Bandwidth samples need a body big enough to overflow socket buffering.
const MIN_SAMPLE_BYTES: usize = 192 * 1024;
/// Bytes pulled before the clock starts: these land in the kernel send buffer
/// at memory speed and say nothing about the network.
const SAMPLE_SKIP_BYTES: usize = 96 * 1024;
const SAMPLE_CHUNK_BYTES: usize = 32 * 1024;
const EWMA_ALPHA: f64 = 0.35;

#[derive(Debug, Clone, Copy)]
struct UplinkEstimate {
    bps: f64,
    samples: u32,
}

static UPLINK: Mutex<Option<UplinkEstimate>> = Mutex::new(None);

/// Current uplink estimate in bits per second, if any sample has been taken.
pub fn uplink_estimate_bps() -> Option<f64> {
    UPLINK.lock().ok().and_then(|g| g.map(|e| e.bps))
}

fn record_uplink_sample(bps: f64) {
    if !bps.is_finite() || bps <= 0.0 {
        return;
    }
    // Clamp so one absurd sample (all bytes buffered) cannot dominate.
    let bps = bps.clamp(0.25e6, 2e9);
    if let Ok(mut guard) = UPLINK.lock() {
        let next = match *guard {
            None => UplinkEstimate { bps, samples: 1 },
            Some(prev) => UplinkEstimate {
                bps: prev.bps + EWMA_ALPHA * (bps - prev.bps),
                samples: prev.samples.saturating_add(1),
            },
        };
        *guard = Some(next);
    }
}

fn reset_uplink_for_tests(value: Option<f64>) {
    if let Ok(mut guard) = UPLINK.lock() {
        *guard = value.map(|bps| UplinkEstimate { bps, samples: 1 });
    }
}

/// A request-body content coding an endpoint is known to accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyEncoding {
    Gzip,
    Zstd,
}

impl BodyEncoding {
    fn header_value(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
        }
    }
}

/// Highest raw/compressed ratio sent zstd-encoded. api.openai.com rejects
/// bodies past an "expansion limit" (HTTP 413): a live probe accepted 82x and
/// rejected 142x. Real session JSON is 2-4x; anything this extreme is
/// synthetic padding, which is sent raw rather than risk a hard failure.
const MAX_ZSTD_EXPANSION: usize = 50;

/// The content coding an endpoint is verified to accept, if any. Verified live
/// (real request decoded, truncated body rejected):
/// - gzip: api.anthropic.com `/v1/messages` (API key and OAuth), chatgpt.com
///   Codex backend.
/// - zstd: api.openai.com `/v1/responses` (gzip is rejected there).
///
/// OpenRouter, the jcode router, `/v1/chat/completions`, and any custom or
/// proxied base URL are never assumed to support compressed bodies.
pub fn endpoint_encoding(url: &str) -> Option<BodyEncoding> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let path = &rest[host.len()..];
    match host.to_ascii_lowercase().as_str() {
        "api.anthropic.com" if path.starts_with("/v1/messages") => Some(BodyEncoding::Gzip),
        "chatgpt.com" if path.starts_with("/backend-api/codex/") => Some(BodyEncoding::Gzip),
        "api.openai.com" if path.starts_with("/v1/responses") => Some(BodyEncoding::Zstd),
        _ => None,
    }
}

/// Whether an endpoint is known to accept gzip-encoded request bodies.
pub fn endpoint_accepts_gzip(url: &str) -> bool {
    endpoint_encoding(url) == Some(BodyEncoding::Gzip)
}

/// Predicted wall-clock saving from sending `compressed` bytes instead of `raw`
/// bytes over an uplink of `uplink_bps`. Negative means compression loses.
pub fn predicted_saving_secs(raw: usize, compressed: usize, uplink_bps: f64) -> f64 {
    let saved_bits = (raw.saturating_sub(compressed) as f64) * 8.0;
    let upload_saved = saved_bits / uplink_bps.max(1.0);
    let codec_cost = raw as f64 / CODEC_BYTES_PER_SEC;
    upload_saved - codec_cost
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Raw(&'static str),
    Gzip { compressed_len: usize },
    Zstd { compressed_len: usize },
}

/// Decide how to send a body. Compresses lazily: skips encode work entirely
/// when even perfect compression could not pay off.
fn decide(
    raw: &[u8],
    mode: CompressionMode,
    encoding: Option<BodyEncoding>,
    uplink_bps: f64,
) -> (Decision, Option<Vec<u8>>) {
    if mode == CompressionMode::Off {
        return (Decision::Raw("disabled"), None);
    }
    let Some(encoding) = encoding else {
        return (Decision::Raw("endpoint"), None);
    };
    if raw.len() < MIN_COMPRESS_BYTES {
        return (Decision::Raw("small"), None);
    }
    let min_saving = MIN_SAVING.as_secs_f64();
    if mode == CompressionMode::Auto && predicted_saving_secs(raw.len(), 0, uplink_bps) < min_saving
    {
        return (Decision::Raw("fast-uplink"), None);
    }
    let compressed = match encoding {
        BodyEncoding::Gzip => gzip_fast(raw),
        BodyEncoding::Zstd => zstd_fast(raw),
    };
    let Some(compressed) = compressed else {
        return (Decision::Raw("encode-error"), None);
    };
    if encoding == BodyEncoding::Zstd && raw.len() > compressed.len() * MAX_ZSTD_EXPANSION {
        return (Decision::Raw("expansion-limit"), None);
    }
    let worth_it = match mode {
        CompressionMode::Always => compressed.len() < raw.len(),
        _ => predicted_saving_secs(raw.len(), compressed.len(), uplink_bps) >= min_saving,
    };
    if !worth_it {
        return (Decision::Raw("low-ratio"), None);
    }
    let compressed_len = compressed.len();
    let decision = match encoding {
        BodyEncoding::Gzip => Decision::Gzip { compressed_len },
        BodyEncoding::Zstd => Decision::Zstd { compressed_len },
    };
    (decision, Some(compressed))
}

fn zstd_fast(raw: &[u8]) -> Option<Vec<u8>> {
    // Level 1 is zstd's fastest standard level and still beats gzip-1's ratio.
    zstd::bulk::compress(raw, 1).ok()
}

fn gzip_fast(raw: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(
        Vec::with_capacity(raw.len() / 2),
        flate2::Compression::fast(),
    );
    encoder.write_all(raw).ok()?;
    encoder.finish().ok()
}

/// Conservative compression ratio assumed when a decision must be made before
/// compressing (streaming transports with a shared deflate context cannot
/// compress speculatively and then discard). Measured session JSON ranges
/// 1.3-3.6x at gzip-1.
const ASSUMED_STREAM_RATIO: f64 = 1.5;

/// Whether a single outbound message on a compressed stream (WebSocket
/// permessage-deflate) should be compressed. Mirrors [`json_body`]'s policy:
/// never below [`MIN_COMPRESS_BYTES`]; in `Auto`, only when the uplink model
/// predicts a saving at a conservative assumed ratio.
pub fn should_compress_stream_message(raw_len: usize, mode: CompressionMode) -> bool {
    if mode == CompressionMode::Off || raw_len < MIN_COMPRESS_BYTES {
        return false;
    }
    if mode == CompressionMode::Always {
        return true;
    }
    let assumed = (raw_len as f64 / ASSUMED_STREAM_RATIO) as usize;
    let uplink = uplink_estimate_bps().unwrap_or(DEFAULT_UPLINK_BPS);
    predicted_saving_secs(raw_len, assumed, uplink) >= MIN_SAVING.as_secs_f64()
}

/// Feed an externally measured upload (for example a WebSocket writer that
/// timed `bytes` draining over `elapsed`) into the shared uplink estimate.
pub fn record_external_uplink_sample(bytes: usize, elapsed: Duration) {
    if bytes >= SAMPLE_CHUNK_BYTES * 2 && !elapsed.is_zero() {
        record_uplink_sample(bytes as f64 * 8.0 / elapsed.as_secs_f64().max(0.001));
    }
}

/// Bytes a stream writer should skip before timing an upload sample: these
/// land in the kernel send buffer at memory speed.
pub const STREAM_SAMPLE_SKIP_BYTES: usize = SAMPLE_SKIP_BYTES;
/// Minimum wire size of a message worth timing for uplink bandwidth.
pub const STREAM_MIN_SAMPLE_BYTES: usize = MIN_SAMPLE_BYTES;

/// Test hook for downstream crates: pin the uplink estimate.
#[doc(hidden)]
pub fn set_uplink_estimate_for_tests(value: Option<f64>) {
    reset_uplink_for_tests(value);
}

/// Attach `body` as JSON to `builder`, gzip-compressing it when the endpoint
/// supports it and the uplink model predicts a latency win. Large bodies are
/// streamed so their upload rate refines the model for future turns.
pub fn json_body<T: serde::Serialize + ?Sized>(
    builder: reqwest::RequestBuilder,
    url: &str,
    body: &T,
    mode: CompressionMode,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let raw = serde_json::to_vec(body)?;
    let encoding = endpoint_encoding(url);
    let eligible = encoding.is_some();
    let uplink = uplink_estimate_bps();
    let started = Instant::now();
    let (decision, compressed) = decide(&raw, mode, encoding, uplink.unwrap_or(DEFAULT_UPLINK_BPS));
    let builder = builder.header(reqwest::header::CONTENT_TYPE, "application/json");
    let raw_len = raw.len();
    let (builder, wire) = match (compressed, encoding) {
        (Some(bytes), Some(encoding)) => (
            builder.header(reqwest::header::CONTENT_ENCODING, encoding.header_value()),
            bytes,
        ),
        _ => (builder, raw),
    };
    if raw_len >= MIN_COMPRESS_BYTES {
        jcode_logging::info(&format!(
            "request_compression: decision={:?} raw={}B wire={}B uplink_est={} encode={}ms",
            decision,
            raw_len,
            wire.len(),
            uplink
                .map(|bps| format!("{:.1}Mbit", bps / 1e6))
                .unwrap_or_else(|| "unknown".to_string()),
            started.elapsed().as_millis()
        ));
    }
    // Only sample on eligible endpoints: bandwidth estimates are useless
    // elsewhere, and keeping other requests on a plain body avoids any
    // streaming-body quirks with third-party proxies.
    if eligible && mode != CompressionMode::Off && wire.len() >= MIN_SAMPLE_BYTES {
        Ok(sampling_body(builder, wire))
    } else {
        Ok(builder.body(wire))
    }
}

/// Stream `wire` in fixed chunks, timing how fast hyper pulls them. Once the
/// send buffer is full, hyper can only pull as fast as TCP drains, so the pull
/// rate tracks real uplink throughput.
fn sampling_body(builder: reqwest::RequestBuilder, wire: Vec<u8>) -> reqwest::RequestBuilder {
    let total = wire.len();
    let wire = bytes::Bytes::from(wire);
    let mut sampler = UploadSampler::new(total);
    let chunks = (0..total).step_by(SAMPLE_CHUNK_BYTES).map(move |start| {
        let end = (start + SAMPLE_CHUNK_BYTES).min(total);
        sampler.on_pull(start, end);
        Ok::<_, std::io::Error>(wire.slice(start..end))
    });
    builder
        .header(reqwest::header::CONTENT_LENGTH, total)
        .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
}

struct UploadSampler {
    total: usize,
    clock_start: Option<(usize, Instant)>,
}

impl UploadSampler {
    fn new(total: usize) -> Self {
        Self {
            total,
            clock_start: None,
        }
    }

    /// Called as each chunk `[start, end)` is handed to hyper. Hyper pulls a
    /// chunk only after the previous one was written into the socket, so the
    /// pull of the final chunk marks when all earlier bytes left userspace.
    fn on_pull(&mut self, start: usize, end: usize) {
        let now = Instant::now();
        if self.clock_start.is_none() && start >= SAMPLE_SKIP_BYTES {
            self.clock_start = Some((start, now));
        }
        if end == self.total
            && let Some((from, at)) = self.clock_start
        {
            let elapsed = now.duration_since(at).as_secs_f64();
            let bytes = start.saturating_sub(from);
            if bytes >= SAMPLE_CHUNK_BYTES * 2 {
                // Sub-millisecond drains mean the buffer swallowed everything:
                // the link is at least this fast, which still safely says
                // "fast uplink" after clamping.
                let bps = bytes as f64 * 8.0 / elapsed.max(0.001);
                record_uplink_sample(bps);
                jcode_logging::info(&format!(
                    "request_compression: uplink sample {:.1}Mbit ({}B in {:.0}ms), estimate now {:.1}Mbit",
                    bps / 1e6,
                    bytes,
                    elapsed * 1000.0,
                    uplink_estimate_bps().unwrap_or(bps) / 1e6
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "request_compression_tests.rs"]
mod tests;
