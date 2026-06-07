use crate::desktop_config::env_flag_enabled;
use std::time::Instant;
use winit::dpi::PhysicalSize;

pub(super) fn startup_log_requested(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--startup-log")
        || std::env::var_os("JCODE_DESKTOP_STARTUP_LOG").is_some_and(env_flag_enabled)
}

pub(super) fn startup_benchmark_requested(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--startup-benchmark")
}

pub(super) fn startup_content_benchmark_requested(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--startup-content-benchmark")
}

pub(super) fn scroll_render_benchmark_frames(args: &[String]) -> Option<usize> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--scroll-render-benchmark=")
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                (arg == "--scroll-render-benchmark").then(|| {
                    args.get(index + 1)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(600)
                })
            })
    })
}

pub(super) fn resize_render_benchmark_frames(args: &[String]) -> Option<usize> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--resize-render-benchmark=")
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                (arg == "--resize-render-benchmark").then(|| {
                    args.get(index + 1)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(240)
                })
            })
    })
}

/// Parse `--real-transcript-scroll-benchmark[=N]`, the number of scroll frames
/// to profile against each of the user's largest real on-disk transcripts.
pub(super) fn real_transcript_scroll_benchmark_frames(args: &[String]) -> Option<usize> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--real-transcript-scroll-benchmark=")
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                (arg == "--real-transcript-scroll-benchmark").then(|| {
                    args.get(index + 1)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(600)
                })
            })
    })
}

/// Parse `--real-transcript-action-benchmark[=N]`, the per-phase frame count for
/// the multi-action interaction benchmark run against real on-disk transcripts.
pub(super) fn real_transcript_action_benchmark_frames(args: &[String]) -> Option<usize> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--real-transcript-action-benchmark=")
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                (arg == "--real-transcript-action-benchmark").then(|| {
                    args.get(index + 1)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(400)
                })
            })
    })
}

pub(super) fn benchmark_phase(
    mut frames: usize,
    mut run_frame: impl FnMut(usize) -> usize,
) -> (f64, usize) {
    frames = frames.max(1);
    let started = Instant::now();
    let mut checksum = 0usize;
    for frame in 0..frames {
        checksum ^= std::hint::black_box(run_frame(frame));
    }
    (started.elapsed().as_secs_f64() * 1000.0, checksum)
}

pub(super) fn benchmark_frame_samples(
    mut frames: usize,
    mut run_frame: impl FnMut(usize) -> usize,
) -> (Vec<f64>, usize) {
    frames = frames.max(1);
    let mut samples = Vec::with_capacity(frames);
    let mut checksum = 0usize;
    for frame in 0..frames {
        let started = Instant::now();
        checksum ^= std::hint::black_box(run_frame(frame));
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    (samples, checksum)
}

pub(super) fn benchmark_phase_json(
    name: &str,
    total_ms: f64,
    frames: usize,
    checksum: usize,
) -> serde_json::Value {
    let frames = frames.max(1);
    serde_json::json!({
        "name": name,
        "total_ms": total_ms,
        "mean_ms_per_frame": total_ms / frames as f64,
        "mean_us_per_frame": total_ms * 1000.0 / frames as f64,
        "checksum": checksum,
    })
}

pub(super) fn benchmark_samples_json(
    name: &str,
    samples: &[f64],
    checksum: usize,
) -> serde_json::Value {
    let frames = samples.len().max(1);
    let total_ms = samples.iter().sum::<f64>();
    serde_json::json!({
        "name": name,
        "frames": samples.len(),
        "total_ms": total_ms,
        "mean_ms_per_frame": total_ms / frames as f64,
        "p50_ms": percentile_ms(samples, 0.50),
        "p95_ms": percentile_ms(samples, 0.95),
        "p99_ms": percentile_ms(samples, 0.99),
        "max_ms": max_sample_ms(samples),
        "checksum": checksum,
    })
}

pub(super) fn percentile_ms(samples: &[f64], quantile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let index = ((sorted.len() as f64 * quantile.clamp(0.0, 1.0)).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

pub(super) fn max_sample_ms(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(0.0, f64::max)
}

pub(super) fn benchmark_resize_size(frame: usize) -> PhysicalSize<u32> {
    let width = 1080 + ((frame * 17) % 260) as u32;
    let height = 650 + ((frame * 11) % 180) as u32;
    PhysicalSize::new(width, height)
}

pub(super) fn benchmark_smooth_scroll_lines(frame: usize) -> f32 {
    ((frame % 16) as f32 / 16.0) - 0.5
}

pub(super) fn benchmark_typing_char(frame: usize) -> char {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz     .,;";
    CHARS[frame % CHARS.len()] as char
}
