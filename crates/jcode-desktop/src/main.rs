mod animation;
mod desktop_app_driver;
mod desktop_benchmark;
mod desktop_config;
mod desktop_gallery;
mod desktop_ipc;
mod desktop_issue_browser;
mod desktop_issue_cache;
mod desktop_log;
mod desktop_prefs;
mod desktop_protocol;
mod desktop_rich_text;
mod desktop_scene;
mod desktop_session_events;
mod desktop_ui_engine;
mod desktop_worker_host;
mod power_inhibit;
mod render_helpers;
mod session_data;
mod session_launch;
mod single_session;
mod single_session_render;
mod workspace;

use ab_glyph::{Font, FontArc, Glyph as AbGlyph, PxScale, ScaleFont, point};
use animation::{
    APP_MODE_TRANSITION_DURATION, AnimatedRect, AnimatedViewport, ColorTransition, FocusPulse,
    StatusTextTransition, StatusTextTransitionFrame, StatusTextVisualFrame,
    SurfaceTransitionAnimator, SurfaceVisualFrame, SurfaceVisualTarget, VisibleColumnLayout,
    WorkspaceRenderLayout,
};
use anyhow::{Context, Result};
use base64::Engine;
use bytemuck::{Pod, Zeroable};
use desktop_app_driver::{
    DESKTOP_UI_SNAPSHOT_VERSION, DesktopAppDriver, DesktopAppRuntime, DesktopSceneBuildContext,
    DesktopSingleSessionSnapshot, DesktopSnapshotRestoreError, DesktopSurfaceSnapshot,
    DesktopUiSnapshot, DesktopWorkspaceSnapshot, DesktopWorkspaceSurfaceSnapshot,
};
use desktop_benchmark::*;
use desktop_config::*;
use desktop_ipc::{DesktopHostToWorkerEnvelope, write_desktop_ipc_frame};
#[cfg(test)]
pub(crate) use desktop_issue_browser::IssueBrowserLayoutMode;
use desktop_issue_browser::{
    IssueBrowserLayout, compose_single_session_issue_browser_vertices, issue_browser_layout,
};
use desktop_protocol::{
    DesktopHostToWorkerMessage, DesktopInputEvent, DesktopKeyEvent, DesktopKeyModifiers,
    DesktopMouseButton, DesktopMouseEvent, DesktopProtocolEnvelope, DesktopSceneUpdate,
    DesktopSessionEventBatchWire, DesktopSessionEventWire, DesktopSnapshotResponse,
    DesktopWindowEvent, DesktopWindowState, DesktopWorkerInit, DesktopWorkerMode,
    DesktopWorkerReady, DesktopWorkerShutdownReason, DesktopWorkerToHostMessage,
};
use desktop_scene::{
    DesktopColor, DesktopDisplayCommand, DesktopRect as DesktopSceneRect, DesktopRectPaint,
    DesktopScene, DesktopSceneViewport,
};
use desktop_session_events::{
    BACKEND_EVENT_FORWARD_INTERVAL, BACKEND_EVENT_FORWARD_MAX_PAYLOAD_BYTES,
    BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS, DesktopSessionEventBatch,
    coalesce_desktop_session_events, collect_desktop_session_event_batch,
    spawn_session_event_forwarder,
};
use desktop_worker_host::DesktopWorkerConnection;
use glyphon::{
    Attrs, Buffer, Color as TextColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Wrap,
};
use image::RgbaImage;
use render_helpers::*;
use session_launch::DesktopSessionStatus;
use single_session::{
    ReasoningEffortCycleOutcome, SINGLE_SESSION_FONT_FAMILY, SINGLE_SESSION_WELCOME_FONT_FAMILY,
    SelectionPoint, SingleSessionApp, SingleSessionLineStyle, SingleSessionMessage,
    SingleSessionStyledLine, handwritten_welcome_phrase, single_session_surface,
    single_session_typography, single_session_typography_for_scale,
};
use single_session_render::*;
use wgpu::{CompositeAlphaMode, PresentMode, SurfaceError, TextureUsages};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowBuilder};
use workspace::{InputMode, KeyInput, KeyOutcome, PanelSizePreset, Workspace};

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_WINDOW_WIDTH: f64 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f64 = 800.0;
const DESKTOP_RELOAD_WINDOW_ENV: &str = "JCODE_DESKTOP_RELOAD_WINDOW";
const DESKTOP_RELOAD_HANDOFF_READY_ENV: &str = "JCODE_DESKTOP_RELOAD_READY_FILE";
const DESKTOP_RELOAD_HANDOFF_RELEASE_ENV: &str = "JCODE_DESKTOP_RELOAD_RELEASE_FILE";
const DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL: Duration = Duration::from_millis(25);
const DESKTOP_RELOAD_HANDOFF_TIMEOUT: Duration = Duration::from_secs(8);
const DESKTOP_RELOAD_STARTUP_RELEASE_TIMEOUT: Duration = Duration::from_secs(3);
const DESKTOP_RELOAD_MAX_RESTORED_DIMENSION: u32 = 32_768;
const OUTER_PADDING: f32 = 8.0;
const GAP: f32 = 6.0;
const STATUS_BAR_HEIGHT: f32 = 30.0;
const FOCUSED_BORDER_WIDTH: f32 = 2.0;
const UNFOCUSED_BORDER_WIDTH: f32 = 1.5;
const PANEL_RADIUS: f32 = 8.0;
const STATUS_RADIUS: f32 = 7.0;
const ROUNDED_CORNER_SEGMENTS: usize = 6;
const PANEL_FIT_TOLERANCE: f32 = 0.15;
const STATUS_PREVIEW_LANE_RADIUS: i32 = 2;
const STATUS_PREVIEW_MAX_WIDTH: f32 = 420.0;
const STATUS_PREVIEW_HEIGHT: f32 = 14.0;
const STATUS_PREVIEW_PANEL_WIDTH: f32 = 9.0;
const STATUS_PREVIEW_PANEL_GAP: f32 = 2.0;
const STATUS_PREVIEW_GROUP_GAP: f32 = 10.0;
const STATUS_PREVIEW_SIDE_RESERVE: f32 = 74.0;
const STATUS_PREVIEW_MAX_TICKS_PER_LANE: i32 = 32;
const SPACE_HOLD_PROGRESS_HEIGHT: f32 = 7.0;
const SPACE_HOLD_PROGRESS_WIDTH_FRACTION: f32 = 0.36;
const SPACE_HOLD_PROGRESS_TRACK_COLOR: [f32; 4] = [0.055, 0.060, 0.075, 0.96];
const SPACE_HOLD_PROGRESS_FILL_COLOR: [f32; 4] = [0.180, 0.900, 0.470, 1.0];
const WORKSPACE_NUMBER_LEFT_PADDING: f32 = 14.0;
const WORKSPACE_NUMBER_DIGIT_WIDTH: f32 = 8.0;
const WORKSPACE_NUMBER_DIGIT_HEIGHT: f32 = 14.0;
const WORKSPACE_NUMBER_DIGIT_GAP: f32 = 4.0;
const WORKSPACE_NUMBER_STROKE: f32 = 2.0;
const BITMAP_TEXT_PIXEL: f32 = 2.0;
const STATUS_TEXT_RIGHT_PADDING: f32 = 14.0;
const PANEL_TITLE_LEFT_PADDING: f32 = 12.0;
const PANEL_TITLE_TOP_PADDING: f32 = 12.0;
const PANEL_BODY_TOP_PADDING: f32 = 38.0;
const PANEL_BODY_LINE_GAP: f32 = 8.0;
const SINGLE_SESSION_DRAFT_TOP_OFFSET: f32 = 158.0;
const SINGLE_SESSION_CARET_WIDTH: f32 = 2.0;
const SINGLE_SESSION_CARET_COLOR: [f32; 4] = [0.130, 0.150, 0.190, 0.92];
const SESSION_SPAWN_REFRESH_DELAY: Duration = Duration::from_millis(350);
const BACKGROUND_POLL_INTERVAL: Duration = Duration::from_millis(33);
const BACKEND_REDRAW_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const SURFACE_TIMEOUT_BACKOFF_MIN: Duration = Duration::from_millis(16);
const SURFACE_TIMEOUT_BACKOFF_MAX: Duration = Duration::from_millis(250);
const HEADLESS_CHAT_SMOKE_TIMEOUT: Duration = Duration::from_secs(90);
const DESKTOP_SPINNER_FRAME_MS: u128 = 180;
const MOUSE_WHEEL_LINES_PER_DETENT: f32 = 3.0;
const MAX_MOUSE_SCROLL_LINES_PER_EVENT: f32 = 24.0;
const SCROLL_GESTURE_IDLE_RESET: Duration = Duration::from_millis(180);
const SCROLL_FRACTIONAL_EPSILON: f32 = 0.000_1;
const SCROLL_MOMENTUM_GAIN: f32 = 8.5;
const SCROLL_MOMENTUM_DECAY_PER_SECOND: f32 = 7.0;
const SCROLL_MOMENTUM_MAX_VELOCITY: f32 = 72.0;
const SCROLL_MOMENTUM_STOP_VELOCITY: f32 = 0.08;
const SCROLL_FRAME_MAX_DT_SECONDS: f32 = 0.050;
const SINGLE_SESSION_SCROLL_ANIMATION_DURATION: Duration = Duration::from_millis(150);
const SINGLE_SESSION_BODY_TEXT_WINDOW_BEFORE_LINES: usize = 8;
const SINGLE_SESSION_BODY_TEXT_WINDOW_AFTER_LINES: usize = 16;
const SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_BEFORE_LINES: usize = 2;
const SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_AFTER_LINES: usize = 4;
const STREAMING_TEXT_FADE_DURATION: Duration = Duration::from_millis(150);
const STREAMING_TEXT_FADE_START_OPACITY: f32 = 0.4;
const STREAMING_TEXT_RISE_START_OFFSET_PIXELS: f32 = 3.5;
const STREAMING_TEXT_HANDOFF_DURATION: Duration = Duration::from_millis(135);
const STREAMING_TEXT_HANDOFF_START_OPACITY: f32 = 0.18;
const DESKTOP_ASYNC_JOB_LIMIT: usize = 12;
const PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY: usize = 1024;
const PRIMITIVE_VERTEX_BUFFER_SHRINK_RATIO: usize = 4;
const WORKSPACE_BASE_VERTEX_CAPACITY_HINT: usize = 512;
const WORKSPACE_SURFACE_VERTEX_CAPACITY_HINT: usize = 2048;
static DESKTOP_ASYNC_JOB_COUNT: AtomicUsize = AtomicUsize::new(0);

struct DesktopAsyncJobPermit<'a> {
    counter: &'a AtomicUsize,
}

impl Drop for DesktopAsyncJobPermit<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

fn try_acquire_desktop_async_job_slot<'a>(
    counter: &'a AtomicUsize,
    limit: usize,
) -> Result<DesktopAsyncJobPermit<'a>> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        if current >= limit {
            anyhow::bail!("desktop async job limit reached ({limit})");
        }
        match counter.compare_exchange_weak(
            current,
            current + 1,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Ok(DesktopAsyncJobPermit { counter }),
            Err(next_current) => current = next_current,
        }
    }
}

fn spawn_bounded_desktop_async_job(
    name: impl Into<String>,
    job: impl FnOnce() + Send + 'static,
) -> Result<()> {
    let name = name.into();
    let permit =
        try_acquire_desktop_async_job_slot(&DESKTOP_ASYNC_JOB_COUNT, DESKTOP_ASYNC_JOB_LIMIT)
            .with_context(|| format!("failed to start {name}"))?;
    std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            let _permit = permit;
            job();
        })
        .with_context(|| format!("failed to spawn {name}"))?;
    Ok(())
}

#[derive(Clone)]
struct DesktopReasoningEffortRequestQueue {
    request_tx: mpsc::Sender<DesktopReasoningEffortRequest>,
    latest_generation: Arc<AtomicU64>,
}

struct DesktopReasoningEffortRequest {
    generation: u64,
    effort: String,
    target_session_id: Option<String>,
    event_tx: session_launch::DesktopSessionEventSender,
}

impl DesktopReasoningEffortRequestQueue {
    fn request(
        &self,
        effort: String,
        target_session_id: Option<String>,
        event_tx: session_launch::DesktopSessionEventSender,
    ) -> Result<()> {
        let generation = self.latest_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.request_tx
            .send(DesktopReasoningEffortRequest {
                generation,
                effort,
                target_session_id,
                event_tx,
            })
            .context("failed to queue desktop reasoning effort change")
    }
}

fn spawn_desktop_reasoning_effort_request_queue() -> Result<DesktopReasoningEffortRequestQueue> {
    let (request_tx, request_rx) = mpsc::channel();
    let latest_generation = Arc::new(AtomicU64::new(0));
    let worker_latest_generation = Arc::clone(&latest_generation);
    std::thread::Builder::new()
        .name("jcode-desktop-effort-queue".to_string())
        .spawn(move || {
            run_desktop_reasoning_effort_request_queue(request_rx, worker_latest_generation);
        })
        .context("failed to spawn desktop reasoning effort queue")?;
    Ok(DesktopReasoningEffortRequestQueue {
        request_tx,
        latest_generation,
    })
}

fn run_desktop_reasoning_effort_request_queue(
    request_rx: mpsc::Receiver<DesktopReasoningEffortRequest>,
    latest_generation: Arc<AtomicU64>,
) {
    while let Ok(mut request) = request_rx.recv() {
        let mut coalesced = 0usize;
        let mut disconnected = false;
        loop {
            match request_rx.try_recv() {
                Ok(next_request) => {
                    request = next_request;
                    coalesced += 1;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if coalesced > 0 {
            desktop_log::info(format_args!(
                "jcode-desktop: coalesced {coalesced} superseded reasoning effort request(s); applying {}",
                desktop_log::truncate_for_log(&request.effort, 64)
            ));
        }
        apply_desktop_reasoning_effort_request(request, &latest_generation);
        if disconnected {
            break;
        }
    }
}

fn apply_desktop_reasoning_effort_request(
    request: DesktopReasoningEffortRequest,
    latest_generation: &AtomicU64,
) {
    let (response_tx, response_rx) = mpsc::channel();
    let result = session_launch::set_reasoning_effort(
        &request.effort,
        request.target_session_id.as_deref(),
        Some(response_tx),
    );
    let still_latest = latest_generation.load(Ordering::Acquire) == request.generation;
    if still_latest {
        for event in response_rx.try_iter() {
            let _ = request.event_tx.send(event);
        }
        if let Err(error) = result {
            desktop_log::error(format_args!(
                "jcode-desktop: reasoning effort sync failed generation={} target_session={}: {error:#}",
                request.generation,
                request.target_session_id.as_deref().unwrap_or("<current>")
            ));
            let _ = request
                .event_tx
                .send(session_launch::DesktopSessionEvent::Status(
                    DesktopSessionStatus::ReasoningEffortFailed(format!("{error:#}")),
                ));
        }
    } else if let Err(error) = result {
        desktop_log::warn(format_args!(
            "jcode-desktop: stale reasoning effort sync failed generation={} target_session={}: {error:#}",
            request.generation,
            request.target_session_id.as_deref().unwrap_or("<current>")
        ));
    } else {
        let dropped = response_rx.try_iter().count();
        desktop_log::info(format_args!(
            "jcode-desktop: dropped stale reasoning effort response generation={} event_count={dropped}",
            request.generation
        ));
    }
}

#[derive(Clone, Debug, Default)]
struct SurfaceTimeoutBackoff {
    consecutive_timeouts: u32,
}

impl SurfaceTimeoutBackoff {
    fn reset(&mut self) {
        self.consecutive_timeouts = 0;
    }

    fn record_timeout(&mut self) -> (Duration, u32) {
        let exponent = self.consecutive_timeouts.min(4);
        self.consecutive_timeouts = self.consecutive_timeouts.saturating_add(1);
        let delay = SURFACE_TIMEOUT_BACKOFF_MIN
            .saturating_mul(1_u32 << exponent)
            .min(SURFACE_TIMEOUT_BACKOFF_MAX);
        (delay, self.consecutive_timeouts)
    }
}

fn desktop_surface_size_is_renderable(size: PhysicalSize<u32>) -> bool {
    size.width > 0 && size.height > 0
}

fn desktop_background_wake(
    now: Instant,
    surface_renderable: bool,
    frame_animation_active: bool,
) -> Option<Instant> {
    if surface_renderable && frame_animation_active {
        Some(now + BACKGROUND_POLL_INTERVAL)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StreamingTextArrivalStyle {
    opacity: f32,
    y_offset_pixels: f32,
    active: bool,
}

fn streaming_text_arrival_style_for_elapsed(elapsed: Duration) -> StreamingTextArrivalStyle {
    if animation::desktop_reduced_motion_enabled() {
        return StreamingTextArrivalStyle {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            active: false,
        };
    }

    let progress =
        (elapsed.as_secs_f32() / STREAMING_TEXT_FADE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    if progress >= 1.0 {
        return StreamingTextArrivalStyle {
            opacity: 1.0,
            y_offset_pixels: 0.0,
            active: false,
        };
    }
    let eased = animation::ease_out_cubic(progress);
    StreamingTextArrivalStyle {
        opacity: STREAMING_TEXT_FADE_START_OPACITY
            + (1.0 - STREAMING_TEXT_FADE_START_OPACITY) * eased,
        y_offset_pixels: STREAMING_TEXT_RISE_START_OFFSET_PIXELS * (1.0 - eased),
        active: true,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn streaming_text_fade_opacity_for_elapsed(elapsed: Duration) -> (f32, bool) {
    let style = streaming_text_arrival_style_for_elapsed(elapsed);
    (style.opacity, style.active)
}

fn streaming_text_fade_start_after_len_change(
    previous_len: usize,
    next_len: usize,
    current_started_at: Option<Instant>,
    now: Instant,
) -> Option<Instant> {
    if next_len == 0 {
        return None;
    }

    let fade_active = current_started_at.is_some_and(|started_at| {
        now.saturating_duration_since(started_at) < STREAMING_TEXT_FADE_DURATION
    });
    if fade_active {
        return current_started_at;
    }

    // Only fade in the beginning of a streaming response. Restarting after
    // every slow delta dims the already-visible response and reads as flicker.
    if previous_len == 0 && next_len > 0 {
        Some(now)
    } else {
        None
    }
}

fn streaming_text_handoff_style_for_elapsed(elapsed: Duration) -> StreamingTextArrivalStyle {
    if animation::desktop_reduced_motion_enabled() {
        return StreamingTextArrivalStyle {
            opacity: 0.0,
            y_offset_pixels: 0.0,
            active: false,
        };
    }

    let progress =
        (elapsed.as_secs_f32() / STREAMING_TEXT_HANDOFF_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    if progress >= 1.0 {
        return StreamingTextArrivalStyle {
            opacity: 0.0,
            y_offset_pixels: 0.0,
            active: false,
        };
    }

    let eased = animation::ease_out_cubic(progress);
    StreamingTextArrivalStyle {
        opacity: STREAMING_TEXT_HANDOFF_START_OPACITY * (1.0 - eased),
        y_offset_pixels: 0.0,
        active: true,
    }
}

fn streaming_text_handoff_start_after_len_change(
    previous_len: usize,
    next_len: usize,
    has_visible_streaming_buffer: bool,
    current_started_at: Option<Instant>,
    now: Instant,
) -> Option<Instant> {
    if animation::desktop_reduced_motion_enabled() || next_len > 0 {
        return None;
    }

    if previous_len > 0 && has_visible_streaming_buffer {
        return Some(now);
    }

    current_started_at.filter(|started_at| {
        now.saturating_duration_since(*started_at) < STREAMING_TEXT_HANDOFF_DURATION
    })
}
const DESKTOP_120FPS_FRAME_BUDGET: Duration = Duration::from_micros(8_333);
const DESKTOP_PRESENT_STALL_BUDGET: Duration = Duration::from_millis(33);
const DESKTOP_INPUT_LATENCY_BUDGET: Duration = Duration::from_millis(25);
const DESKTOP_NO_PAINT_BUDGET: Duration = Duration::from_millis(250);
const DESKTOP_FRAME_PROFILE_REPORT_INTERVAL: Duration = Duration::from_secs(1);

const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.955,
    g: 0.965,
    b: 0.985,
    a: 1.0,
};

const BACKGROUND_TOP_LEFT: [f32; 4] = [0.858, 0.910, 1.000, 1.0];
const BACKGROUND_TOP_RIGHT: [f32; 4] = [0.945, 0.884, 1.000, 1.0];
const BACKGROUND_BOTTOM_RIGHT: [f32; 4] = [0.846, 0.972, 0.910, 1.0];
const BACKGROUND_BOTTOM_LEFT: [f32; 4] = [0.930, 0.950, 0.988, 1.0];
const FOCUS_RING_COLOR: [f32; 4] = [0.165, 0.185, 0.225, 0.94];
const NAV_STATUS_COLOR: [f32; 4] = [0.184, 0.204, 0.251, 1.0];
const INSERT_STATUS_COLOR: [f32; 4] = [0.310, 0.435, 0.376, 1.0];
const STATUS_PREVIEW_ACTIVE_GROUP_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.16];
const STATUS_PREVIEW_EMPTY_FOCUSED_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.50];
const STATUS_PREVIEW_VIEWPORT_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.78];
const WORKSPACE_NUMBER_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.90];
const STATUS_TEXT_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.88];
const PANEL_TITLE_COLOR: [f32; 4] = [0.010, 0.014, 0.025, 1.0];
const PANEL_BODY_COLOR: [f32; 4] = [0.008, 0.012, 0.020, 1.0];
const ASSISTANT_TEXT_COLOR: [f32; 4] = [0.026, 0.034, 0.052, 1.0];
const ASSISTANT_HEADING_TEXT_COLOR: [f32; 4] = [0.030, 0.095, 0.300, 1.0];
const ASSISTANT_QUOTE_TEXT_COLOR: [f32; 4] = [0.210, 0.090, 0.355, 1.0];
const ASSISTANT_TABLE_TEXT_COLOR: [f32; 4] = [0.000, 0.155, 0.185, 1.0];
const ASSISTANT_LINK_TEXT_COLOR: [f32; 4] = [0.000, 0.170, 0.430, 1.0];
const USER_TEXT_COLOR: [f32; 4] = [0.012, 0.030, 0.180, 1.0];
const USER_CONTINUATION_TEXT_COLOR: [f32; 4] = [0.018, 0.035, 0.155, 1.0];
const TOOL_TEXT_COLOR: [f32; 4] = [0.150, 0.095, 0.325, 1.0];
const TOOL_DETAIL_TEXT_COLOR: [f32; 4] = [0.135, 0.155, 0.220, 1.0];
const TOOL_MUTED_TEXT_COLOR: [f32; 4] = [0.345, 0.365, 0.430, 0.96];
const TOOL_RUNNING_TEXT_COLOR: [f32; 4] = [0.045, 0.265, 0.640, 1.0];
const TOOL_SUCCESS_TEXT_COLOR: [f32; 4] = [0.035, 0.360, 0.220, 1.0];
const TOOL_FAILED_TEXT_COLOR: [f32; 4] = [0.560, 0.070, 0.095, 1.0];
const TOOL_PENDING_TEXT_COLOR: [f32; 4] = [0.320, 0.345, 0.405, 1.0];
const TOOL_CARD_BACKGROUND_COLOR: [f32; 4] = [0.985, 0.990, 1.000, 0.56];
const TOOL_CARD_ACTIVE_BACKGROUND_COLOR: [f32; 4] = [0.890, 0.945, 1.000, 0.62];
const TOOL_CARD_SUCCESS_BACKGROUND_COLOR: [f32; 4] = [0.875, 0.975, 0.925, 0.46];
const TOOL_CARD_FAILED_BACKGROUND_COLOR: [f32; 4] = [1.000, 0.900, 0.910, 0.54];
const TOOL_CARD_GROUP_BACKGROUND_COLOR: [f32; 4] = [0.945, 0.930, 1.000, 0.40];
const TOOL_CARD_BORDER_COLOR: [f32; 4] = [0.105, 0.165, 0.295, 0.16];
const TOOL_CARD_ACTIVE_BORDER_COLOR: [f32; 4] = [0.000, 0.260, 0.720, 0.28];
const TOOL_TIMELINE_RAIL_COLOR: [f32; 4] = [0.105, 0.165, 0.295, 0.20];
const TOOL_TIMELINE_ACTIVE_RAIL_COLOR: [f32; 4] = [0.000, 0.260, 0.720, 0.46];
const TOOL_OUTPUT_DRAWER_COLOR: [f32; 4] = [0.030, 0.055, 0.095, 0.070];
const TOOL_STATUS_CHIP_COLOR: [f32; 4] = [1.000, 1.000, 1.000, 0.42];
const META_TEXT_COLOR: [f32; 4] = [0.095, 0.110, 0.155, 0.98];
const CODE_TEXT_COLOR: [f32; 4] = [0.055, 0.065, 0.095, 1.0];
const STATUS_TEXT_ACCENT_COLOR: [f32; 4] = [0.030, 0.125, 0.080, 1.0];
const ERROR_TEXT_COLOR: [f32; 4] = [0.360, 0.000, 0.000, 1.0];
const OVERLAY_TEXT_COLOR: [f32; 4] = [0.030, 0.045, 0.075, 1.0];
const OVERLAY_SELECTION_TEXT_COLOR: [f32; 4] = [0.010, 0.035, 0.105, 1.0];
const USER_PROMPT_ACCENT_COLOR: [f32; 4] = [0.000, 0.105, 0.250, 1.0];
const PANEL_SECTION_COLOR: [f32; 4] = [0.045, 0.055, 0.080, 0.95];
const SELECTION_HIGHLIGHT_COLOR: [f32; 4] = [0.220, 0.420, 0.700, 0.22];
const WELCOME_AURORA_BLUE: [f32; 4] = [0.250, 0.520, 1.000, 0.145];
const WELCOME_AURORA_VIOLET: [f32; 4] = [0.720, 0.360, 0.980, 0.125];
const WELCOME_AURORA_MINT: [f32; 4] = [0.220, 0.840, 0.660, 0.115];
const WELCOME_AURORA_WARM: [f32; 4] = [1.000, 0.620, 0.360, 0.075];
const WELCOME_HANDWRITING_COLOR: [f32; 4] = [0.012, 0.080, 0.250, 0.94];
const NATIVE_SPINNER_HEAD_COLOR: [f32; 4] = [0.000, 0.260, 0.720, 1.0];
const CODE_BLOCK_BACKGROUND_COLOR: [f32; 4] = [0.075, 0.095, 0.135, 0.075];
const INLINE_CODE_BACKGROUND_COLOR: [f32; 4] = [0.075, 0.095, 0.135, 0.135];
const QUOTE_CARD_BACKGROUND_COLOR: [f32; 4] = [0.520, 0.330, 0.760, 0.070];
const TABLE_CARD_BACKGROUND_COLOR: [f32; 4] = [0.080, 0.460, 0.520, 0.060];
const ERROR_CARD_BACKGROUND_COLOR: [f32; 4] = [0.850, 0.170, 0.170, 0.105];
const OVERLAY_SELECTION_BACKGROUND_COLOR: [f32; 4] = [0.280, 0.470, 0.780, 0.115];
const STATUS_PREVIEW_ACCENTS: [[f32; 3]; 8] = [
    [0.560, 0.690, 0.980],
    [0.780, 0.610, 0.910],
    [0.520, 0.760, 0.620],
    [0.900, 0.650, 0.450],
    [0.600, 0.780, 0.840],
    [0.880, 0.580, 0.690],
    [0.720, 0.740, 0.820],
    [0.810, 0.760, 0.520],
];

const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

const HERO_MASK_SHADER: &str = r#"
struct HeroVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct HeroRevealUniform {
    progress: f32,
    feather: f32,
    padding: vec2<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var hero_alpha: texture_2d<f32>;
@group(0) @binding(1) var hero_reveal: texture_2d<f32>;
@group(0) @binding(2) var hero_sampler: sampler;
@group(0) @binding(3) var<uniform> hero_uniform: HeroRevealUniform;

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> HeroVertexOutput {
    var out: HeroVertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: HeroVertexOutput) -> @location(0) vec4<f32> {
    let glyph_alpha = textureSample(hero_alpha, hero_sampler, in.uv).r;
    let reveal_at = textureSample(hero_reveal, hero_sampler, in.uv).r;
    let feather = max(hero_uniform.feather, 0.0001);
    let t = clamp((hero_uniform.progress - reveal_at + feather) / (2.0 * feather), 0.0, 1.0);
    let softened = t * t * (3.0 - 2.0 * t);
    let reveal_alpha = select(softened, 1.0, hero_uniform.progress >= 0.999);
    return vec4<f32>(hero_uniform.color.rgb, glyph_alpha * reveal_alpha * hero_uniform.color.a);
}
"#;

fn main() {
    desktop_log::init();
    install_desktop_diagnostic_hooks();
    desktop_log::info(format_args!(
        "jcode-desktop: starting pid={} version={} build_hash={}",
        std::process::id(),
        desktop_header_version_label(),
        desktop_build_hash_label()
    ));

    if let Err(error) = pollster::block_on(run()) {
        desktop_log::error(format_args!("jcode-desktop: fatal error: {error:#}"));
        std::process::exit(1);
    }
}

fn install_desktop_diagnostic_hooks() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        desktop_log::error(format_args!("jcode-desktop: panic: {panic_info}"));
        desktop_log::error(format_args!(
            "jcode-desktop: panic backtrace: {}",
            std::backtrace::Backtrace::force_capture()
        ));
        default_hook(panic_info);
    }));
}

async fn run() -> Result<()> {
    log_desktop_platform_support_warning();
    let args = std::env::args().collect::<Vec<_>>();
    let startup_benchmark = startup_benchmark_requested(&args);
    let startup_content_benchmark = startup_content_benchmark_requested(&args);
    let startup_trace = DesktopStartupTrace::new(
        startup_benchmark || startup_content_benchmark || startup_log_requested(&args),
    );
    startup_trace.mark("args parsed");
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", desktop_help_text());
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "{} {}",
            desktop_header_version_label(),
            desktop_build_hash_label()
        );
        return Ok(());
    }
    if let Some(message) = headless_chat_smoke_message(&args) {
        return run_headless_chat_smoke(message);
    }
    if let Some(frames) = resize_render_benchmark_frames(&args) {
        return run_resize_render_benchmark(frames);
    }
    if let Some(frames) = scroll_render_benchmark_frames(&args) {
        return run_scroll_render_benchmark(frames);
    }
    if let Some(output_dir) = hero_screenshot_capture_dir(&args) {
        return run_hero_screenshot_capture(&output_dir).await;
    }
    if let Some(raw_events) = stream_e2e_benchmark_raw_events(&args) {
        return run_stream_e2e_benchmark(raw_events);
    }
    if desktop_gallery::launcher_requested(&args) {
        return desktop_gallery::launch_temporary_windows();
    }
    let fullscreen = args.iter().any(|arg| arg == "--fullscreen");
    let desktop_gallery_state = desktop_gallery::state_from_args(&args);
    let desktop_gallery = desktop_gallery_state.is_some();
    let process_role = desktop_process_role_from_args(args.iter().map(String::as_str));
    let desktop_mode = desktop_mode_from_args(args.iter().map(String::as_str));
    if process_role == DesktopProcessRole::AppWorker {
        return run_desktop_app_worker_process(desktop_mode);
    }
    let resume_session_id = desktop_resume_session_id_from_args(args.iter().map(String::as_str));
    let desktop_reload_startup = DesktopReloadStartup::from_env();
    emit_desktop_profile_event(
        "jcode-desktop-launch-profile",
        serde_json::json!({
            "mode": desktop_mode.as_str(),
            "process_role": process_role.as_str(),
            "version": desktop_header_version_label(),
            "build_hash": desktop_build_hash_label(),
            "pid": std::process::id(),
        }),
    );
    let event_loop = EventLoopBuilder::<DesktopUserEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    let event_loop_proxy = event_loop.create_proxy();
    startup_trace.mark("event loop created");
    let mut window_builder = WindowBuilder::new().with_title("Jcode Desktop");
    if let Some(placement) = desktop_reload_startup.window_placement {
        window_builder = placement.apply_to_window_builder(window_builder);
    } else {
        window_builder = window_builder.with_inner_size(LogicalSize::new(
            DEFAULT_WINDOW_WIDTH,
            DEFAULT_WINDOW_HEIGHT,
        ));
    }

    if desktop_reload_startup.hidden_until_handoff_release() {
        window_builder = window_builder.with_visible(false);
    }

    if fullscreen {
        window_builder = window_builder.with_fullscreen(Some(Fullscreen::Borderless(None)));
    }

    let window = Arc::new(
        window_builder
            .build(&event_loop)
            .context("failed to create desktop window")?,
    );
    startup_trace.mark("window created");
    let mut renderer = DesktopHostRendererState::NoGpuBoot;
    renderer.start_gpu_init(window.clone(), event_loop_proxy.clone(), startup_trace)?;
    startup_trace.mark("canvas init spawned");

    let mut pending_workspace_startup_load = false;
    let mut pending_workspace_startup_preferences = None;
    let mut app = if let Some(gallery_state) = desktop_gallery_state.as_deref() {
        desktop_gallery::temporary_app(gallery_state)
    } else if desktop_mode == DesktopMode::WorkspacePrototype {
        let mut workspace = Workspace::loading_sessions();
        if let Some(preferences) = load_desktop_preferences() {
            workspace.apply_preferences(preferences.clone());
            pending_workspace_startup_preferences = Some(preferences);
        }
        pending_workspace_startup_load = true;
        DesktopApp::Workspace(workspace)
    } else {
        initial_single_session_app(resume_session_id.as_deref())
    };
    startup_trace.mark("app state initialized");
    window.set_title(&app.status_title());
    let mut reload_startup_handoff = desktop_reload_startup.handoff;
    let mut modifiers = ModifiersState::empty();
    let mut cursor_position = winit::dpi::PhysicalPosition::new(0.0, 0.0);
    let mut selecting_body = false;
    let mut selecting_draft = false;
    let mut scroll_accumulator = ScrollLineAccumulator::default();
    let mut scroll_metrics_cache = SingleSessionScrollMetricsCache::default();
    let mut hot_reloader = DesktopHotReloader::new(process_role.reload_strategy());
    if process_role == DesktopProcessRole::StableHost {
        hot_reloader.start_app_worker_for_current_binary(&app, &window, "stable host startup");
    }
    let preferences_save_tx = spawn_desktop_preferences_saver();
    let mut power_inhibitor = power_inhibit::PowerInhibitor::new();
    let (session_event_tx, session_event_rx) = mpsc::channel();
    spawn_session_event_forwarder(session_event_rx, event_loop_proxy.clone());
    let reasoning_effort_queue = spawn_desktop_reasoning_effort_request_queue()?;
    let mut recovery_scan_pending = app.is_single_session() && !desktop_gallery;
    let mut first_frame_presented = false;
    let mut first_content_frame_presented = false;
    let mut interaction_latency = DesktopInteractionLatencyProfiler::new();
    let mut no_paint_watchdog = DesktopNoPaintWatchdog::new();
    let mut last_backend_redraw_request: Option<Instant> = None;
    let mut pending_backend_redraw_since: Option<Instant> = None;
    let mut surface_timeout_backoff = SurfaceTimeoutBackoff::default();
    let mut surface_timeout_redraw_at: Option<Instant> = None;
    let mut pending_resize: Option<PhysicalSize<u32>> = None;
    let mut space_hold_started_at: Option<Instant> = None;
    let mut space_hold_consumed = false;
    let mut github_issue_sync_running = false;
    let mut desktop_clipboard = DesktopClipboard::default();

    if pending_workspace_startup_load {
        spawn_session_cards_load(
            DesktopSessionCardsPurpose::WorkspaceInitialLoad,
            event_loop_proxy.clone(),
            Duration::ZERO,
        );
    }

    let mut event_loop_entered = false;
    event_loop.run(move |event, target| {
        if !event_loop_entered {
            event_loop_entered = true;
            startup_trace.mark("event loop entered");
        }
        let event_loop_now = Instant::now();
        let surface_renderable = desktop_surface_size_is_renderable(window.inner_size());
        let renderer_ready = renderer.is_gpu_ready();
        let has_background_work = app.has_background_work();
        power_inhibitor.set_active(has_background_work);
        let default_wake = desktop_background_wake(
            event_loop_now,
            surface_renderable,
            app.has_frame_animation(),
        );
        let backend_wake = pending_backend_redraw_since
            .and(last_backend_redraw_request)
            .map(|last| last + BACKEND_REDRAW_FRAME_INTERVAL);
        let hot_reload_wake = hot_reloader.next_wake(event_loop_now);
        let space_hold_wake = space_hold_started_at.and_then(|started_at| match &app {
            DesktopApp::Workspace(workspace) if !space_hold_consumed => {
                Some(started_at + workspace.space_hold_toggle_duration())
            }
            _ => None,
        });
        let wake = [
            default_wake,
            backend_wake,
            hot_reload_wake,
            space_hold_wake,
            surface_timeout_redraw_at,
        ]
            .into_iter()
            .flatten()
            .min();
        if let Some(wake) = wake {
            target.set_control_flow(ControlFlow::WaitUntil(wake));
        } else {
            target.set_control_flow(ControlFlow::Wait);
        }

        let pending_interaction_kind = interaction_latency.pending_kind();
        let frame_animation_active = app.has_frame_animation();
        let pending_backend_redraw = pending_backend_redraw_since.is_some();
        let no_paint_active = surface_renderable
            && renderer_ready
            && (!first_frame_presented
                || has_background_work
                || frame_animation_active
                || pending_backend_redraw
                || pending_interaction_kind.is_some());
        if no_paint_watchdog.observe_active_tick(
            event_loop_now,
            NoPaintWatchdogContext {
                active: no_paint_active,
                mode: app.mode(),
                has_background_work,
                frame_animation_active,
                pending_backend_redraw,
                pending_interaction_kind,
            },
        ) {
            window.request_redraw();
        }
        let worker_drain = hot_reloader.drain_app_worker_messages();
        if let Some(scene) = worker_drain.latest_scene {
            // Keep receiving worker scenes so the IPC path stays exercised, but do
            // not make them primary yet. The worker currently emits only the
            // display-list skeleton, while the in-process host renderer still owns
            // the complete desktop UI. Rendering the worker scene here regresses
            // normal launches to a blank/gray window.
            drop(scene);
            window.request_redraw();
        }
        if worker_drain.reload_requested {
            show_desktop_reload_notice(&mut app);
            window.set_title(&app.status_title());
            window.request_redraw();
            if hot_reloader.force_reload(&app, &window) {
                target.exit();
                return;
            }
        }

        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => {
                    pending_resize = Some(size);
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Window(DesktopWindowEvent::Resized {
                            width: size.width,
                            height: size.height,
                            scale_factor: window.scale_factor() as f32,
                        }),
                    );
                    window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    pending_resize = Some(window.inner_size());
                    let size = window.inner_size();
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Window(DesktopWindowEvent::Resized {
                            width: size.width,
                            height: size.height,
                            scale_factor: window.scale_factor() as f32,
                        }),
                    );
                    window.request_redraw();
                }
                WindowEvent::Focused(focused) => {
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Window(DesktopWindowEvent::Focused(focused)),
                    );
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::MouseWheel { delta, phase, .. } => {
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Mouse(desktop_mouse_wheel_event(delta)),
                    );
                    let size = window.inner_size();
                    let now = Instant::now();
                    let previous_smooth_scroll = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        size,
                        &mut scroll_metrics_cache,
                    );
                    let mut should_redraw = false;
                    if !app.is_single_session() {
                        scroll_accumulator.reset();
                        scroll_metrics_cache.clear();
                    } else if let Some(lines) = scroll_accumulator.scroll_lines(delta, now) {
                        should_redraw |=
                            app.scroll_single_session_body(lines, size, &mut scroll_metrics_cache);
                    }
                    if matches!(phase, TouchPhase::Cancelled) {
                        scroll_accumulator.reset();
                    }
                    let next_smooth_scroll = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        size,
                        &mut scroll_metrics_cache,
                    );
                    should_redraw |= (next_smooth_scroll - previous_smooth_scroll).abs()
                        >= SCROLL_FRACTIONAL_EPSILON;
                    if should_redraw {
                        interaction_latency.mark("mouse_wheel", now);
                        window.request_redraw();
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let cursor_started = Instant::now();
                    cursor_position = position;
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Mouse(DesktopMouseEvent::Move {
                            x: cursor_position.x as f32,
                            y: cursor_position.y as f32,
                        }),
                    );
                    if selecting_draft
                        && app.update_single_session_draft_selection_at(
                            cursor_position.x as f32,
                            cursor_position.y as f32,
                            window.inner_size(),
                        )
                    {
                        interaction_latency.mark("draft_selection_drag", cursor_started);
                        window.request_redraw();
                    } else if selecting_body
                        && app.update_single_session_selection_at(
                            cursor_position.x as f32,
                            cursor_position.y as f32,
                            window.inner_size(),
                        )
                    {
                        interaction_latency.mark("body_selection_drag", cursor_started);
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    let mouse_started = Instant::now();
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Mouse(DesktopMouseEvent::Button {
                            button: DesktopMouseButton::Left,
                            pressed: state == ElementState::Pressed,
                        }),
                    );
                    match state {
                        ElementState::Pressed => {
                        if app.begin_single_session_draft_selection_at(
                            cursor_position.x as f32,
                            cursor_position.y as f32,
                            window.inner_size(),
                        ) {
                            selecting_body = false;
                            selecting_draft = true;
                            window.set_title(&app.status_title());
                            interaction_latency.mark("mouse_press", mouse_started);
                            window.request_redraw();
                            return;
                        }

                        selecting_draft = false;
                        selecting_body = app.begin_single_session_selection_at(
                            cursor_position.x as f32,
                            cursor_position.y as f32,
                            window.inner_size(),
                        );
                        if selecting_body {
                            interaction_latency.mark("mouse_press", mouse_started);
                            window.request_redraw();
                        }
                    }
                    ElementState::Released => {
                        if selecting_draft {
                            app.update_single_session_draft_selection_at(
                                cursor_position.x as f32,
                                cursor_position.y as f32,
                                window.inner_size(),
                            );
                            selecting_draft = false;
                            let selected = app.selected_single_session_draft_text();
                            if let Some(text) = selected {
                                copy_text_to_clipboard(
                                    &mut desktop_clipboard,
                                    &text,
                                    "copied input selection",
                                    &mut app,
                                );
                            }
                            window.set_title(&app.status_title());
                            interaction_latency.mark("mouse_release", mouse_started);
                            window.request_redraw();
                        } else if selecting_body {
                            app.update_single_session_selection_at(
                                cursor_position.x as f32,
                                cursor_position.y as f32,
                                window.inner_size(),
                            );
                            selecting_body = false;
                            let selected = app.selected_single_session_text(window.inner_size());
                            if let Some(text) = selected {
                                copy_text_to_clipboard(
                                    &mut desktop_clipboard,
                                    &text,
                                    "copied selection",
                                    &mut app,
                                );
                            }
                            window.set_title(&app.status_title());
                            interaction_latency.mark("mouse_release", mouse_started);
                            window.request_redraw();
                        }
                    }
                    }
                }
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Released => {
                    if app.is_workspace() && is_space_key(&event.logical_key) {
                        if space_hold_started_at.take().is_some()
                            && !space_hold_consumed
                            && matches!(&app, DesktopApp::Workspace(workspace) if workspace.mode == InputMode::Insert)
                            && matches!(app.handle_key(KeyInput::Character(" ".to_string())), KeyOutcome::Redraw)
                        {
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        space_hold_consumed = false;
                    }
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    let keyboard_started = Instant::now();
                    let size = window.inner_size();
                    let had_smooth_scroll = app
                        .single_session_smooth_scroll_lines(
                            scroll_accumulator.pending_lines(),
                            size,
                            &mut scroll_metrics_cache,
                        )
                        .abs()
                        >= SCROLL_FRACTIONAL_EPSILON;
                    scroll_accumulator.reset();
                    if had_smooth_scroll {
                        window.request_redraw();
                    }
                    if app.is_workspace()
                        && is_space_key(&event.logical_key)
                        && modifiers.is_empty()
                    {
                        if space_hold_started_at.is_none() {
                            space_hold_started_at = Some(keyboard_started);
                            space_hold_consumed = false;
                        }
                        window.request_redraw();
                        return;
                    }

                    let key_input = to_key_input(&event.logical_key, modifiers);
                    let key_debug = format!("{key_input:?}");
                    interaction_latency.mark("keyboard_input", keyboard_started);
                    if hot_reloader.has_app_worker() {
                        forward_app_worker_input(
                            &mut hot_reloader,
                            DesktopInputEvent::Key(desktop_key_event_from_winit(
                                &event.logical_key,
                                modifiers,
                                true,
                            )),
                        );
                        window.request_redraw();
                    }
                    if key_input == KeyInput::RefreshSessions && app.is_workspace() {
                        spawn_session_cards_load(
                            DesktopSessionCardsPurpose::WorkspaceRefresh,
                            event_loop_proxy.clone(),
                            Duration::ZERO,
                        );
                        window.request_redraw();
                        return;
                    }

                    match app.handle_key(key_input) {
                        KeyOutcome::Exit => target.exit(),
                        KeyOutcome::Redraw => {
                            if let DesktopApp::Workspace(workspace) = &app {
                                queue_desktop_preferences_save(workspace, &preferences_save_tx);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::OpenSession { session_id, title } => {
                            if let DesktopApp::Workspace(workspace) = &app {
                                queue_desktop_preferences_save(workspace, &preferences_save_tx);
                            }
                            if app.promote_focused_workspace_session() {
                                scroll_accumulator = ScrollLineAccumulator::default();
                                scroll_metrics_cache = SingleSessionScrollMetricsCache::default();
                                window.set_title(&app.status_title());
                                window.request_redraw();
                            } else if let Err(error) =
                                session_launch::launch_validated_resume_session(&session_id, &title)
                            {
                                desktop_log::error(format_args!(
                                    "jcode-desktop: failed to open session {session_id}: {error:#}"
                                ));
                            }
                        }
                        KeyOutcome::SpawnSession => {
                            if let DesktopApp::SingleSession(app) = &mut app {
                                app.reset_fresh_session();
                                window.set_title(&app.status_title());
                                window.request_redraw();
                                return;
                            }

                            if let Err(error) = session_launch::launch_new_session() {
                                desktop_log::error(format_args!(
                                    "jcode-desktop: failed to spawn session: {error:#}"
                                ));
                            } else {
                                spawn_session_cards_load(
                                    DesktopSessionCardsPurpose::WorkspaceRefresh,
                                    event_loop_proxy.clone(),
                                    SESSION_SPAWN_REFRESH_DELAY,
                                );
                                window.request_redraw();
                            }
                        }
                        KeyOutcome::SendDraft {
                            session_id,
                            title,
                            message,
                            images,
                        } => {
                            if app.is_single_session() {
                                match session_launch::spawn_message_to_session(
                                    session_id.clone(),
                                    message,
                                    images,
                                    session_event_tx.clone(),
                                ) {
                                    Ok(handle) => app.set_single_session_handle(handle),
                                    Err(error) => apply_single_session_error(&mut app, error),
                                }
                                window.set_title(&app.status_title());
                                window.request_redraw();
                            } else if !images.is_empty() {
                                match session_launch::spawn_message_to_session(
                                    session_id.clone(),
                                    message,
                                    images,
                                    session_event_tx.clone(),
                                ) {
                                    Ok(_handle) => {
                                        spawn_session_cards_load(
                                            DesktopSessionCardsPurpose::WorkspaceRefresh,
                                            event_loop_proxy.clone(),
                                            SESSION_SPAWN_REFRESH_DELAY,
                                        );
                                        window.request_redraw();
                                    }
                                    Err(error) => desktop_log::error(format_args!(
                                        "jcode-desktop: failed to send image draft to {session_id}: {error:#}"
                                    )),
                                }
                            } else if let Err(error) = session_launch::send_message_to_session(
                                &session_id,
                                &title,
                                &message,
                            ) {
                                desktop_log::error(format_args!(
                                    "jcode-desktop: failed to send draft to {session_id}: {error:#}"
                                ));
                            } else {
                                spawn_session_cards_load(
                                    DesktopSessionCardsPurpose::WorkspaceRefresh,
                                    event_loop_proxy.clone(),
                                    SESSION_SPAWN_REFRESH_DELAY,
                                );
                                window.request_redraw();
                            }
                        }
                        KeyOutcome::StartFreshSession { message, images } => {
                            match session_launch::spawn_fresh_server_session(
                                message,
                                images,
                                session_event_tx.clone(),
                            ) {
                                Ok(handle) => app.set_single_session_handle(handle),
                                Err(error) => apply_single_session_error(&mut app, error),
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CancelGeneration => {
                            app.cancel_single_session_generation();
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CopyLatestResponse(text) => {
                            copy_text_to_clipboard(
                                &mut desktop_clipboard,
                                &text,
                                "copied latest response",
                                &mut app,
                            );
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CopyText {
                            text,
                            success_notice,
                        } => {
                            copy_text_to_clipboard(
                                &mut desktop_clipboard,
                                &text,
                                success_notice,
                                &mut app,
                            );
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CutDraftToClipboard(text) => {
                            copy_text_to_clipboard(
                                &mut desktop_clipboard,
                                &text,
                                "cut input line",
                                &mut app,
                            );
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CycleModel(direction) => {
                            if let Err(error) = session_launch::spawn_cycle_model(
                                direction,
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::SwitchingModel,
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CycleReasoningEffort(direction) => {
                            let target_session_id = app.single_session_live_id();
                            let outcome = app.preview_single_session_reasoning_effort_cycle(direction);
                            match outcome {
                                ReasoningEffortCycleOutcome::Set(effort) => {
                                    if let Err(error) = reasoning_effort_queue.request(
                                        effort,
                                        target_session_id,
                                        session_event_tx.clone(),
                                    ) {
                                        apply_single_session_error(&mut app, error);
                                    }
                                }
                                ReasoningEffortCycleOutcome::AlreadyAtLimit { .. }
                                | ReasoningEffortCycleOutcome::Unavailable => {}
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::LoadModelCatalog => {
                            if let Err(error) = session_launch::spawn_load_model_catalog(
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::LoadSessionSwitcher => {
                            let purpose = if app.is_workspace() {
                                DesktopSessionCardsPurpose::WorkspaceRefresh
                            } else {
                                DesktopSessionCardsPurpose::SingleSessionSwitcher
                            };
                            spawn_session_cards_load(
                                purpose,
                                event_loop_proxy.clone(),
                                Duration::ZERO,
                            );
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::RestoreCrashedSessions => {
                            spawn_restore_crashed_sessions(event_loop_proxy.clone());
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::SetModel(model) => {
                            if let Err(error) = session_launch::spawn_set_model(
                                model,
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::SwitchingModel,
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::RefreshModelCatalog => {
                            if let Err(error) = session_launch::spawn_refresh_models(
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::SetReasoningEffort(effort) => {
                            let target_session_id = app.single_session_live_id();
                            match app.preview_single_session_reasoning_effort_set(&effort) {
                                Some(effort) => {
                                    if app
                                        .set_reasoning_effort_via_active_session(effort.clone())
                                        .is_err()
                                    {
                                        if let Err(error) = reasoning_effort_queue.request(
                                            effort,
                                            target_session_id,
                                            session_event_tx.clone(),
                                        ) {
                                            apply_single_session_error(&mut app, error);
                                        }
                                    }
                                }
                                None => app.set_single_session_status_label(
                                    "thinking level is not available for this model",
                                ),
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::SetServiceTier(service_tier) => {
                            if let Err(error) = session_launch::spawn_set_service_tier(
                                service_tier,
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::external("setting fast mode"),
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::SetTransport(transport) => {
                            if let Err(error) = session_launch::spawn_set_transport(
                                transport,
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::external("setting transport"),
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::SetCompactionMode(mode) => {
                            if let Err(error) = session_launch::spawn_set_compaction_mode(
                                mode,
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::external("setting compaction mode"),
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::CompactSession => {
                            if let Err(error) = session_launch::spawn_compact_session(
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::external("requesting compaction"),
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::RenameSession(title) => {
                            if let Err(error) = session_launch::spawn_rename_session(
                                title,
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::external("renaming session"),
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::ClearServerSession => {
                            if let Err(error) = session_launch::spawn_clear_server_session(
                                app.single_session_live_id(),
                                session_event_tx.clone(),
                            ) {
                                apply_single_session_error(&mut app, error);
                            } else {
                                app.apply_session_event(session_launch::DesktopSessionEvent::Status(
                                    DesktopSessionStatus::external("clearing session"),
                                ));
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::SendStdinResponse { request_id, input } => {
                            if let Err(error) = app.send_single_session_stdin_response(request_id, input)
                            {
                                apply_single_session_error(&mut app, error);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::AttachClipboardImage => {
                            match clipboard_image_png_base64(&mut desktop_clipboard) {
                                Ok((media_type, base64_data)) => {
                                    app.attach_clipboard_image(media_type, base64_data);
                                }
                                Err(error) => apply_single_session_error(&mut app, error),
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::PasteText => {
                            if let Err(error) =
                                paste_clipboard_into_app(&mut desktop_clipboard, &mut app)
                            {
                                apply_single_session_error(&mut app, error);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::ForceReload => {
                            if hot_reloader.force_reload(&app, &window) {
                                target.exit();
                            } else {
                                window.set_title(&app.status_title());
                                window.request_redraw();
                            }
                        }
                        KeyOutcome::None => {}
                    }
                    if start_pending_github_issue_sync(
                        &mut app,
                        &mut github_issue_sync_running,
                        event_loop_proxy.clone(),
                    ) {
                        window.set_title(&app.status_title());
                        window.request_redraw();
                    }
                    log_desktop_slow_interaction(
                        "keyboard_input",
                        keyboard_started.elapsed(),
                        serde_json::json!({ "key": key_debug }),
                    );
                }
                WindowEvent::RedrawRequested => {
                    let Some(canvas) = renderer.canvas_mut() else {
                        return;
                    };
                    if let Some(size) = pending_resize.take() {
                        canvas.resize(size);
                    }
                    let window_size = window.inner_size();
                    if !desktop_surface_size_is_renderable(window_size) {
                        canvas.suspend_for_zero_size(window_size);
                        surface_timeout_backoff.reset();
                        surface_timeout_redraw_at = None;
                        return;
                    }
                    let smooth_scroll_lines = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        window_size,
                        &mut scroll_metrics_cache,
                    );
                    let render_result = canvas.render(
                        &app,
                        window.current_monitor().map(|monitor| monitor.size()),
                        smooth_scroll_lines,
                        workspace_space_hold_progress(
                            &app,
                            space_hold_started_at,
                            space_hold_consumed,
                        ),
                    );
                    match render_result {
                    Ok(frame) => {
                        surface_timeout_backoff.reset();
                        surface_timeout_redraw_at = None;
                        no_paint_watchdog.observe_presented(Instant::now(), &frame);
                        interaction_latency.observe_presented(&frame);
                        if !first_frame_presented {
                            first_frame_presented = true;
                            startup_trace.mark("first frame presented");
                            if startup_benchmark {
                                target.exit();
                                return;
                            }
                            if recovery_scan_pending {
                                recovery_scan_pending = false;
                                spawn_recovery_session_count_scan(
                                    event_loop_proxy.clone(),
                                    startup_trace,
                                );
                            }
                        }
                        if frame.content_ready && !first_content_frame_presented {
                            first_content_frame_presented = true;
                            startup_trace.mark("first content frame presented");
                        }
                        if startup_content_benchmark && frame.content_ready {
                            target.exit();
                            return;
                        }
                        if frame.animation_active {
                            window.request_redraw();
                        }
                    }
                    Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                        surface_timeout_backoff.reset();
                        surface_timeout_redraw_at = None;
                        canvas.resize(window.inner_size());
                        window.request_redraw();
                    }
                    Err(SurfaceError::OutOfMemory) => target.exit(),
                    Err(SurfaceError::Timeout) => {
                        let now = Instant::now();
                        let (delay, consecutive_timeouts) = surface_timeout_backoff.record_timeout();
                        let redraw_at = now + delay;
                        surface_timeout_redraw_at = Some(redraw_at);
                        if consecutive_timeouts == 1 || delay == SURFACE_TIMEOUT_BACKOFF_MAX {
                            desktop_log::warn(format_args!(
                                "jcode-desktop: surface acquire timed out, retrying in {}ms after {} consecutive timeout(s)",
                                delay.as_millis(),
                                consecutive_timeouts
                            ));
                        }
                        target.set_control_flow(ControlFlow::WaitUntil(redraw_at));
                    }
                    }
                }
                _ => {}
            },
            Event::UserEvent(DesktopUserEvent::RecoveryCount(recovery_count)) => {
                if let DesktopApp::SingleSession(single_session) = &mut app {
                    single_session.set_recovery_session_count(recovery_count);
                    window.set_title(&app.status_title());
                    interaction_latency.mark("recovery_count", Instant::now());
                    window.request_redraw();
                }
            }
            Event::UserEvent(DesktopUserEvent::CanvasReady(result)) => {
                let DesktopCanvasInitResult { canvas, elapsed } = *result;
                match canvas {
                    Ok(mut ready_canvas) => {
                        startup_trace.mark(&format!(
                            "canvas ready (async {}ms)",
                            elapsed.as_millis()
                        ));
                        ready_canvas.resize(window.inner_size());
                        renderer = DesktopHostRendererState::GpuReady(Box::new(ready_canvas));
                        if let Some(handoff) = reload_startup_handoff.as_ref() {
                            handoff.signal_ready_and_wait_for_release();
                            window.set_visible(true);
                            startup_trace.mark("reload handoff released");
                        }
                        reload_startup_handoff = None;
                        window.request_redraw();
                    }
                    Err(message) => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: failed to initialize desktop renderer: {message}"
                        ));
                        renderer = DesktopHostRendererState::GpuFailed { _message: message };
                        target.exit();
                    }
                }
            }
            Event::UserEvent(DesktopUserEvent::SessionCardsLoaded {
                purpose,
                cards,
                loaded_in,
            }) => {
                let card_count = cards.len();
                let mut applied = false;
                match purpose {
                    DesktopSessionCardsPurpose::WorkspaceInitialLoad => {
                        if let DesktopApp::Workspace(workspace) = &mut app {
                            workspace.replace_session_cards(cards);
                            if let Some(preferences) = pending_workspace_startup_preferences.take() {
                                workspace.apply_preferences(preferences);
                            }
                            applied = true;
                        }
                    }
                    DesktopSessionCardsPurpose::WorkspaceRefresh => {
                        if let DesktopApp::Workspace(workspace) = &mut app {
                            workspace.replace_session_cards(cards);
                            queue_desktop_preferences_save(workspace, &preferences_save_tx);
                            applied = true;
                        }
                    }
                    DesktopSessionCardsPurpose::SingleSessionSwitcher => {
                        if app.is_single_session() {
                            app.apply_single_session_switcher_cards(cards);
                            applied = true;
                        }
                    }
                }
                log_desktop_session_cards_load_profile(purpose, loaded_in, card_count, applied);
                if applied {
                    window.set_title(&app.status_title());
                    interaction_latency.mark("session_cards_load", Instant::now());
                    window.request_redraw();
                }
            }
            Event::UserEvent(DesktopUserEvent::SessionCardLoaded {
                session_id,
                card,
                loaded_in,
            }) => {
                let card_found = card.is_some();
                let mut applied = false;
                if let DesktopApp::SingleSession(single_session) = &mut app
                    && single_session.live_session_id.as_deref() == Some(session_id.as_str())
                    && let Some(card) = card
                {
                    single_session.replace_session(Some(card));
                    applied = true;
                }
                log_desktop_session_card_refresh_profile(
                    &session_id,
                    loaded_in,
                    card_found,
                    applied,
                );
                if applied {
                    window.set_title(&app.status_title());
                    interaction_latency.mark("session_card_refresh", Instant::now());
                    window.request_redraw();
                }
            }
            Event::UserEvent(DesktopUserEvent::CrashedSessionsRestoreFinished {
                restored,
                errors,
                elapsed,
            }) => {
                log_desktop_crashed_sessions_restore_profile(restored, errors.len(), elapsed);
                if restored == 0 {
                    let message = if errors.is_empty() {
                        "no crashed sessions found".to_string()
                    } else {
                        format!("failed to restore crashed sessions: {}", errors.join("; "))
                    };
                    apply_single_session_error(&mut app, anyhow::anyhow!(message));
                } else if let DesktopApp::SingleSession(single_session) = &mut app {
                    single_session.set_recovery_session_count(0);
                    single_session.set_status_label(format!("restored {restored} crashed session(s)"));
                }
                window.set_title(&app.status_title());
                interaction_latency.mark("restore_crashed_sessions", Instant::now());
                window.request_redraw();
            }
            Event::UserEvent(DesktopUserEvent::GitHubIssuesSyncFinished(result)) => {
                github_issue_sync_running = false;
                app.apply_github_issue_sync_result(result);
                window.set_title(&app.status_title());
                interaction_latency.mark("github_issue_sync", Instant::now());
                window.request_redraw();
            }
            Event::UserEvent(DesktopUserEvent::SessionEvents(batch)) => {
                let ui_received_at = Instant::now();
                let accumulated_for = batch.accumulated_for();
                let raw_event_count = batch.raw_event_count;
                let raw_payload_bytes = batch.raw_payload_bytes;
                let forwarded_at = batch.forwarded_at;
                forward_desktop_session_event_batch_to_worker(&mut hot_reloader, &batch);
                let apply_stats = apply_desktop_session_event_batch_with_stats(&mut app, batch.events);
                let ui_queue_delay = ui_received_at.saturating_duration_since(forwarded_at);
                let mut redraw_requested = false;
                let mut redraw_deferred = false;
                let mut session_card_refresh_spawned = false;
                if apply_stats.visible_changed {
                    let now = Instant::now();
                    if apply_stats.session_card_refresh_requested
                        && let Some(session_id) = app.single_session_live_id()
                    {
                        spawn_single_session_card_refresh(session_id, event_loop_proxy.clone());
                        session_card_refresh_spawned = true;
                    }
                    if let Some((message, images)) = app.take_next_queued_single_session_draft() {
                        let result = if let Some(session_id) = app.single_session_live_id() {
                            session_launch::spawn_message_to_session(
                                session_id,
                                message,
                                images,
                                session_event_tx.clone(),
                            )
                        } else {
                            session_launch::spawn_fresh_server_session(
                                message,
                                images,
                                session_event_tx.clone(),
                            )
                        };
                        match result {
                            Ok(handle) => app.set_single_session_handle(handle),
                            Err(error) => apply_single_session_error(&mut app, error),
                        }
                    }
                    window.set_title(&app.status_title());
                    let redraw_due = last_backend_redraw_request.is_none_or(|last| {
                        now.saturating_duration_since(last) >= BACKEND_REDRAW_FRAME_INTERVAL
                    });
                    if redraw_due {
                        let first_pending = pending_backend_redraw_since.take().unwrap_or(now);
                        interaction_latency.mark("backend_events", first_pending);
                        last_backend_redraw_request = Some(now);
                        window.request_redraw();
                        redraw_requested = true;
                    } else {
                        pending_backend_redraw_since.get_or_insert(now);
                        redraw_deferred = true;
                    }
                }
                log_desktop_session_event_batch_profile(
                    raw_event_count,
                    raw_payload_bytes,
                    accumulated_for,
                    ui_queue_delay,
                    &apply_stats,
                    redraw_requested,
                    redraw_deferred,
                    session_card_refresh_spawned,
                );
            }
            Event::AboutToWait => {
                let surface_renderable = desktop_surface_size_is_renderable(window.inner_size());
                if let Some(redraw_at) = surface_timeout_redraw_at {
                    let now = Instant::now();
                    if now >= redraw_at {
                        surface_timeout_redraw_at = None;
                        if surface_renderable {
                            window.request_redraw();
                        }
                    }
                }
                if surface_renderable && app.is_single_session() {
                    let about_to_wait_started = Instant::now();
                    let size = window.inner_size();
                    let previous_smooth_scroll = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        size,
                        &mut scroll_metrics_cache,
                    );
                    let frame = scroll_accumulator.frame(Instant::now());
                    if let Some(lines) = frame.scroll_lines
                        && !app.scroll_single_session_body(lines, size, &mut scroll_metrics_cache)
                    {
                        scroll_accumulator.stop();
                    }
                    let next_smooth_scroll = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        size,
                        &mut scroll_metrics_cache,
                    );
                    if frame.active
                        || (next_smooth_scroll - previous_smooth_scroll).abs()
                            >= SCROLL_FRACTIONAL_EPSILON
                    {
                        interaction_latency.mark("scroll_momentum", about_to_wait_started);
                        window.request_redraw();
                    }
                } else if scroll_accumulator.is_active() {
                    scroll_accumulator.reset();
                    scroll_metrics_cache.clear();
                }
                if let (DesktopApp::Workspace(workspace), Some(started_at)) = (&mut app, space_hold_started_at)
                    && !space_hold_consumed
                {
                    let now = Instant::now();
                    if now.saturating_duration_since(started_at) >= workspace.space_hold_toggle_duration() {
                        space_hold_consumed = true;
                        if matches!(workspace.handle_key(KeyInput::ToggleInputMode), KeyOutcome::Redraw) {
                            window.set_title(&app.status_title());
                        }
                    }
                    if surface_renderable {
                        window.request_redraw();
                    }
                }
                if let Some(first_pending_backend_redraw) = pending_backend_redraw_since {
                    let now = Instant::now();
                    if surface_renderable
                        && last_backend_redraw_request.is_none_or(|last| {
                            now.saturating_duration_since(last) >= BACKEND_REDRAW_FRAME_INTERVAL
                        })
                    {
                        pending_backend_redraw_since = None;
                        interaction_latency.mark("backend_events", first_pending_backend_redraw);
                        last_backend_redraw_request = Some(now);
                        window.request_redraw();
                    }
                }
                if hot_reloader.poll(&app, &window) {
                    target.exit();
                    return;
                }

                if let Some(canvas) = renderer.canvas_mut()
                    && surface_renderable
                    && canvas.needs_initial_frame
                {
                    canvas.needs_initial_frame = false;
                    window.request_redraw();
                } else if surface_renderable && app.has_frame_animation() {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    })?;

    Ok(())
}

fn load_session_cards_for_desktop() -> Vec<workspace::SessionCard> {
    match session_data::load_recent_session_cards() {
        Ok(cards) => cards,
        Err(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to load session metadata: {error:#}"
            ));
            Vec::new()
        }
    }
}

fn load_crashed_session_cards_for_desktop() -> Vec<workspace::SessionCard> {
    match session_data::load_crashed_session_cards() {
        Ok(cards) => cards,
        Err(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to load crashed session metadata: {error:#}"
            ));
            Vec::new()
        }
    }
}

fn spawn_recovery_session_count_scan(
    event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
    startup_trace: DesktopStartupTrace,
) {
    if let Err(error) = spawn_bounded_desktop_async_job("jcode-desktop-recovery-scan", move || {
        startup_trace.mark("recovery scan started");
        let recovery_count = load_crashed_session_cards_for_desktop().len();
        startup_trace.mark(&format!(
            "recovery scan completed ({recovery_count} crashed)"
        ));
        if event_loop_proxy
            .send_event(DesktopUserEvent::RecoveryCount(recovery_count))
            .is_err()
        {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to deliver recovery count, event loop is closed"
            ));
        }
    }) {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to start recovery scan: {error:#}"
        ));
    }
}

fn spawn_single_session_card_refresh(
    session_id: String,
    event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
) {
    if let Err(error) =
        spawn_bounded_desktop_async_job("jcode-desktop-session-card-refresh", move || {
            let started = Instant::now();
            let card = load_session_cards_for_desktop()
                .into_iter()
                .find(|card| card.session_id == session_id);
            let loaded_in = started.elapsed();
            if event_loop_proxy
                .send_event(DesktopUserEvent::SessionCardLoaded {
                    session_id,
                    card,
                    loaded_in,
                })
                .is_err()
            {
                desktop_log::warn(format_args!(
                    "jcode-desktop: failed to deliver session card refresh, event loop is closed"
                ));
            }
        })
    {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to start session card refresh: {error:#}"
        ));
    }
}

fn spawn_session_cards_load(
    purpose: DesktopSessionCardsPurpose,
    event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
    delay: Duration,
) {
    if let Err(error) = spawn_bounded_desktop_async_job(
        format!("jcode-desktop-session-cards-{purpose:?}"),
        move || {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            let started = Instant::now();
            let cards = load_session_cards_for_desktop();
            let loaded_in = started.elapsed();
            if event_loop_proxy
                .send_event(DesktopUserEvent::SessionCardsLoaded {
                    purpose,
                    cards,
                    loaded_in,
                })
                .is_err()
            {
                desktop_log::warn(format_args!(
                    "jcode-desktop: failed to deliver session cards load, event loop is closed"
                ));
            }
        },
    ) {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to start session card load: {error:#}"
        ));
    }
}

fn spawn_restore_crashed_sessions(event_loop_proxy: EventLoopProxy<DesktopUserEvent>) {
    if let Err(error) = spawn_bounded_desktop_async_job(
        "jcode-desktop-restore-crashed-sessions",
        move || {
            let started = Instant::now();
            let crashed = load_crashed_session_cards_for_desktop();
            let mut restored = 0usize;
            let mut errors = Vec::new();
            for card in crashed {
                match session_launch::launch_validated_resume_session(&card.session_id, &card.title)
                {
                    Ok(()) => restored += 1,
                    Err(error) => errors.push(format!("{}: {error:#}", card.session_id)),
                }
            }
            if event_loop_proxy
                .send_event(DesktopUserEvent::CrashedSessionsRestoreFinished {
                    restored,
                    errors,
                    elapsed: started.elapsed(),
                })
                .is_err()
            {
                desktop_log::warn(format_args!(
                    "jcode-desktop: failed to deliver crashed-session restore result, event loop is closed"
                ));
            }
        },
    ) {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to start crashed-session restore: {error:#}"
        ));
    }
}

fn spawn_github_issue_sync(event_loop_proxy: EventLoopProxy<DesktopUserEvent>) -> Result<()> {
    spawn_bounded_desktop_async_job("jcode-desktop-github-issues-sync", move || {
        let result = desktop_issue_cache::sync_current_repo_issue_cache()
            .map_err(|error| format!("{error:#}"));
        match &result {
            Ok(summary) => desktop_log::info(format_args!(
                "jcode-desktop: synced {} GitHub issue(s) for {} in {}ms to {} (comment_threads={} comment_errors={})",
                summary.issue_count,
                summary.repo,
                summary.elapsed.as_millis(),
                summary.cache_path.display(),
                summary.fetched_comment_threads,
                summary.comment_fetch_errors
            )),
            Err(error) => desktop_log::warn(format_args!(
                "jcode-desktop: GitHub issue sync failed: {error}"
            )),
        }
        if event_loop_proxy
            .send_event(DesktopUserEvent::GitHubIssuesSyncFinished(result))
            .is_err()
        {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to deliver GitHub issue sync result"
            ));
        }
    })
}

fn start_pending_github_issue_sync(
    app: &mut DesktopApp,
    sync_running: &mut bool,
    event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
) -> bool {
    if !app.take_github_issue_sync_request() {
        return false;
    }
    if *sync_running {
        app.note_github_issue_sync_already_running();
        return true;
    }
    match spawn_github_issue_sync(event_loop_proxy) {
        Ok(()) => {
            *sync_running = true;
            true
        }
        Err(error) => {
            app.apply_github_issue_sync_result(Err(format!("{error:#}")));
            true
        }
    }
}

fn spawn_desktop_preferences_saver() -> Option<mpsc::Sender<workspace::DesktopPreferences>> {
    let (tx, rx) = mpsc::channel::<workspace::DesktopPreferences>();
    match std::thread::Builder::new()
        .name("jcode-desktop-preferences-saver".to_string())
        .spawn(move || {
            while let Ok(mut preferences) = rx.recv() {
                let received_at = Instant::now();
                let mut coalesced_saves = 1usize;
                while let Ok(next_preferences) = rx.try_recv() {
                    preferences = next_preferences;
                    coalesced_saves += 1;
                }
                save_desktop_preferences_off_ui_thread(
                    preferences,
                    coalesced_saves,
                    received_at.elapsed(),
                );
            }
        }) {
        Ok(_) => Some(tx),
        Err(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to start preferences saver: {error:#}"
            ));
            None
        }
    }
}

fn queue_desktop_preferences_save(
    workspace: &Workspace,
    preferences_save_tx: &Option<mpsc::Sender<workspace::DesktopPreferences>>,
) {
    let preferences = workspace.preferences();
    if let Some(tx) = preferences_save_tx
        && tx.send(preferences.clone()).is_ok()
    {
        return;
    }

    if let Err(error) =
        spawn_bounded_desktop_async_job("jcode-desktop-preferences-save-once", move || {
            save_desktop_preferences_off_ui_thread(preferences, 1, Duration::ZERO);
        })
    {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to queue preferences save: {error:#}"
        ));
    }
}

fn save_desktop_preferences_off_ui_thread(
    preferences: workspace::DesktopPreferences,
    coalesced_saves: usize,
    queued_for: Duration,
) {
    let started = Instant::now();
    let error = desktop_prefs::save_preferences(&preferences)
        .err()
        .map(|error| format!("{error:#}"));
    log_desktop_preferences_save_profile(
        started.elapsed(),
        queued_for,
        coalesced_saves,
        error.as_deref(),
    );
}

fn headless_chat_smoke_message(args: &[String]) -> Option<String> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--headless-chat-smoke=")
            .map(ToOwned::to_owned)
            .or_else(|| {
                (arg == "--headless-chat-smoke")
                    .then(|| args.get(index + 1).cloned())
                    .flatten()
            })
    })
}

const DESKTOP_HELP_LINES: &[&str] = &[
    "Jcode Desktop",
    "",
    "Usage:",
    "  jcode-desktop [OPTIONS]",
    "",
    "Options:",
    "  --fullscreen                 Start borderless fullscreen",
    "  --workspace                  Open the workspace prototype instead of the single-session chat",
    "  --desktop-process-role ROLE  Internal: standalone, host, or worker",
    "  --desktop-host               Internal alias for --desktop-process-role=host",
    "  --desktop-app-worker         Internal alias for --desktop-process-role=worker",
    "  --startup-log                Print launch timing milestones to stderr",
    "  --startup-benchmark          Print launch timings and exit after the first frame",
    "  --capture-hero-animation DIR Write deterministic hero animation PNG frames and exit",
    "  --resize-render-benchmark[N]  Print CPU resize/render benchmark JSON and exit",
    "  --scroll-render-benchmark[N]  Print CPU scroll/render benchmark JSON and exit",
    "  --stream-e2e-benchmark[N]     Print stream event-to-paint guardrail JSON and exit",
    "  --headless-chat-smoke <MSG>  Run a hidden backend smoke test and print JSON events",
    "  --headless-chat-smoke=<MSG>  Same as above",
    "  -V, --version                Print version information",
    "  -h, --help                   Print this help",
    "",
];

fn desktop_help_text() -> String {
    DESKTOP_HELP_LINES.join("\n")
}

fn hero_screenshot_capture_dir(args: &[String]) -> Option<PathBuf> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--capture-hero-animation=")
            .map(PathBuf::from)
            .or_else(|| {
                (arg == "--capture-hero-animation")
                    .then(|| args.get(index + 1).map(PathBuf::from))
                    .flatten()
            })
    })
}

async fn run_hero_screenshot_capture(output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create hero screenshot directory {}",
            output_dir.display()
        )
    })?;

    let app = SingleSessionApp::new(None);
    let size = PhysicalSize::new(DEFAULT_WINDOW_WIDTH as u32, DEFAULT_WINDOW_HEIGHT as u32);
    let (target_image, _) = render_hero_frame_to_image(&app, size, 0, 1.0, true).await?;
    let target_path = output_dir.join("hero-font-target.png");
    target_image
        .save(&target_path)
        .with_context(|| format!("failed to save {}", target_path.display()))?;
    let frames = [0_u64, 150, 300, 450, 675, 900, 1125, 1350];
    let mut manifest = Vec::new();
    for elapsed_ms in frames {
        let progress = welcome_hero_reveal_progress_for_elapsed(Duration::from_millis(elapsed_ms));
        let tick = elapsed_ms / DESKTOP_SPINNER_FRAME_MS as u64;
        let (image, vertices_len) =
            render_hero_frame_to_image(&app, size, tick, progress, false).await?;
        let filename = format!("hero-{elapsed_ms:04}ms.png");
        let path = output_dir.join(&filename);
        image
            .save(&path)
            .with_context(|| format!("failed to save {}", path.display()))?;
        manifest.push(serde_json::json!({
            "file": filename,
            "elapsed_ms": elapsed_ms,
            "progress": progress,
            "vertices": vertices_len,
        }));
    }

    let manifest_path = output_dir.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .context("failed to serialize hero frame manifest")?;
    std::fs::write(&manifest_path, manifest_json)
        .with_context(|| format!("failed to save {}", manifest_path.display()))?;
    println!(
        "{}",
        serde_json::json!({
            "output_dir": output_dir,
            "font_target": "hero-font-target.png",
            "frames": manifest,
        })
    );
    Ok(())
}

async fn render_hero_frame_to_image(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    spinner_tick: u64,
    welcome_hero_reveal_progress: f32,
    font_target_only: bool,
) -> Result<(RgbaImage, usize)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .context("failed to find a GPU adapter for hero capture")?;
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("jcode-desktop-hero-capture-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        )
        .await
        .context("failed to create GPU device for hero capture")?;

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("jcode-desktop-hero-capture-primitive-shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("jcode-desktop-hero-capture-pipeline-layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("jcode-desktop-hero-capture-primitive-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[Vertex::layout()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
    });

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("jcode-desktop-hero-capture-texture"),
        size: wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut font_system = create_desktop_font_system();
    let mut swash_cache = SwashCache::new();
    let mut text_atlas = TextAtlas::new(&device, &queue, format);
    let mut text_renderer = TextRenderer::new(
        &mut text_atlas,
        &device,
        wgpu::MultisampleState::default(),
        None,
    );
    let mut hero_mask_renderer = HeroMaskRenderer::new(&device, format);

    let rendered_body_lines = single_session_rendered_body_lines_for_tick(app, size, spinner_tick);
    let text_key = single_session_text_key_for_tick_with_rendered_body(
        app,
        size,
        spinner_tick,
        0.0,
        &rendered_body_lines,
    );
    let text_buffers = single_session_text_buffers_from_key(&text_key, size, &mut font_system);
    let viewport = single_session_body_viewport_from_lines(app, size, 0.0, &rendered_body_lines);
    let hero_mask_spec = if font_target_only {
        welcome_hero_runtime_mask_spec_for_phrase(
            &app.welcome_hero_text(),
            size,
            app.text_scale(),
            0.0,
        )
    } else {
        welcome_hero_runtime_mask_spec_for_total_lines(app, size, 0.0, rendered_body_lines.len())
    };
    let text_areas = if font_target_only {
        Vec::new()
    } else {
        single_session_text_areas_for_app_with_cached_body_viewport_and_reveal(
            app,
            &text_buffers,
            size,
            0.0,
            viewport,
            welcome_hero_reveal_progress,
        )
    };
    let has_text_areas = !text_areas.is_empty();
    if has_text_areas {
        text_renderer
            .prepare(
                &device,
                &queue,
                &mut font_system,
                &mut text_atlas,
                Resolution {
                    width: size.width,
                    height: size.height,
                },
                text_areas,
                &mut swash_cache,
            )
            .context("failed to prepare hero capture text")?;
    }

    let vertices = if font_target_only {
        let mut vertices = build_single_session_vertices_with_cached_body(
            app,
            size,
            0.0,
            spinner_tick,
            0.0,
            0.0,
            &rendered_body_lines,
        );
        vertices.truncate(0);
        push_gradient_rect(
            &mut vertices,
            Rect {
                x: 0.0,
                y: 0.0,
                width: size.width as f32,
                height: size.height as f32,
            },
            BACKGROUND_TOP_LEFT,
            BACKGROUND_BOTTOM_LEFT,
            BACKGROUND_BOTTOM_RIGHT,
            BACKGROUND_TOP_RIGHT,
            size,
        );
        vertices
    } else {
        build_single_session_vertices_with_cached_body(
            app,
            size,
            0.0,
            spinner_tick,
            0.0,
            welcome_hero_reveal_progress,
            &rendered_body_lines,
        )
    };
    let hero_mask_prepared = hero_mask_renderer.prepare(
        &device,
        &queue,
        size,
        hero_mask_spec.as_ref(),
        welcome_hero_reveal_progress,
    );
    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("jcode-desktop-hero-capture-vertices"),
        size: (vertices.len() * std::mem::size_of::<Vertex>()) as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&vertices));

    let bytes_per_pixel = 4u32;
    let unpadded_bytes_per_row = size.width * bytes_per_pixel;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let output_buffer_size = padded_bytes_per_row as u64 * size.height as u64;
    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("jcode-desktop-hero-capture-readback"),
        size: output_buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("jcode-desktop-hero-capture-encoder"),
    });
    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("jcode-desktop-hero-capture-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        render_pass.set_pipeline(&render_pipeline);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.draw(0..vertices.len() as u32, 0..1);
        if hero_mask_prepared {
            hero_mask_renderer.render_prepared(&mut render_pass);
        }
        if has_text_areas {
            text_renderer
                .render(&text_atlas, &mut render_pass)
                .context("failed to render hero capture text")?;
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &output_buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size.height),
            },
        },
        wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let buffer_slice = output_buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        if tx.send(result).is_err() {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to deliver hero capture readback result"
            ));
        }
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .context("hero capture readback channel closed")?
        .context("failed to map hero capture readback buffer")?;
    let mapped = buffer_slice.get_mapped_range();
    let mut pixels = vec![0_u8; (unpadded_bytes_per_row * size.height) as usize];
    for y in 0..size.height as usize {
        let src_start = y * padded_bytes_per_row as usize;
        let dst_start = y * unpadded_bytes_per_row as usize;
        pixels[dst_start..dst_start + unpadded_bytes_per_row as usize]
            .copy_from_slice(&mapped[src_start..src_start + unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    output_buffer.unmap();
    let image = RgbaImage::from_raw(size.width, size.height, pixels)
        .context("failed to construct hero capture image")?;
    Ok((image, vertices.len()))
}

enum DesktopUserEvent {
    CanvasReady(Box<DesktopCanvasInitResult>),
    SessionEvents(DesktopSessionEventBatch),
    SessionCardsLoaded {
        purpose: DesktopSessionCardsPurpose,
        cards: Vec<workspace::SessionCard>,
        loaded_in: Duration,
    },
    SessionCardLoaded {
        session_id: String,
        card: Option<workspace::SessionCard>,
        loaded_in: Duration,
    },
    CrashedSessionsRestoreFinished {
        restored: usize,
        errors: Vec<String>,
        elapsed: Duration,
    },
    GitHubIssuesSyncFinished(
        std::result::Result<desktop_issue_cache::GitHubIssueSyncSummary, String>,
    ),
    RecoveryCount(usize),
}

struct DesktopCanvasInitResult {
    canvas: std::result::Result<Canvas, String>,
    elapsed: Duration,
}

enum DesktopHostRendererState {
    NoGpuBoot,
    GpuInitializing { _started_at: Instant },
    GpuReady(Box<Canvas>),
    GpuFailed { _message: String },
}

impl DesktopHostRendererState {
    fn start_gpu_init(
        &mut self,
        window: Arc<Window>,
        event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
        startup_trace: DesktopStartupTrace,
    ) -> Result<()> {
        if matches!(self, Self::GpuInitializing { .. } | Self::GpuReady(_)) {
            return Ok(());
        }

        let started_at = Instant::now();
        std::thread::Builder::new()
            .name("jcode-desktop-gpu-init".to_string())
            .spawn(move || {
                startup_trace.mark("canvas init started");
                let canvas = pollster::block_on(Canvas::new(window, startup_trace))
                    .map_err(|error| format!("{error:#}"));
                let result = DesktopCanvasInitResult {
                    canvas,
                    elapsed: started_at.elapsed(),
                };
                if event_loop_proxy
                    .send_event(DesktopUserEvent::CanvasReady(Box::new(result)))
                    .is_err()
                {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: failed to deliver async canvas initialization result"
                    ));
                }
            })
            .context("failed to spawn desktop GPU initialization thread")?;
        *self = Self::GpuInitializing {
            _started_at: started_at,
        };
        Ok(())
    }

    fn is_gpu_ready(&self) -> bool {
        matches!(self, Self::GpuReady(_))
    }

    fn canvas_mut(&mut self) -> Option<&mut Canvas> {
        match self {
            Self::GpuReady(canvas) => Some(canvas.as_mut()),
            Self::NoGpuBoot | Self::GpuInitializing { .. } | Self::GpuFailed { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopSessionCardsPurpose {
    WorkspaceInitialLoad,
    WorkspaceRefresh,
    SingleSessionSwitcher,
}

fn run_headless_chat_smoke(message: String) -> Result<()> {
    if message.trim().is_empty() {
        anyhow::bail!("headless chat smoke message cannot be empty");
    }

    let (event_tx, event_rx) = mpsc::channel();
    let _handle = session_launch::spawn_fresh_server_session(message, Vec::new(), event_tx)
        .context("failed to start desktop headless chat smoke")?;
    let started = Instant::now();
    let mut session_id = None;
    let mut response = String::new();
    let mut last_status = None;

    while started.elapsed() < HEADLESS_CHAT_SMOKE_TIMEOUT {
        let remaining = HEADLESS_CHAT_SMOKE_TIMEOUT.saturating_sub(started.elapsed());
        let poll = remaining.min(Duration::from_millis(250));
        let event = match event_rx.recv_timeout(poll) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!(
                    "desktop chat smoke worker disconnected before completion; last_status={}",
                    last_status.as_deref().unwrap_or("unknown")
                );
            }
        };

        match event {
            session_launch::DesktopSessionEvent::Status(status) => {
                let status = status.label();
                last_status = Some(status.clone());
                println!(
                    "{}",
                    serde_json::json!({"event": "status", "status": status})
                );
            }
            session_launch::DesktopSessionEvent::SessionStarted { session_id: id } => {
                session_id = Some(id.clone());
                println!(
                    "{}",
                    serde_json::json!({"event": "session", "session_id": id})
                );
            }
            session_launch::DesktopSessionEvent::SessionRenamed {
                title,
                display_title,
            } => {
                last_status = Some(if title.is_some() {
                    format!("renamed session to {display_title}")
                } else {
                    format!("cleared session name; title is now {display_title}")
                });
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "session_renamed",
                        "title": title,
                        "display_title": display_title,
                    })
                );
            }
            session_launch::DesktopSessionEvent::Reloaded { session_id: id } => {
                session_id = Some(id.clone());
                last_status = Some("server reconnected".to_string());
                println!(
                    "{}",
                    serde_json::json!({"event": "reloaded", "session_id": id})
                );
            }
            session_launch::DesktopSessionEvent::TextDelta(text) => {
                response.push_str(&text);
                println!(
                    "{}",
                    serde_json::json!({"event": "text_delta", "chars": text.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::TextReplace(text) => {
                response = text;
                println!(
                    "{}",
                    serde_json::json!({"event": "text_replace", "chars": response.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::ToolStarted { id, name } => {
                last_status = Some(format!("preparing tool {name}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_started", "id": id, "name": name})
                );
            }
            session_launch::DesktopSessionEvent::ToolExecuting { id, name } => {
                last_status = Some(format!("using tool {name}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_executing", "id": id, "name": name})
                );
            }
            session_launch::DesktopSessionEvent::ToolInput { id, delta } => {
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_input", "id": id, "chars": delta.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::ToolFinished {
                id,
                name,
                summary,
                is_error,
            } => {
                last_status = Some(if is_error {
                    format!("tool {name} failed")
                } else {
                    format!("tool {name} done")
                });
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "tool_finished",
                        "id": id,
                        "name": name,
                        "summary": summary,
                        "is_error": is_error,
                    })
                );
            }
            session_launch::DesktopSessionEvent::Reloading { new_socket } => {
                last_status = Some("server reloading, reconnecting".to_string());
                println!(
                    "{}",
                    serde_json::json!({"event": "reloading", "new_socket": new_socket})
                );
            }
            session_launch::DesktopSessionEvent::ModelChanged {
                model,
                provider_name,
                error,
            } => {
                if let Some(error) = error {
                    last_status = Some(format!("model switch failed: {error}"));
                    println!(
                        "{}",
                        serde_json::json!({
                            "event": "model_changed",
                            "model": model,
                            "provider_name": provider_name,
                            "error": error,
                        })
                    );
                    continue;
                }
                let label = provider_name
                    .as_deref()
                    .map(|provider| format!("{provider} · {model}"))
                    .unwrap_or_else(|| model.clone());
                last_status = Some(format!("model: {label}"));
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "model_changed",
                        "model": model,
                        "provider_name": provider_name,
                    })
                );
            }
            session_launch::DesktopSessionEvent::ModelCatalog {
                current_model,
                provider_name,
                models,
                ..
            } => {
                last_status = Some(format!("models loaded ({})", models.len()));
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "model_catalog",
                        "current_model": current_model,
                        "provider_name": provider_name,
                        "models": models.len(),
                    })
                );
            }
            session_launch::DesktopSessionEvent::ModelCatalogError { error } => {
                last_status = Some(format!("model picker error: {error}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "model_catalog_error", "error": error})
                );
            }
            session_launch::DesktopSessionEvent::StdinRequest {
                request_id,
                prompt,
                is_password,
                tool_call_id,
            } => {
                last_status = Some("interactive input requested".to_string());
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "stdin_request",
                        "request_id": request_id,
                        "prompt": prompt,
                        "is_password": is_password,
                        "tool_call_id": tool_call_id,
                    })
                );
            }
            session_launch::DesktopSessionEvent::ReloadProgress {
                step,
                message,
                success,
                output,
            } => {
                last_status = Some(format!("reload {step}: {message}"));
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "reload_progress",
                        "step": step,
                        "message": message,
                        "success": success,
                        "output": output,
                    })
                );
            }
            session_launch::DesktopSessionEvent::RuntimeMetadata {
                connection_type,
                status_detail,
                upstream_provider,
            } => {
                if let Some(status_detail) = &status_detail {
                    last_status = Some(status_detail.clone());
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "runtime_metadata",
                        "connection_type": connection_type,
                        "status_detail": status_detail,
                        "upstream_provider": upstream_provider,
                    })
                );
            }
            session_launch::DesktopSessionEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
            } => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "tokens",
                        "input": input,
                        "output": output,
                        "cache_read_input": cache_read_input,
                        "cache_creation_input": cache_creation_input,
                    })
                );
            }
            session_launch::DesktopSessionEvent::SystemNotice { title, message } => {
                last_status = Some(title.clone());
                println!(
                    "{}",
                    serde_json::json!({"event": "system_notice", "title": title, "message": message})
                );
            }
            session_launch::DesktopSessionEvent::SessionCloseRequested { reason } => {
                anyhow::bail!(
                    "desktop chat smoke session close requested; session_id={}; reason={}",
                    session_id.as_deref().unwrap_or("unknown"),
                    reason
                );
            }
            session_launch::DesktopSessionEvent::Done => {
                let response = response.trim().to_string();
                if response.is_empty() {
                    anyhow::bail!(
                        "desktop chat smoke completed without assistant text; session_id={}; last_status={}",
                        session_id.as_deref().unwrap_or("unknown"),
                        last_status.as_deref().unwrap_or("unknown")
                    );
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "ok",
                        "session_id": session_id,
                        "response_chars": response.chars().count(),
                        "response_preview": response.chars().take(240).collect::<String>(),
                    })
                );
                return Ok(());
            }
            session_launch::DesktopSessionEvent::Error(error) => {
                anyhow::bail!(
                    "desktop chat smoke failed; session_id={}; error={}",
                    session_id.as_deref().unwrap_or("unknown"),
                    error
                );
            }
        }
    }

    anyhow::bail!(
        "desktop chat smoke timed out after {:?}; session_id={}; response_chars={}; last_status={}",
        HEADLESS_CHAT_SMOKE_TIMEOUT,
        session_id.as_deref().unwrap_or("unknown"),
        response.chars().count(),
        last_status.as_deref().unwrap_or("unknown")
    )
}

fn run_resize_render_benchmark(frames: usize) -> Result<()> {
    let frames = frames.max(1);
    let target_p95_ms = 16.0;
    let target_max_ms = 33.0;
    let base_size = PhysicalSize::new(1200, 760);
    let mut app = desktop_large_transcript_benchmark_app();
    let initial_body_lines = single_session_rendered_body_lines_for_tick(&app, base_size, 0);
    if let Some(metrics) = single_session_body_scroll_metrics_for_total_lines(
        &app,
        base_size,
        initial_body_lines.len(),
    ) {
        app.body_scroll_lines = metrics.max_scroll_lines as f32 / 2.0;
    }
    let sizes = (0..frames).map(benchmark_resize_size).collect::<Vec<_>>();

    let mut legacy_font_system = benchmark_font_system();
    let (legacy_samples, legacy_checksum) = benchmark_frame_samples(frames, |frame| {
        let size = sizes[frame];
        let tick = frame as u64;
        let key = single_session_text_key_for_tick_with_scroll(&app, size, tick, 0.0);
        let buffers = single_session_text_buffers_from_key(&key, size, &mut legacy_font_system);
        let areas = single_session_text_areas_for_app_with_scroll(&app, &buffers, size, tick, 0.0);
        let body_glyphs = buffers
            .get(1)
            .map(|buffer| {
                buffer
                    .layout_runs()
                    .map(|run| run.glyphs.len())
                    .sum::<usize>()
            })
            .unwrap_or_default();
        let vertices =
            build_single_session_vertices_with_scroll_and_reveal(&app, size, 0.0, tick, 0.0, 1.0);
        key.body.len() ^ buffers.len() ^ areas.len() ^ vertices.len() ^ body_glyphs
    });

    let mut optimized_font_system = benchmark_font_system();
    let mut optimized_raw_body_key = Some(app.rendered_body_cache_key((0, 0)));
    let mut optimized_raw_body_lines = app.body_styled_lines_for_tick(0);
    let mut optimized_body_key = None;
    let mut optimized_body_lines = Vec::new();
    let mut optimized_text_cache_key = None;
    let mut optimized_text_key = None;
    let mut optimized_buffers: Vec<Buffer> = Vec::new();
    let mut optimized_window_start = None;
    let mut optimized_window_end = None;
    let mut optimized_body_rebuilds = 0usize;
    let mut optimized_body_wraps = 0usize;
    let (optimized_samples, optimized_checksum) = benchmark_frame_samples(frames, |frame| {
        let size = sizes[frame];
        let tick = frame as u64;
        let body_layout_size = single_session_body_layout_cache_size(&app, size);
        let body_key = app.rendered_body_cache_key(body_layout_size);
        let rendered_body_changed = if optimized_body_key != Some(body_key) {
            let raw_body_key = app.rendered_body_cache_key((0, 0));
            if optimized_raw_body_key != Some(raw_body_key) {
                optimized_raw_body_lines = app.body_styled_lines_for_tick(tick);
                optimized_raw_body_key = Some(raw_body_key);
            }
            optimized_body_lines = single_session_rendered_body_lines_from_raw_ref(
                &app,
                size,
                &optimized_raw_body_lines,
            );
            optimized_body_key = Some(body_key);
            optimized_window_start = None;
            optimized_window_end = None;
            optimized_body_wraps += 1;
            true
        } else {
            false
        };

        let viewport =
            single_session_body_viewport_from_lines(&app, size, 0.0, &optimized_body_lines);
        let text_cache_key = single_session_text_buffer_cache_key(&app, size, tick, body_key);
        let key = single_session_text_key_for_tick_with_rendered_body(
            &app,
            size,
            tick,
            0.0,
            &optimized_body_lines,
        );
        let text_key_changed = optimized_text_key.as_ref() != Some(&key);
        if optimized_text_cache_key != Some(text_cache_key) || text_key_changed {
            let desired_body_window = single_session_body_text_window_bounds(&viewport);
            let body_window_contains = if let (Some(window_start), Some(window_end)) =
                (optimized_window_start, optimized_window_end)
            {
                single_session_body_text_window_contains(window_start, window_end, &viewport)
            } else {
                false
            };
            let previous_key = optimized_text_key.take();
            let mut old_buffers = std::mem::take(&mut optimized_buffers);
            let body_content_changed_in_buffer =
                rendered_body_changed && app.streaming_response.is_empty();
            let body_layout_compatible = previous_key.as_ref().is_some_and(|previous| {
                single_session_body_text_buffer_layout_compatible(
                    previous.size,
                    size,
                    app.text_scale(),
                )
            });
            let mut can_reuse_body_buffer = old_buffers.len() > 1
                && body_window_contains
                && !body_content_changed_in_buffer
                && body_layout_compatible;
            if old_buffers.len() > 1
                && (!body_window_contains
                    || body_content_changed_in_buffer
                    || !body_layout_compatible)
            {
                let (window_start, window_end) = desired_body_window;
                old_buffers[1] = single_session_body_text_buffer_from_lines(
                    &mut optimized_font_system,
                    &optimized_body_lines[window_start..window_end],
                    size,
                    app.text_scale(),
                );
                optimized_window_start = Some(window_start);
                optimized_window_end = Some(window_end);
                optimized_body_rebuilds += 1;
                can_reuse_body_buffer = true;
            }
            optimized_buffers = single_session_text_buffers_from_key_reusing_unchanged(
                &key,
                previous_key.as_ref(),
                old_buffers,
                can_reuse_body_buffer,
                size,
                &mut optimized_font_system,
            );
            optimized_text_key = Some(key);
            optimized_text_cache_key = Some(text_cache_key);
            if !can_reuse_body_buffer {
                optimized_window_start = None;
                optimized_window_end = None;
            }
        }

        let viewport =
            single_session_body_viewport_from_lines(&app, size, 0.0, &optimized_body_lines);
        if let (Some(window_start), Some(window_end)) =
            (optimized_window_start, optimized_window_end)
            && single_session_body_text_window_contains(window_start, window_end, &viewport)
        {
            if let Some(body_buffer) = optimized_buffers.get_mut(1) {
                body_buffer.set_scroll(
                    viewport
                        .start_line
                        .saturating_sub(window_start)
                        .min(i32::MAX as usize) as i32,
                );
            }
        } else {
            let (window_start, window_end) = single_session_body_text_window_bounds(&viewport);
            if let Some(body_buffer) = optimized_buffers.get_mut(1) {
                *body_buffer = single_session_body_text_buffer_from_lines(
                    &mut optimized_font_system,
                    &optimized_body_lines[window_start..window_end],
                    size,
                    app.text_scale(),
                );
                body_buffer.set_scroll(
                    viewport
                        .start_line
                        .saturating_sub(window_start)
                        .min(i32::MAX as usize) as i32,
                );
                optimized_body_rebuilds += 1;
            }
            optimized_window_start = Some(window_start);
            optimized_window_end = Some(window_end);
        }

        let areas = single_session_text_areas_for_app_with_cached_body_viewport(
            &app,
            &optimized_buffers,
            size,
            0.0,
            viewport,
        );
        let body_glyphs = optimized_buffers
            .get(1)
            .map(|buffer| {
                buffer
                    .layout_runs()
                    .map(|run| run.glyphs.len())
                    .sum::<usize>()
            })
            .unwrap_or_default();
        let vertices = build_single_session_vertices_with_cached_body(
            &app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
            &optimized_body_lines,
        );
        optimized_body_lines.len()
            ^ optimized_buffers.len()
            ^ areas.len()
            ^ vertices.len()
            ^ body_glyphs
    });

    let optimized_p95 = percentile_ms(&optimized_samples, 0.95);
    let optimized_max = max_sample_ms(&optimized_samples);
    let passes_resize_cpu_budget = optimized_p95 <= target_p95_ms && optimized_max <= target_max_ms;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "frames": frames,
            "target_p95_ms": target_p95_ms,
            "target_repeated_max_ms": target_max_ms,
            "passes_resize_cpu_budget": passes_resize_cpu_budget,
            "scenario": "large transcript continuous resize CPU layout path",
            "size_range": {
                "min_width": sizes.iter().map(|size| size.width).min().unwrap_or_default(),
                "max_width": sizes.iter().map(|size| size.width).max().unwrap_or_default(),
                "min_height": sizes.iter().map(|size| size.height).min().unwrap_or_default(),
                "max_height": sizes.iter().map(|size| size.height).max().unwrap_or_default(),
            },
            "optimized_body_wraps": optimized_body_wraps,
            "optimized_body_buffer_rebuilds": optimized_body_rebuilds,
            "legacy": benchmark_samples_json("legacy_resize_full_text_relayout", &legacy_samples, legacy_checksum),
            "optimized": benchmark_samples_json("optimized_resize_cached_visible_body", &optimized_samples, optimized_checksum),
        }))?
    );
    Ok(())
}

fn run_scroll_render_benchmark(frames: usize) -> Result<()> {
    let frames = frames.max(1);
    let size = PhysicalSize::new(1200, 760);
    let mut app = desktop_scroll_benchmark_app();
    if let Some(metrics) = single_session_body_scroll_metrics(&app, size, 0) {
        app.body_scroll_lines = metrics.max_scroll_lines as f32 / 2.0;
    }

    let mut setup_font_system = benchmark_font_system();
    let setup_started = Instant::now();
    let setup_key = single_session_text_key_for_tick_with_scroll(&app, size, 0, 0.0);
    let setup_buffers =
        single_session_text_buffers_from_key(&setup_key, size, &mut setup_font_system);
    let setup_areas =
        single_session_text_areas_for_app_with_scroll(&app, &setup_buffers, size, 0, 0.0);
    let setup_vertices =
        build_single_session_vertices_with_scroll_and_reveal(&app, size, 0.0, 0, 0.0, 1.0);
    let setup_elapsed = setup_started.elapsed();
    let setup_checksum =
        setup_key.body.len() ^ setup_buffers.len() ^ setup_areas.len() ^ setup_vertices.len();

    let cold_fresh_app = SingleSessionApp::new(None);
    let cold_fresh_started = Instant::now();
    let cold_phase_started = Instant::now();
    let mut cold_fresh_font_system = benchmark_font_system();
    let cold_fresh_font_ms = cold_phase_started.elapsed().as_secs_f64() * 1000.0;
    let cold_phase_started = Instant::now();
    let cold_fresh_key =
        single_session_text_key_for_tick_with_scroll(&cold_fresh_app, size, 0, 0.0);
    let cold_fresh_key_ms = cold_phase_started.elapsed().as_secs_f64() * 1000.0;
    let cold_phase_started = Instant::now();
    let cold_fresh_buffers =
        single_session_text_buffers_from_key(&cold_fresh_key, size, &mut cold_fresh_font_system);
    let cold_fresh_buffers_ms = cold_phase_started.elapsed().as_secs_f64() * 1000.0;
    let cold_phase_started = Instant::now();
    let cold_fresh_areas = single_session_text_areas_for_app_with_scroll(
        &cold_fresh_app,
        &cold_fresh_buffers,
        size,
        0,
        0.0,
    );
    let cold_fresh_areas_ms = cold_phase_started.elapsed().as_secs_f64() * 1000.0;
    let cold_phase_started = Instant::now();
    let cold_fresh_vertices = build_single_session_vertices_with_scroll_and_reveal(
        &cold_fresh_app,
        size,
        0.0,
        0,
        0.0,
        1.0,
    );
    let cold_fresh_vertices_ms = cold_phase_started.elapsed().as_secs_f64() * 1000.0;
    let cold_fresh_ms = cold_fresh_started.elapsed().as_secs_f64() * 1000.0;
    let cold_fresh_checksum = cold_fresh_key.body.len()
        ^ cold_fresh_buffers.len()
        ^ cold_fresh_areas.len()
        ^ cold_fresh_vertices.len();

    let prewarmed_fresh_app = SingleSessionApp::new(None);
    let mut prewarmed_fresh_font_system = benchmark_font_system();
    let prewarmed_fresh_started = Instant::now();
    let prewarmed_fresh_key =
        single_session_text_key_for_tick_with_scroll(&prewarmed_fresh_app, size, 0, 0.0);
    let prewarmed_fresh_buffers = single_session_text_buffers_from_key(
        &prewarmed_fresh_key,
        size,
        &mut prewarmed_fresh_font_system,
    );
    let prewarmed_fresh_areas = single_session_text_areas_for_app_with_scroll(
        &prewarmed_fresh_app,
        &prewarmed_fresh_buffers,
        size,
        0,
        0.0,
    );
    let prewarmed_fresh_vertices = build_single_session_vertices_with_scroll_and_reveal(
        &prewarmed_fresh_app,
        size,
        0.0,
        0,
        0.0,
        1.0,
    );
    let prewarmed_fresh_ms = prewarmed_fresh_started.elapsed().as_secs_f64() * 1000.0;
    let prewarmed_fresh_checksum = prewarmed_fresh_key.body.len()
        ^ prewarmed_fresh_buffers.len()
        ^ prewarmed_fresh_areas.len()
        ^ prewarmed_fresh_vertices.len();

    let warm_fresh_app = SingleSessionApp::new(None);
    let mut warm_fresh_font_system = benchmark_font_system();
    let warm_fresh_initial_key =
        single_session_text_key_for_tick_with_scroll(&warm_fresh_app, size, 0, 0.0);
    let warm_fresh_initial_buffers = single_session_text_buffers_from_key(
        &warm_fresh_initial_key,
        size,
        &mut warm_fresh_font_system,
    );
    let warm_fresh_started = Instant::now();
    let warm_fresh_next_key =
        single_session_text_key_for_tick_with_scroll(&warm_fresh_app, size, 1, 0.0);
    let warm_fresh_buffers = single_session_text_buffers_from_key_reusing_unchanged(
        &warm_fresh_next_key,
        Some(&warm_fresh_initial_key),
        warm_fresh_initial_buffers,
        true,
        size,
        &mut warm_fresh_font_system,
    );
    let warm_fresh_areas = single_session_text_areas_for_app_with_scroll(
        &warm_fresh_app,
        &warm_fresh_buffers,
        size,
        1,
        0.0,
    );
    let warm_fresh_vertices = build_single_session_vertices_with_scroll_and_reveal(
        &warm_fresh_app,
        size,
        0.0,
        1,
        0.0,
        1.0,
    );
    let warm_fresh_ms = warm_fresh_started.elapsed().as_secs_f64() * 1000.0;
    let warm_fresh_checksum = warm_fresh_next_key.body.len()
        ^ warm_fresh_buffers.len()
        ^ warm_fresh_areas.len()
        ^ warm_fresh_vertices.len();

    let mut legacy_font_system = benchmark_font_system();
    let (legacy_smooth_text_ms, legacy_smooth_text_checksum) = benchmark_phase(frames, |frame| {
        let tick = frame as u64;
        let smooth_scroll_lines = benchmark_smooth_scroll_lines(frame);
        let key =
            single_session_text_key_for_tick_with_scroll(&app, size, tick, smooth_scroll_lines);
        let buffers = single_session_text_buffers_from_key(&key, size, &mut legacy_font_system);
        let areas = single_session_text_areas_for_app_with_scroll(
            &app,
            &buffers,
            size,
            tick,
            smooth_scroll_lines,
        );
        let vertices = build_single_session_vertices_with_scroll_and_reveal(
            &app,
            size,
            0.0,
            tick,
            smooth_scroll_lines,
            1.0,
        );
        key.body.len() ^ buffers.len() ^ areas.len() ^ vertices.len()
    });

    let mut optimized_font_system = benchmark_font_system();
    let optimized_key = single_session_text_key_for_tick_with_scroll(&app, size, 0, 0.0);
    let optimized_buffers =
        single_session_text_buffers_from_key(&optimized_key, size, &mut optimized_font_system);
    let optimized_areas =
        single_session_text_areas_for_app_with_scroll(&app, &optimized_buffers, size, 0, 0.0);
    let optimized_body_lines = single_session_rendered_body_lines_for_tick(&app, size, 0);
    let (optimized_smooth_geometry_ms, optimized_smooth_geometry_checksum) =
        benchmark_phase(frames, |frame| {
            let tick = frame as u64;
            let smooth_scroll_lines = benchmark_smooth_scroll_lines(frame);
            let vertices = build_single_session_vertices_with_cached_body(
                &app,
                size,
                0.0,
                tick,
                smooth_scroll_lines,
                1.0,
                &optimized_body_lines,
            );
            optimized_key.body.len()
                ^ optimized_buffers.len()
                ^ optimized_areas.len()
                ^ vertices.len()
        });

    let mut whole_line_app = app.clone();
    let mut whole_line_font_system = benchmark_font_system();
    let whole_line_body_lines =
        single_session_rendered_body_lines_for_tick(&whole_line_app, size, 0);
    let (whole_line_text_ms, whole_line_text_checksum) = benchmark_phase(frames, |frame| {
        whole_line_app.scroll_body_lines(if frame % 2 == 0 { 1 } else { -1 });
        let tick = frame as u64;
        let key = single_session_text_key_for_tick_with_rendered_body(
            &whole_line_app,
            size,
            tick,
            0.0,
            &whole_line_body_lines,
        );
        let buffers = single_session_text_buffers_from_key(&key, size, &mut whole_line_font_system);
        let areas = single_session_text_areas_for_app_with_cached_body(
            &whole_line_app,
            &buffers,
            size,
            0.0,
            &whole_line_body_lines,
        );
        let vertices = build_single_session_vertices_with_cached_body(
            &whole_line_app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
            &whole_line_body_lines,
        );
        key.body.len() ^ buffers.len() ^ areas.len() ^ vertices.len()
    });

    let mut visible_whole_line_app = app.clone();
    let mut visible_whole_line_font_system = benchmark_font_system();
    let visible_whole_line_body_lines =
        single_session_rendered_body_lines_for_tick(&visible_whole_line_app, size, 0);
    let visible_whole_line_key = single_session_text_key_for_tick_with_rendered_body(
        &visible_whole_line_app,
        size,
        0,
        0.0,
        &visible_whole_line_body_lines,
    );
    let mut visible_whole_line_buffers = single_session_text_buffers_from_key(
        &visible_whole_line_key,
        size,
        &mut visible_whole_line_font_system,
    );
    let mut visible_whole_line_start = single_session_body_viewport_from_lines(
        &visible_whole_line_app,
        size,
        0.0,
        &visible_whole_line_body_lines,
    )
    .start_line;
    let initial_visible_viewport = single_session_body_viewport_from_lines(
        &visible_whole_line_app,
        size,
        0.0,
        &visible_whole_line_body_lines,
    );
    let (mut visible_window_start, mut visible_window_end) =
        single_session_body_text_window_bounds(&initial_visible_viewport);
    if let Some(body_buffer) = visible_whole_line_buffers.get_mut(1) {
        *body_buffer = single_session_body_text_buffer_from_lines(
            &mut visible_whole_line_font_system,
            &visible_whole_line_body_lines[visible_window_start..visible_window_end],
            size,
            visible_whole_line_app.text_scale(),
        );
        body_buffer.set_scroll(
            initial_visible_viewport
                .start_line
                .saturating_sub(visible_window_start)
                .min(i32::MAX as usize) as i32,
        );
    }
    let mut visible_viewport_ms = 0.0;
    let mut visible_window_ms = 0.0;
    let mut visible_scroll_ms = 0.0;
    let mut visible_glyph_ms = 0.0;
    let mut visible_areas_ms = 0.0;
    let mut visible_vertices_ms = 0.0;
    let (visible_whole_line_text_ms, visible_whole_line_text_checksum) =
        benchmark_phase(frames, |frame| {
            visible_whole_line_app.scroll_body_lines(if frame % 2 == 0 { 1 } else { -1 });
            let tick = frame as u64;
            let phase_started = Instant::now();
            let viewport = single_session_body_viewport_from_lines(
                &visible_whole_line_app,
                size,
                0.0,
                &visible_whole_line_body_lines,
            );
            visible_viewport_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let phase_started = Instant::now();
            if !single_session_body_text_window_contains(
                visible_window_start,
                visible_window_end,
                &viewport,
            ) {
                (visible_window_start, visible_window_end) =
                    single_session_body_text_window_bounds(&viewport);
                if let Some(body_buffer) = visible_whole_line_buffers.get_mut(1) {
                    *body_buffer = single_session_body_text_buffer_from_lines(
                        &mut visible_whole_line_font_system,
                        &visible_whole_line_body_lines[visible_window_start..visible_window_end],
                        size,
                        visible_whole_line_app.text_scale(),
                    );
                }
                visible_whole_line_start = usize::MAX;
            }
            visible_window_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let phase_started = Instant::now();
            if viewport.start_line != visible_whole_line_start {
                if let Some(body_buffer) = visible_whole_line_buffers.get_mut(1) {
                    body_buffer.set_scroll(
                        viewport
                            .start_line
                            .saturating_sub(visible_window_start)
                            .min(i32::MAX as usize) as i32,
                    );
                }
                visible_whole_line_start = viewport.start_line;
            }
            visible_scroll_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let phase_started = Instant::now();
            let glyph_checksum = visible_whole_line_buffers
                .get(1)
                .map(|body_buffer| {
                    body_buffer
                        .layout_runs()
                        .map(|run| run.glyphs.len())
                        .sum::<usize>()
                })
                .unwrap_or_default();
            visible_glyph_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let phase_started = Instant::now();
            let areas = single_session_text_areas_for_app_with_cached_body_viewport(
                &visible_whole_line_app,
                &visible_whole_line_buffers,
                size,
                0.0,
                viewport,
            );
            visible_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let phase_started = Instant::now();
            let vertices = build_single_session_vertices_with_cached_body(
                &visible_whole_line_app,
                size,
                0.0,
                tick,
                0.0,
                1.0,
                &visible_whole_line_body_lines,
            );
            visible_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            visible_whole_line_key.body.len()
                ^ visible_whole_line_buffers.len()
                ^ areas.len()
                ^ vertices.len()
                ^ glyph_checksum
        });

    let mut typing_app = app.clone();
    typing_app.scroll_body_to_bottom();
    typing_app.draft.clear();
    typing_app.draft_cursor = 0;
    let typing_body_lines = single_session_rendered_body_lines_for_tick(&typing_app, size, 0);
    let mut typing_font_system = benchmark_font_system();
    let typing_initial_key = single_session_text_key_for_tick_with_rendered_body(
        &typing_app,
        size,
        0,
        0.0,
        &typing_body_lines,
    );
    let mut typing_buffers =
        single_session_text_buffers_from_key(&typing_initial_key, size, &mut typing_font_system);
    let typing_initial_viewport =
        single_session_body_viewport_from_lines(&typing_app, size, 0.0, &typing_body_lines);
    let (typing_window_start, typing_window_end) =
        single_session_body_text_window_bounds(&typing_initial_viewport);
    if let Some(body_buffer) = typing_buffers.get_mut(1) {
        *body_buffer = single_session_body_text_buffer_from_lines(
            &mut typing_font_system,
            &typing_body_lines[typing_window_start..typing_window_end],
            size,
            typing_app.text_scale(),
        );
    }
    let mut typing_previous_key = Some(typing_initial_key);
    let mut typing_text_cache_ms = 0.0;
    let mut typing_areas_ms = 0.0;
    let mut typing_vertices_ms = 0.0;
    let (typing_redraw_ms, typing_redraw_checksum) = benchmark_phase(frames, |frame| {
        let ch = benchmark_typing_char(frame);
        typing_app.draft.push(ch);
        typing_app.draft_cursor = typing_app.draft.len();
        let tick = frame as u64;

        let phase_started = Instant::now();
        let key = single_session_text_key_for_tick_with_rendered_body(
            &typing_app,
            size,
            tick,
            0.0,
            &typing_body_lines,
        );
        let draft_len = key.draft.len();
        let previous_key = typing_previous_key.take();
        let old_buffers = std::mem::take(&mut typing_buffers);
        typing_buffers = single_session_text_buffers_from_key_reusing_unchanged(
            &key,
            previous_key.as_ref(),
            old_buffers,
            true,
            size,
            &mut typing_font_system,
        );
        typing_previous_key = Some(key);
        typing_text_cache_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let areas = single_session_text_areas_for_app_with_cached_body(
            &typing_app,
            &typing_buffers,
            size,
            0.0,
            &typing_body_lines,
        );
        typing_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let vertices = build_single_session_vertices_with_cached_body(
            &typing_app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
            &typing_body_lines,
        );
        typing_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        draft_len ^ typing_buffers.len() ^ areas.len() ^ vertices.len()
    });

    let mut fresh_typing_app = SingleSessionApp::new(None);
    fresh_typing_app.draft.clear();
    fresh_typing_app.draft_cursor = 0;
    let mut fresh_typing_font_system = benchmark_font_system();
    let mut fresh_typing_text_cache_ms = 0.0;
    let mut fresh_typing_areas_ms = 0.0;
    let mut fresh_typing_vertices_ms = 0.0;
    let (fresh_typing_ms, fresh_typing_checksum) = benchmark_phase(frames, |frame| {
        let ch = benchmark_typing_char(frame);
        fresh_typing_app.draft.push(ch);
        fresh_typing_app.draft_cursor = fresh_typing_app.draft.len();
        let tick = frame as u64;

        let phase_started = Instant::now();
        let key = single_session_text_key_for_tick_with_scroll(&fresh_typing_app, size, tick, 0.0);
        let buffers =
            single_session_text_buffers_from_key(&key, size, &mut fresh_typing_font_system);
        fresh_typing_text_cache_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let areas = single_session_text_areas_for_app_with_scroll(
            &fresh_typing_app,
            &buffers,
            size,
            tick,
            0.0,
        );
        fresh_typing_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let vertices = build_single_session_vertices_with_scroll_and_reveal(
            &fresh_typing_app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
        );
        fresh_typing_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        key.draft.len() ^ buffers.len() ^ areas.len() ^ vertices.len()
    });

    let mut streaming_app = app.clone();
    streaming_app.scroll_body_to_bottom();
    streaming_app.streaming_response.clear();
    let mut streaming_font_system = benchmark_font_system();
    let mut streaming_body_lines =
        single_session_rendered_body_lines_for_tick(&streaming_app, size, 0);
    let mut streaming_base_key = None;
    let mut streaming_base_len = 0usize;
    let streaming_initial_key = single_session_text_key_for_tick_with_rendered_body(
        &streaming_app,
        size,
        0,
        0.0,
        &streaming_body_lines,
    );
    let mut streaming_buffers = single_session_text_buffers_from_key(
        &streaming_initial_key,
        size,
        &mut streaming_font_system,
    );
    let streaming_initial_viewport =
        single_session_body_viewport_from_lines(&streaming_app, size, 0.0, &streaming_body_lines);
    let (mut streaming_window_start, mut streaming_window_end) =
        single_session_body_text_window_bounds(&streaming_initial_viewport);
    if let Some(body_buffer) = streaming_buffers.get_mut(1) {
        *body_buffer = single_session_body_text_buffer_from_lines(
            &mut streaming_font_system,
            &streaming_body_lines[streaming_window_start..streaming_window_end],
            size,
            streaming_app.text_scale(),
        );
        body_buffer.set_scroll(
            streaming_initial_viewport
                .start_line
                .saturating_sub(streaming_window_start)
                .min(i32::MAX as usize) as i32,
        );
    }
    let mut streaming_previous_key = Some(streaming_initial_key);
    let mut streaming_tail_text_key = None;
    let mut streaming_tail_text_start_line = None;
    let mut streaming_tail_text_buffer = None;
    let mut streaming_body_ms = 0.0;
    let mut streaming_text_cache_ms = 0.0;
    let mut streaming_areas_ms = 0.0;
    let mut streaming_vertices_ms = 0.0;
    let mut streaming_static_base_rebuilds = 0usize;
    let mut streaming_tail_text_buffer_rebuilds = 0usize;
    let (streaming_delta_ms, streaming_delta_checksum) = benchmark_phase(frames, |frame| {
        streaming_app
            .streaming_response
            .push(benchmark_typing_char(frame));
        if frame % 17 == 0 {
            streaming_app.streaming_response.push('\n');
        }
        let tick = frame as u64;

        let phase_started = Instant::now();
        if !streaming_app.streaming_response.is_empty() {
            let base_key = streaming_app.rendered_body_static_cache_key((size.width, size.height));
            if streaming_base_key != Some(base_key) {
                streaming_static_base_rebuilds += 1;
                streaming_body_lines = single_session_rendered_static_body_lines_for_streaming(
                    &streaming_app,
                    size,
                    tick,
                )
                .unwrap_or_else(|| {
                    single_session_rendered_body_lines_for_tick(&streaming_app, size, tick)
                });
                streaming_base_len = streaming_body_lines.len();
                streaming_base_key = Some(base_key);
            } else {
                streaming_body_lines.truncate(streaming_base_len);
            }
            append_single_session_streaming_response_rendered_body_lines(
                &streaming_app,
                size,
                &mut streaming_body_lines,
            );
        } else {
            streaming_body_lines =
                single_session_rendered_body_lines_for_tick(&streaming_app, size, tick);
            streaming_base_key = None;
            streaming_base_len = 0;
        }
        streaming_body_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let key = single_session_text_key_for_tick_with_rendered_body(
            &streaming_app,
            size,
            tick,
            0.0,
            &streaming_body_lines,
        );
        let viewport = single_session_body_viewport_from_lines(
            &streaming_app,
            size,
            0.0,
            &streaming_body_lines,
        );
        let visible_static_start = viewport.start_line.min(streaming_base_len);
        let visible_static_end = viewport
            .start_line
            .saturating_add(viewport.lines.len())
            .min(streaming_base_len);
        let desired_streaming_window_start = visible_static_start
            .saturating_sub(SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_BEFORE_LINES);
        let desired_streaming_window_end = visible_static_end
            .saturating_add(SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_AFTER_LINES)
            .min(streaming_base_len)
            .max(desired_streaming_window_start);
        let body_window_contains = streaming_window_start == desired_streaming_window_start
            && streaming_window_end == desired_streaming_window_end;
        let previous_key = streaming_previous_key.take();
        let mut old_buffers = std::mem::take(&mut streaming_buffers);
        if old_buffers.len() > 1 && !body_window_contains {
            streaming_window_start = desired_streaming_window_start;
            streaming_window_end = desired_streaming_window_end;
            old_buffers[1] = single_session_body_text_buffer_from_lines(
                &mut streaming_font_system,
                &streaming_body_lines[streaming_window_start..streaming_window_end],
                size,
                streaming_app.text_scale(),
            );
        }
        let can_reuse_body_buffer = old_buffers.len() > 1;
        streaming_buffers = single_session_text_buffers_from_key_reusing_unchanged(
            &key,
            previous_key.as_ref(),
            old_buffers,
            can_reuse_body_buffer,
            size,
            &mut streaming_font_system,
        );
        if let Some(body_buffer) = streaming_buffers.get_mut(1) {
            body_buffer.set_scroll(
                viewport
                    .start_line
                    .saturating_sub(streaming_window_start)
                    .min(i32::MAX as usize) as i32,
            );
        }
        let streaming_start_line =
            streaming_base_len.saturating_add(usize::from(!streaming_app.messages.is_empty()));
        let visible_start = viewport.start_line;
        let visible_end = viewport.start_line.saturating_add(viewport.lines.len());
        let streaming_visible_start = streaming_start_line.max(visible_start);
        let streaming_visible_end = streaming_body_lines.len().min(visible_end);
        if streaming_visible_start < streaming_visible_end {
            let mut hasher = DefaultHasher::new();
            (size.width, size.height).hash(&mut hasher);
            streaming_app.text_scale().to_bits().hash(&mut hasher);
            streaming_visible_start.hash(&mut hasher);
            streaming_visible_end.hash(&mut hasher);
            streaming_body_lines[streaming_visible_start..streaming_visible_end].hash(&mut hasher);
            let tail_key = hasher.finish();
            if streaming_tail_text_key != Some(tail_key) {
                streaming_tail_text_buffer_rebuilds += 1;
                streaming_tail_text_buffer = Some(single_session_body_text_buffer_from_lines(
                    &mut streaming_font_system,
                    &streaming_body_lines[streaming_visible_start..streaming_visible_end],
                    size,
                    streaming_app.text_scale(),
                ));
                streaming_tail_text_key = Some(tail_key);
                streaming_tail_text_start_line = Some(streaming_visible_start);
            }
        } else {
            streaming_tail_text_key = None;
            streaming_tail_text_start_line = None;
            streaming_tail_text_buffer = None;
        }
        streaming_previous_key = Some(key);
        streaming_text_cache_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let mut areas = single_session_text_areas_for_app_with_cached_body_viewport(
            &streaming_app,
            &streaming_buffers,
            size,
            0.0,
            viewport.clone(),
        );
        if let (Some(buffer), Some(start_line)) = (
            streaming_tail_text_buffer.as_ref(),
            streaming_tail_text_start_line,
        ) {
            areas.push(single_session_streaming_text_area_for_cached_body_viewport(
                &streaming_app,
                buffer,
                size,
                viewport,
                start_line,
                1.0,
                0.0,
            ));
        }
        streaming_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let vertices = build_single_session_vertices_with_cached_body(
            &streaming_app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
            &streaming_body_lines,
        );
        streaming_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        streaming_body_lines.len()
            ^ streaming_buffers.len()
            ^ streaming_tail_text_buffer.is_some() as usize
            ^ streaming_static_base_rebuilds
            ^ streaming_tail_text_buffer_rebuilds
            ^ areas.len()
            ^ vertices.len()
    });

    let mut long_streaming_app = app.clone();
    long_streaming_app.scroll_body_to_bottom();
    long_streaming_app.streaming_response = (0..512)
        .map(|index| {
            format!(
                "### partial heading {index}\n- live item with **bold** text and `code` span number {index}\n"
            )
        })
        .collect::<String>();
    let (long_streaming_body_wrap_ms, long_streaming_body_wrap_checksum) =
        benchmark_phase(frames, |frame| {
            long_streaming_app
                .streaming_response
                .push(benchmark_typing_char(frame));
            if frame % 29 == 0 {
                long_streaming_app.streaming_response.push('\n');
            }
            let mut rendered_lines = Vec::new();
            append_single_session_streaming_response_rendered_body_lines(
                &long_streaming_app,
                size,
                &mut rendered_lines,
            );
            rendered_lines.len() ^ long_streaming_app.streaming_response.len()
        });
    let mut long_streaming_line_count_app = long_streaming_app.clone();
    let (long_streaming_line_count_ms, long_streaming_line_count_checksum) =
        benchmark_phase(frames, |frame| {
            long_streaming_line_count_app
                .streaming_response
                .push(benchmark_typing_char(frame));
            if frame % 31 == 0 {
                long_streaming_line_count_app.streaming_response.push('\n');
            }
            single_session_streaming_response_rendered_body_line_count(
                &long_streaming_line_count_app,
                size,
            ) ^ long_streaming_line_count_app.streaming_response.len()
        });
    let mut long_unbroken_streaming_app = app.clone();
    long_unbroken_streaming_app.streaming_response = "x".repeat(8192);
    let (long_unbroken_streaming_wrap_ms, long_unbroken_streaming_wrap_checksum) =
        benchmark_phase(frames, |frame| {
            long_unbroken_streaming_app
                .streaming_response
                .push(benchmark_typing_char(frame));
            let mut rendered_lines = Vec::new();
            append_single_session_streaming_response_rendered_body_lines(
                &long_unbroken_streaming_app,
                size,
                &mut rendered_lines,
            );
            rendered_lines.len() ^ long_unbroken_streaming_app.streaming_response.len()
        });

    let mut event_batch_app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let (event_batch_ms, event_batch_checksum) = benchmark_phase(frames, |frame| {
        let events = (0..128)
            .map(|offset| {
                session_launch::DesktopSessionEvent::TextDelta(
                    benchmark_typing_char(frame + offset).to_string(),
                )
            })
            .collect::<Vec<_>>();
        let original_events = events.len();
        let coalesced = coalesce_desktop_session_events(events);
        let coalesced_events = coalesced.len();
        apply_desktop_session_event_batch(&mut event_batch_app, coalesced);
        original_events ^ coalesced_events
    });

    let (event_forwarder_flood_ms, event_forwarder_flood_checksum) =
        benchmark_phase(frames, |frame| {
            let (tx, rx) = mpsc::channel();
            for offset in 0..(BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS * 3) {
                tx.send(session_launch::DesktopSessionEvent::TextDelta(
                    benchmark_typing_char(frame + offset).to_string(),
                ))
                .unwrap();
            }
            let batch = collect_desktop_session_event_batch(
                session_launch::DesktopSessionEvent::TextDelta(
                    benchmark_typing_char(frame).to_string(),
                ),
                &rx,
            );
            let remaining_is_queued = rx.try_recv().is_ok();
            batch.raw_event_count
                ^ batch.raw_payload_bytes
                ^ batch.events.len()
                ^ usize::from(remaining_is_queued)
        });
    let end_to_end_stream_flood =
        run_desktop_stream_end_to_end_benchmark(BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS * 6);

    let mut hero_app = desktop_scroll_benchmark_app_with_turns(24);
    let hero_body_lines = single_session_rendered_body_lines_for_tick(&hero_app, size, 0);
    let hero_boundary_scroll =
        benchmark_hero_boundary_scroll_lines(&hero_app, size, &hero_body_lines);
    hero_app.body_scroll_lines = hero_boundary_scroll;
    let mut hero_font_system = benchmark_font_system();
    let hero_initial_key = single_session_text_key_for_tick_with_rendered_body(
        &hero_app,
        size,
        0,
        0.0,
        &hero_body_lines,
    );
    let mut hero_buffers =
        single_session_text_buffers_from_key(&hero_initial_key, size, &mut hero_font_system);
    let hero_initial_viewport =
        single_session_body_viewport_from_lines(&hero_app, size, 0.0, &hero_body_lines);
    let (mut hero_window_start, mut hero_window_end) =
        single_session_body_text_window_bounds(&hero_initial_viewport);
    if let Some(body_buffer) = hero_buffers.get_mut(1) {
        *body_buffer = single_session_body_text_buffer_from_lines(
            &mut hero_font_system,
            &hero_body_lines[hero_window_start..hero_window_end],
            size,
            hero_app.text_scale(),
        );
    }
    let mut hero_previous_key = Some(hero_initial_key);
    let mut hero_viewport_key_ms = 0.0;
    let mut hero_window_rebuild_ms = 0.0;
    let mut hero_buffer_reuse_ms = 0.0;
    let mut hero_body_buffer_rebuilds = 0usize;
    let mut hero_text_cache_ms = 0.0;
    let mut hero_areas_ms = 0.0;
    let mut hero_vertices_ms = 0.0;
    let (hero_boundary_scroll_ms, hero_boundary_checksum) = benchmark_phase(frames, |frame| {
        let tick = frame as u64;
        let scroll_offset = (frame % 24) as f32 - 12.0;
        hero_app.body_scroll_lines = (hero_boundary_scroll + scroll_offset).max(0.0);
        let smooth_scroll_lines = benchmark_smooth_scroll_lines(frame);

        let phase_started = Instant::now();
        let subphase_started = Instant::now();
        let viewport = single_session_body_viewport_from_lines(
            &hero_app,
            size,
            smooth_scroll_lines,
            &hero_body_lines,
        );
        let key = single_session_text_key_for_tick_with_rendered_body(
            &hero_app,
            size,
            tick,
            smooth_scroll_lines,
            &hero_body_lines,
        );
        hero_viewport_key_ms += subphase_started.elapsed().as_secs_f64() * 1000.0;

        let subphase_started = Instant::now();
        let previous_key = hero_previous_key.take();
        let mut old_buffers = std::mem::take(&mut hero_buffers);
        if old_buffers.len() > 1
            && !single_session_body_text_window_contains(
                hero_window_start,
                hero_window_end,
                &viewport,
            )
        {
            hero_body_buffer_rebuilds += 1;
            (hero_window_start, hero_window_end) =
                single_session_body_text_window_bounds(&viewport);
            old_buffers[1] = single_session_body_text_buffer_from_lines(
                &mut hero_font_system,
                &hero_body_lines[hero_window_start..hero_window_end],
                size,
                hero_app.text_scale(),
            );
        }
        hero_window_rebuild_ms += subphase_started.elapsed().as_secs_f64() * 1000.0;

        let subphase_started = Instant::now();
        let can_reuse_body_buffer = old_buffers.len() > 1;
        hero_buffers = single_session_text_buffers_from_key_reusing_unchanged(
            &key,
            previous_key.as_ref(),
            old_buffers,
            can_reuse_body_buffer,
            size,
            &mut hero_font_system,
        );
        if let Some(body_buffer) = hero_buffers.get_mut(1) {
            body_buffer.set_scroll(
                viewport
                    .start_line
                    .saturating_sub(hero_window_start)
                    .min(i32::MAX as usize) as i32,
            );
        }
        let hero_visible = key.fresh_welcome_visible;
        hero_previous_key = Some(key);
        hero_buffer_reuse_ms += subphase_started.elapsed().as_secs_f64() * 1000.0;
        hero_text_cache_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let areas = single_session_text_areas_for_app_with_cached_body_viewport(
            &hero_app,
            &hero_buffers,
            size,
            smooth_scroll_lines,
            viewport,
        );
        hero_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let vertices = build_single_session_vertices_with_cached_body(
            &hero_app,
            size,
            0.0,
            tick,
            smooth_scroll_lines,
            1.0,
            &hero_body_lines,
        );
        hero_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        hero_buffers.len() ^ areas.len() ^ vertices.len() ^ usize::from(hero_visible)
    });

    let mut action_input_app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let (action_input_ms, action_input_checksum) = benchmark_phase(frames, |frame| {
        let events = (0..128)
            .map(|offset| session_launch::DesktopSessionEvent::ToolInput {
                id: None,
                delta: benchmark_typing_char(frame + offset).to_string(),
            })
            .collect::<Vec<_>>();
        let coalesced = coalesce_desktop_session_events(events);
        let visible_changed = apply_desktop_session_event_batch(&mut action_input_app, coalesced);
        usize::from(visible_changed)
    });

    let mut action_app = desktop_scroll_benchmark_app_with_turns(64);
    action_app.scroll_body_to_bottom();
    action_app.apply_session_event(session_launch::DesktopSessionEvent::ToolStarted {
        id: None,
        name: "bash".to_string(),
    });
    let mut action_font_system = benchmark_font_system();
    let mut action_body_key = action_app.rendered_body_cache_key((size.width, size.height));
    let mut action_body_lines = single_session_rendered_body_lines_for_tick(&action_app, size, 0);
    let action_initial_key = single_session_text_key_for_tick_with_rendered_body(
        &action_app,
        size,
        0,
        0.0,
        &action_body_lines,
    );
    let mut action_buffers =
        single_session_text_buffers_from_key(&action_initial_key, size, &mut action_font_system);
    let action_initial_viewport =
        single_session_body_viewport_from_lines(&action_app, size, 0.0, &action_body_lines);
    let (mut action_window_start, mut action_window_end) =
        single_session_body_text_window_bounds(&action_initial_viewport);
    if let Some(body_buffer) = action_buffers.get_mut(1) {
        *body_buffer = single_session_body_text_buffer_from_lines(
            &mut action_font_system,
            &action_body_lines[action_window_start..action_window_end],
            size,
            action_app.text_scale(),
        );
    }
    let mut action_previous_key = Some(action_initial_key);
    let mut action_apply_ms = 0.0;
    let mut action_body_ms = 0.0;
    let mut action_text_cache_ms = 0.0;
    let mut action_areas_ms = 0.0;
    let mut action_vertices_ms = 0.0;
    let (action_visible_ms, action_visible_checksum) = benchmark_phase(frames, |frame| {
        let phase_started = Instant::now();
        action_app.apply_session_event(session_launch::DesktopSessionEvent::ToolInput {
            id: None,
            delta: format!(" chunk-{frame}"),
        });
        action_app.apply_session_event(session_launch::DesktopSessionEvent::ToolExecuting {
            id: None,
            name: "bash".to_string(),
        });
        action_apply_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
        let tick = frame as u64;

        let phase_started = Instant::now();
        let next_body_key = action_app.rendered_body_cache_key((size.width, size.height));
        let action_body_changed = action_body_key != next_body_key;
        if action_body_changed {
            action_body_lines =
                single_session_rendered_body_lines_for_tick(&action_app, size, tick);
            action_body_key = next_body_key;
        }
        action_body_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let key = single_session_text_key_for_tick_with_rendered_body(
            &action_app,
            size,
            tick,
            0.0,
            &action_body_lines,
        );
        let viewport =
            single_session_body_viewport_from_lines(&action_app, size, 0.0, &action_body_lines);
        let previous_key = action_previous_key.take();
        let mut old_buffers = std::mem::take(&mut action_buffers);
        if old_buffers.len() > 1
            && (action_body_changed
                || !single_session_body_text_window_contains(
                    action_window_start,
                    action_window_end,
                    &viewport,
                ))
        {
            (action_window_start, action_window_end) =
                single_session_body_text_window_bounds(&viewport);
            old_buffers[1] = single_session_body_text_buffer_from_lines(
                &mut action_font_system,
                &action_body_lines[action_window_start..action_window_end],
                size,
                action_app.text_scale(),
            );
        }
        let can_reuse_body_buffer = old_buffers.len() > 1;
        action_buffers = single_session_text_buffers_from_key_reusing_unchanged(
            &key,
            previous_key.as_ref(),
            old_buffers,
            can_reuse_body_buffer,
            size,
            &mut action_font_system,
        );
        if let Some(body_buffer) = action_buffers.get_mut(1) {
            body_buffer.set_scroll(
                viewport
                    .start_line
                    .saturating_sub(action_window_start)
                    .min(i32::MAX as usize) as i32,
            );
        }
        action_previous_key = Some(key);
        action_text_cache_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let areas = single_session_text_areas_for_app_with_cached_body_viewport(
            &action_app,
            &action_buffers,
            size,
            0.0,
            viewport,
        );
        action_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        let phase_started = Instant::now();
        let vertices = build_single_session_vertices_with_cached_body(
            &action_app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
            &action_body_lines,
        );
        action_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;

        action_body_lines.len() ^ action_buffers.len() ^ areas.len() ^ vertices.len()
    });

    let mut workspace_app = Workspace::from_session_cards(benchmark_workspace_session_cards(128));
    let mut workspace_layout_ms = 0.0;
    let mut workspace_vertices_ms = 0.0;
    let mut workspace_visible_surfaces = 0usize;
    let (workspace_navigation_ms, workspace_navigation_checksum) =
        benchmark_phase(frames, |frame| {
            let key = if frame % 2 == 0 { "l" } else { "h" };
            let _ = workspace_app.handle_key(KeyInput::Character(key.to_string()));
            let phase_started = Instant::now();
            let layout = workspace_render_layout(&workspace_app, size, Some(size));
            workspace_layout_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let phase_started = Instant::now();
            let vertices = build_vertices(&workspace_app, size, layout, 0.0, None);
            workspace_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            workspace_visible_surfaces +=
                workspace_visible_surface_count(&workspace_app, size, layout);
            vertices.len() ^ (workspace_app.focused_id as usize) ^ workspace_app.surfaces.len()
        });

    let mut workspace_full_app =
        Workspace::from_session_cards(benchmark_workspace_session_cards(512));
    let mut workspace_full_layout_ms = 0.0;
    let mut workspace_full_vertices_ms = 0.0;
    let mut workspace_full_text_panes_ms = 0.0;
    let mut workspace_full_text_areas_ms = 0.0;
    let mut workspace_full_visible_surfaces = 0usize;
    let mut workspace_full_text_pane_count = 0usize;
    let mut workspace_full_text_area_count = 0usize;
    let mut workspace_full_font_system = benchmark_font_system();
    let mut workspace_full_text_pane_cache = HashMap::new();
    let (workspace_full_frame_ms, workspace_full_frame_checksum) =
        benchmark_phase(frames, |frame| {
            let key = match frame % 4 {
                0 | 1 => "j",
                _ => "k",
            };
            let _ = workspace_full_app.handle_key(KeyInput::Character(key.to_string()));
            let phase_started = Instant::now();
            let layout = workspace_render_layout(&workspace_full_app, size, Some(size));
            workspace_full_layout_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            workspace_full_visible_surfaces +=
                workspace_visible_surface_count(&workspace_full_app, size, layout);
            let phase_started = Instant::now();
            let panes = build_workspace_single_session_text_panes(
                &mut workspace_full_text_pane_cache,
                &workspace_full_app,
                size,
                layout,
                None,
                &mut workspace_full_font_system,
            );
            workspace_full_text_panes_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let pane_count = panes.len();
            workspace_full_text_pane_count += pane_count;
            let phase_started = Instant::now();
            let areas = workspace_single_session_text_areas(&panes);
            workspace_full_text_areas_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            let area_count = areas.len();
            workspace_full_text_area_count += area_count;
            drop(areas);
            drop(panes);
            let phase_started = Instant::now();
            let mut vertices =
                Vec::with_capacity(workspace_vertex_capacity_hint(&workspace_full_app));
            build_vertices_into(
                WorkspaceVertexBuildParams {
                    workspace: &workspace_full_app,
                    size,
                    render_layout: layout,
                    focus_pulse: 0.0,
                    space_hold_progress: None,
                    surface_frames: None,
                    exiting_surfaces: &HashMap::new(),
                    workspace_panel_cache: Some(&workspace_full_text_pane_cache),
                    status_color: workspace_status_bar_target_color(&workspace_full_app),
                    status_text_frame: None,
                },
                &mut vertices,
            );
            workspace_full_vertices_ms += phase_started.elapsed().as_secs_f64() * 1000.0;
            vertices.len() ^ pane_count ^ area_count ^ workspace_full_app.surfaces.len()
        });

    let mut large_app = desktop_large_transcript_benchmark_app();
    let large_body_started = Instant::now();
    let large_body_lines = single_session_rendered_body_lines_for_tick(&large_app, size, 0);
    if let Some(metrics) =
        single_session_body_scroll_metrics_for_total_lines(&large_app, size, large_body_lines.len())
    {
        large_app.body_scroll_lines = metrics.max_scroll_lines as f32 / 2.0;
    }
    let large_body_elapsed = large_body_started.elapsed();
    let (large_scroll_ms, large_scroll_checksum) = benchmark_phase(frames, |frame| {
        large_app.scroll_body_lines(if frame % 2 == 0 { 1 } else { -1 });
        let viewport =
            single_session_body_viewport_from_lines(&large_app, size, 0.0, &large_body_lines);
        let areas = single_session_text_areas_for_app_with_cached_body_viewport(
            &large_app,
            &visible_whole_line_buffers,
            size,
            0.0,
            viewport,
        );
        let vertices = build_single_session_vertices_with_cached_body(
            &large_app,
            size,
            0.0,
            frame as u64,
            0.0,
            1.0,
            &large_body_lines,
        );
        large_body_lines.len() ^ areas.len() ^ vertices.len()
    });
    let (large_cache_key_ms, large_cache_key_checksum) = benchmark_phase(frames, |frame| {
        let key = large_app.rendered_body_cache_key((size.width, size.height));
        (key as usize) ^ frame ^ large_app.messages.len()
    });

    let target_budget_ms = duration_ms(DESKTOP_120FPS_FRAME_BUDGET);
    let critical_phase_means_ms = [
        visible_whole_line_text_ms / frames as f64,
        typing_redraw_ms / frames as f64,
        fresh_typing_ms / frames as f64,
        streaming_delta_ms / frames as f64,
        long_streaming_body_wrap_ms / frames as f64,
        long_streaming_line_count_ms / frames as f64,
        long_unbroken_streaming_wrap_ms / frames as f64,
        event_batch_ms / frames as f64,
        event_forwarder_flood_ms / frames as f64,
        hero_boundary_scroll_ms / frames as f64,
        action_input_ms / frames as f64,
        action_visible_ms / frames as f64,
        workspace_navigation_ms / frames as f64,
        workspace_full_frame_ms / frames as f64,
        large_scroll_ms / frames as f64,
        large_cache_key_ms / frames as f64,
    ];
    let passes_interaction_cpu_budget = critical_phase_means_ms
        .iter()
        .all(|mean_ms| *mean_ms <= target_budget_ms);
    let metrics = single_session_body_scroll_metrics(&app, size, 0);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "frames": frames,
            "target_frame_budget_ms": target_budget_ms,
            "passes_120fps_scroll_cpu_budget": (visible_whole_line_text_ms / frames as f64)
                <= target_budget_ms,
            "passes_120fps_interaction_cpu_budget": passes_interaction_cpu_budget,
            "passes_no_paint_watchdog_budget": end_to_end_stream_flood.passes_no_paint_budget(),
            "passes_streaming_incremental_wrap_guard": streaming_static_base_rebuilds <= 1,
            "passes_long_streaming_body_wrap_budget": (long_streaming_body_wrap_ms / frames as f64) <= target_budget_ms,
            "passes_long_streaming_line_count_budget": (long_streaming_line_count_ms / frames as f64) <= target_budget_ms,
            "passes_long_unbroken_streaming_wrap_budget": (long_unbroken_streaming_wrap_ms / frames as f64) <= target_budget_ms,
            "event_delivery": {
                "previous_background_poll_interval_ms": duration_ms(BACKGROUND_POLL_INTERVAL),
                "backend_redraw_frame_interval_ms": duration_ms(BACKEND_REDRAW_FRAME_INTERVAL),
                "backend_event_forward_interval_ms": duration_ms(BACKEND_EVENT_FORWARD_INTERVAL),
                "backend_event_forward_max_raw_events": BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS,
                "backend_event_forward_max_payload_bytes": BACKEND_EVENT_FORWARD_MAX_PAYLOAD_BYTES,
                "backend_events_wake_event_loop": true,
                "coalesces_consecutive_text_delta_events": true,
                "bounded_stream_flood_forwarding": true,
            },
            "no_paint_watchdog": {
                "budget_ms": duration_ms(DESKTOP_NO_PAINT_BUDGET),
                "log_event": "jcode-desktop-no-paint-profile",
                "requests_recovery_redraw": true,
            },
            "end_to_end_stream_flood": end_to_end_stream_flood.to_json(),
            "size": { "width": size.width, "height": size.height },
            "messages": app.messages.len(),
            "scroll_metrics": metrics.map(|metrics| serde_json::json!({
                "total_lines": metrics.total_lines,
                "visible_lines": metrics.visible_lines,
                "max_scroll_lines": metrics.max_scroll_lines,
                "start_scroll_lines": app.body_scroll_lines,
            })),
            "setup": benchmark_phase_json(
                "setup_text_and_geometry",
                setup_elapsed.as_secs_f64() * 1000.0,
                1,
                setup_checksum,
            ),
            "phases": [
                benchmark_phase_json(
                    "legacy_smooth_scroll_text_relayout",
                    legacy_smooth_text_ms,
                    frames,
                    legacy_smooth_text_checksum,
                ),
                benchmark_phase_json(
                    "optimized_smooth_scroll_geometry_only",
                    optimized_smooth_geometry_ms,
                    frames,
                    optimized_smooth_geometry_checksum,
                ),
                benchmark_phase_json(
                    "legacy_whole_line_scroll_text_relayout",
                    whole_line_text_ms,
                    frames,
                    whole_line_text_checksum,
                ),
                benchmark_phase_json(
                    "optimized_whole_line_scroll_visible_body_only",
                    visible_whole_line_text_ms,
                    frames,
                    visible_whole_line_text_checksum,
                ),
                benchmark_phase_json(
                    "typing_redraw_reuse_body_cache",
                    typing_redraw_ms,
                    frames,
                    typing_redraw_checksum,
                ),
                benchmark_phase_json(
                    "fresh_welcome_typing_redraw",
                    fresh_typing_ms,
                    frames,
                    fresh_typing_checksum,
                ),
                benchmark_phase_json(
                    "streaming_delta_redraw",
                    streaming_delta_ms,
                    frames,
                    streaming_delta_checksum,
                ),
                benchmark_phase_json(
                    "long_streaming_response_body_wrap",
                    long_streaming_body_wrap_ms,
                    frames,
                    long_streaming_body_wrap_checksum,
                ),
                benchmark_phase_json(
                    "long_streaming_response_line_count",
                    long_streaming_line_count_ms,
                    frames,
                    long_streaming_line_count_checksum,
                ),
                benchmark_phase_json(
                    "long_unbroken_streaming_line_wrap",
                    long_unbroken_streaming_wrap_ms,
                    frames,
                    long_unbroken_streaming_wrap_checksum,
                ),
                benchmark_phase_json(
                    "background_event_batch_coalesce_apply",
                    event_batch_ms,
                    frames,
                    event_batch_checksum,
                ),
                benchmark_phase_json(
                    "background_event_forwarder_flood_collect",
                    event_forwarder_flood_ms,
                    frames,
                    event_forwarder_flood_checksum,
                ),
                benchmark_phase_json(
                    "hero_boundary_scroll_redraw",
                    hero_boundary_scroll_ms,
                    frames,
                    hero_boundary_checksum,
                ),
                benchmark_phase_json(
                    "action_tool_input_batch_no_redraw",
                    action_input_ms,
                    frames,
                    action_input_checksum,
                ),
                benchmark_phase_json(
                    "action_tool_visible_redraw",
                    action_visible_ms,
                    frames,
                    action_visible_checksum,
                ),
                benchmark_phase_json(
                    "workspace_navigation_geometry",
                    workspace_navigation_ms,
                    frames,
                    workspace_navigation_checksum,
                ),
                benchmark_phase_json(
                    "workspace_full_frame_scroll_attributed",
                    workspace_full_frame_ms,
                    frames,
                    workspace_full_frame_checksum,
                ),
                benchmark_phase_json(
                    "large_transcript_scroll_visible_body_only",
                    large_scroll_ms,
                    frames,
                    large_scroll_checksum,
                ),
                benchmark_phase_json(
                    "large_transcript_cache_key",
                    large_cache_key_ms,
                    frames,
                    large_cache_key_checksum,
                ),
            ],
            "workspace_navigation_subphases": {
                "visible_surface_count_mean": workspace_visible_surfaces as f64 / frames as f64,
                "layout": benchmark_phase_json("workspace_layout", workspace_layout_ms, frames, 0),
                "vertices": benchmark_phase_json("workspace_vertices", workspace_vertices_ms, frames, 0),
            },
            "workspace_full_frame_subphases": {
                "surfaces_total": workspace_full_app.surfaces.len(),
                "visible_surface_count_mean": workspace_full_visible_surfaces as f64 / frames as f64,
                "text_pane_count_mean": workspace_full_text_pane_count as f64 / frames as f64,
                "text_area_count_mean": workspace_full_text_area_count as f64 / frames as f64,
                "layout": benchmark_phase_json("workspace_full_layout", workspace_full_layout_ms, frames, 0),
                "vertices": benchmark_phase_json("workspace_full_vertices", workspace_full_vertices_ms, frames, 0),
                "text_panes": benchmark_phase_json("workspace_full_text_panes", workspace_full_text_panes_ms, frames, 0),
                "text_areas": benchmark_phase_json("workspace_full_text_areas", workspace_full_text_areas_ms, frames, 0),
            },
            "visible_whole_line_subphases": [
                benchmark_phase_json("viewport", visible_viewport_ms, frames, 0),
                benchmark_phase_json("window", visible_window_ms, frames, 0),
                benchmark_phase_json("set_scroll", visible_scroll_ms, frames, 0),
                benchmark_phase_json("glyph_runs", visible_glyph_ms, frames, 0),
                benchmark_phase_json("areas", visible_areas_ms, frames, 0),
                benchmark_phase_json("vertices", visible_vertices_ms, frames, 0),
            ],
            "cold_start_cpu": [
                benchmark_phase_json("fresh_welcome_cold_text_frame", cold_fresh_ms, 1, cold_fresh_checksum),
                benchmark_phase_json("fresh_welcome_prewarmed_text_frame", prewarmed_fresh_ms, 1, prewarmed_fresh_checksum),
                benchmark_phase_json("fresh_welcome_warm_cached_text_frame", warm_fresh_ms, 1, warm_fresh_checksum),
            ],
            "cold_start_subphases": [
                benchmark_phase_json("font_system", cold_fresh_font_ms, 1, 0),
                benchmark_phase_json("text_key", cold_fresh_key_ms, 1, 0),
                benchmark_phase_json("text_buffers", cold_fresh_buffers_ms, 1, 0),
                benchmark_phase_json("text_areas", cold_fresh_areas_ms, 1, 0),
                benchmark_phase_json("vertices", cold_fresh_vertices_ms, 1, 0),
            ],
            "typing_redraw_subphases": [
                benchmark_phase_json("text_cache", typing_text_cache_ms, frames, 0),
                benchmark_phase_json("areas", typing_areas_ms, frames, 0),
                benchmark_phase_json("vertices", typing_vertices_ms, frames, 0),
            ],
            "fresh_welcome_typing_subphases": [
                benchmark_phase_json("text_cache", fresh_typing_text_cache_ms, frames, 0),
                benchmark_phase_json("areas", fresh_typing_areas_ms, frames, 0),
                benchmark_phase_json("vertices", fresh_typing_vertices_ms, frames, 0),
            ],
            "streaming_delta_subphases": [
                benchmark_phase_json("body_wrap", streaming_body_ms, frames, 0),
                benchmark_phase_json("text_cache", streaming_text_cache_ms, frames, 0),
                benchmark_phase_json("areas", streaming_areas_ms, frames, 0),
                benchmark_phase_json("vertices", streaming_vertices_ms, frames, 0),
            ],
            "streaming_incremental_wrap": {
                "static_base_rebuilds": streaming_static_base_rebuilds,
                "tail_text_buffer_rebuilds": streaming_tail_text_buffer_rebuilds,
                "static_base_rebuild_budget": 1,
                "passes_static_base_rebuild_budget": streaming_static_base_rebuilds <= 1,
            },
            "hero_boundary": {
                "start_scroll_lines": hero_boundary_scroll,
                "body_buffer_rebuilds": hero_body_buffer_rebuilds,
                "subphases": [
                    benchmark_phase_json("text_cache", hero_text_cache_ms, frames, 0),
                    benchmark_phase_json("viewport_and_key", hero_viewport_key_ms, frames, 0),
                    benchmark_phase_json("body_window_rebuild", hero_window_rebuild_ms, frames, hero_body_buffer_rebuilds),
                    benchmark_phase_json("reuse_text_buffers", hero_buffer_reuse_ms, frames, 0),
                    benchmark_phase_json("areas", hero_areas_ms, frames, 0),
                    benchmark_phase_json("vertices", hero_vertices_ms, frames, 0),
                ],
            },
            "action_tool_visible_subphases": [
                benchmark_phase_json("event_apply_body_mutation", action_apply_ms, frames, 0),
                benchmark_phase_json("body_wrap", action_body_ms, frames, 0),
                benchmark_phase_json("text_cache", action_text_cache_ms, frames, 0),
                benchmark_phase_json("areas", action_areas_ms, frames, 0),
                benchmark_phase_json("vertices", action_vertices_ms, frames, 0),
            ],
            "large_transcript_setup": benchmark_phase_json(
                "large_transcript_initial_body_wrap",
                large_body_elapsed.as_secs_f64() * 1000.0,
                1,
                large_body_lines.len(),
            ),
        }))?
    );
    Ok(())
}

fn run_stream_e2e_benchmark(raw_events: usize) -> Result<()> {
    let result = run_desktop_stream_end_to_end_benchmark(raw_events);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "target_frame_budget_ms": duration_ms(DESKTOP_120FPS_FRAME_BUDGET),
            "no_paint_budget_ms": duration_ms(DESKTOP_NO_PAINT_BUDGET),
            "event_delivery": {
                "backend_redraw_frame_interval_ms": duration_ms(BACKEND_REDRAW_FRAME_INTERVAL),
                "backend_event_forward_interval_ms": duration_ms(BACKEND_EVENT_FORWARD_INTERVAL),
                "backend_event_forward_max_raw_events": BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS,
                "backend_event_forward_max_payload_bytes": BACKEND_EVENT_FORWARD_MAX_PAYLOAD_BYTES,
            },
            "passes_120fps_interaction_cpu_budget": result.passes_interaction_budget(),
            "passes_no_paint_watchdog_budget": result.passes_no_paint_budget(),
            "end_to_end_stream_flood": result.to_json(),
        }))?
    );
    Ok(())
}

fn benchmark_hero_boundary_scroll_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    body_lines: &[SingleSessionStyledLine],
) -> f32 {
    let Some(metrics) =
        single_session_body_scroll_metrics_for_total_lines(app, size, body_lines.len())
    else {
        return 0.0;
    };
    let mut probe = app.clone();
    for scroll in 0..=metrics.max_scroll_lines {
        probe.body_scroll_lines = scroll as f32;
        let key =
            single_session_text_key_for_tick_with_rendered_body(&probe, size, 0, 0.0, body_lines);
        if key.fresh_welcome_visible {
            return scroll.saturating_sub(6) as f32;
        }
    }
    metrics.max_scroll_lines.saturating_sub(12) as f32
}

fn benchmark_font_system() -> FontSystem {
    create_desktop_font_system()
}

fn create_desktop_font_system() -> FontSystem {
    let mut font_system = FontSystem::new();
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/Kalam-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/ShadowsIntoLightTwo-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/HomemadeApple-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/PatrickHand-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/Gaegu-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/Caveat-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/IndieFlower-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/GloriaHallelujah-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/Handlee-Regular.ttf").to_vec());
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../assets/fonts/ReenieBeanie-Regular.ttf").to_vec());
    font_system
}

fn spawn_desktop_font_system_loader() -> JoinHandle<FontSystem> {
    std::thread::spawn(create_desktop_font_system)
}

#[cfg(target_os = "linux")]
fn desktop_wgpu_startup_backends() -> Vec<wgpu::Backends> {
    vec![wgpu::Backends::PRIMARY]
}

#[cfg(not(target_os = "linux"))]
fn desktop_wgpu_startup_backends() -> Vec<wgpu::Backends> {
    vec![wgpu::Backends::PRIMARY]
}

async fn request_startup_adapter(
    window: Arc<Window>,
    backend_candidates: Vec<wgpu::Backends>,
    startup_trace: DesktopStartupTrace,
) -> Result<(wgpu::Surface<'static>, wgpu::Adapter)> {
    let mut last_error = None;
    for backends in backend_candidates {
        let backend_label = format!("{backends:?}");
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::empty().with_env(),
            ..Default::default()
        });
        startup_trace.mark(&format!("wgpu instance created ({backend_label})"));
        let surface = match instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                last_error = Some(format!(
                    "{backend_label}: failed to create surface: {error:#}"
                ));
                continue;
            }
        };
        startup_trace.mark(&format!("wgpu surface created ({backend_label})"));
        if let Some(adapter) = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
        {
            startup_trace.mark(&format!("wgpu adapter selected ({backend_label})"));
            return Ok((surface, adapter));
        }
        last_error = Some(format!("{backend_label}: no compatible adapter"));
    }

    match last_error {
        Some(error) => anyhow::bail!("failed to find a compatible GPU adapter ({error})"),
        None => anyhow::bail!("failed to find a compatible GPU adapter"),
    }
}

fn desktop_scroll_benchmark_app() -> SingleSessionApp {
    desktop_scroll_benchmark_app_with_turns(320)
}

fn desktop_large_transcript_benchmark_app() -> SingleSessionApp {
    desktop_scroll_benchmark_app_with_turns(2_000)
}

fn benchmark_workspace_session_cards(count: usize) -> Vec<workspace::SessionCard> {
    (0..count)
        .map(|index| workspace::SessionCard {
            session_id: format!("benchmark-session-{index}"),
            title: format!("agent {index} · desktop benchmark"),
            subtitle: format!("workspace surface {index}"),
            detail: "rendering session metadata, preview lines, and detail transcript".to_string(),
            preview_lines: vec![
                "recent prompt: inspect render latency and input jank".to_string(),
                "assistant: caching text and geometry keeps navigation responsive".to_string(),
                format!("status: benchmark card {index}"),
            ],
            detail_lines: (0..16)
                .map(|line| {
                    format!(
                        "detail line {line}: this synthetic transcript line exercises zoom/detail rendering for card {index}"
                    )
                })
                .collect(),
            transcript_messages: Vec::new(),
        })
        .collect()
}

fn desktop_scroll_benchmark_app_with_turns(turns: usize) -> SingleSessionApp {
    let mut app = SingleSessionApp::new(None);
    app.draft = "summarize the latest changes and suggest the next optimization".to_string();
    app.draft_cursor = app.draft.len();
    for turn in 0..turns {
        app.messages.push(SingleSessionMessage::user(format!(
            "Prompt {turn}: inspect this desktop render path and explain where scroll jank may come from."
        )));
        app.messages.push(SingleSessionMessage::assistant(format!(
            "Assistant response {turn}: The render path includes markdown wrapping, transcript card geometry, scrollbar geometry, text buffer preparation, and GPU primitive upload. This paragraph is intentionally long enough to wrap across several desktop body lines so the benchmark exercises visible-line virtualization and wrapping behavior.\n\n- Check cached text keys.\n- Check smooth scroll fractional offsets.\n- Check whether geometry can update without reshaping text.\n\n```rust\nfn sample_{turn}() {{ println!(\"benchmark\"); }}\n```"
        )));
    }
    app.set_status_label("benchmark fixture");
    app
}

fn load_desktop_preferences() -> Option<workspace::DesktopPreferences> {
    match desktop_prefs::load_preferences() {
        Ok(preferences) => preferences,
        Err(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to load desktop preferences: {error:#}"
            ));
            None
        }
    }
}

fn fresh_single_session_app() -> DesktopApp {
    DesktopApp::SingleSession(SingleSessionApp::new(None))
}

fn fresh_desktop_app_for_worker_mode(mode: DesktopWorkerMode) -> DesktopApp {
    match mode {
        DesktopWorkerMode::SingleSession => fresh_single_session_app(),
        DesktopWorkerMode::Workspace => DesktopApp::Workspace(Workspace::loading_sessions()),
    }
}

fn initial_single_session_app(resume_session_id: Option<&str>) -> DesktopApp {
    let Some(session_id) = resume_session_id else {
        return fresh_single_session_app();
    };

    let mut app = SingleSessionApp::new(None);
    app.initialize_resumed_session(session_id);
    match session_data::load_session_card_by_id(session_id) {
        Ok(Some(card)) => {
            app.replace_session(Some(card));
            app.hydrate_resumed_session_from_disk(session_id);
        }
        Ok(None) => {
            app.set_status_label(format!("resumed session {session_id}"));
            app.hydrate_resumed_session_from_disk(session_id);
        }
        Err(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to load resumed session metadata for {session_id}: {error:#}"
            ));
            app.set_status_label(format!("resumed session {session_id}"));
            app.error = Some(format!("failed to load session metadata: {error:#}"));
        }
    }
    DesktopApp::SingleSession(app)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopMode {
    SingleSession,
    WorkspacePrototype,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopProcessRole {
    Standalone,
    StableHost,
    AppWorker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopReloadStrategy {
    FullProcessHandoff,
    AppWorkerRestart,
}

impl DesktopProcessRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::StableHost => "stable_host",
            Self::AppWorker => "app_worker",
        }
    }

    fn reload_strategy(self) -> DesktopReloadStrategy {
        match self {
            Self::Standalone | Self::AppWorker => DesktopReloadStrategy::FullProcessHandoff,
            Self::StableHost => DesktopReloadStrategy::AppWorkerRestart,
        }
    }
}

impl DesktopMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::SingleSession => "single_session",
            Self::WorkspacePrototype => "workspace",
        }
    }

    fn worker_mode(self) -> DesktopWorkerMode {
        match self {
            Self::SingleSession => DesktopWorkerMode::SingleSession,
            Self::WorkspacePrototype => DesktopWorkerMode::Workspace,
        }
    }
}

fn run_desktop_app_worker_process(desktop_mode: DesktopMode) -> Result<()> {
    desktop_log::info(format_args!(
        "jcode-desktop: app worker process started; pid={}",
        std::process::id()
    ));

    let mut stdout = std::io::stdout().lock();
    let ready = DesktopProtocolEnvelope::new(
        1,
        DesktopWorkerToHostMessage::Ready(DesktopWorkerReady {
            worker_pid: std::process::id(),
            mode: desktop_mode.worker_mode(),
        }),
    );
    write_desktop_ipc_frame(&mut stdout, &ready).context("failed to write worker ready frame")?;

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut runtime: Option<DesktopAppRuntime<DesktopApp>> = None;
    let mut latest_window = DesktopWindowState {
        width: DEFAULT_WINDOW_WIDTH as u32,
        height: DEFAULT_WINDOW_HEIGHT as u32,
        scale_factor: 1.0,
        focused: true,
    };
    let mut next_worker_sequence = 2;
    loop {
        let frame: Option<DesktopHostToWorkerEnvelope> =
            desktop_ipc::read_desktop_ipc_frame(&mut reader)
                .context("failed to read host frame")?;
        let Some(frame) = frame else {
            break;
        };
        frame
            .validate_version()
            .context("host sent incompatible protocol frame")?;
        match frame.payload {
            DesktopHostToWorkerMessage::Initialize(init) => {
                latest_window = init.window.clone();
                let mut app = fresh_desktop_app_for_worker_mode(init.mode);
                if let Some(snapshot) = init.snapshot.clone()
                    && let Err(error) = app.restore_snapshot(snapshot)
                {
                    desktop_log::error(format_args!(
                        "jcode-desktop: app worker failed to restore host snapshot: {error:#}"
                    ));
                }
                let app_runtime = DesktopAppRuntime::new(app);
                let scene = desktop_scene_for_worker_runtime(&app_runtime, &latest_window);
                runtime = Some(app_runtime);
                let scene_update = DesktopProtocolEnvelope::new(
                    next_worker_sequence,
                    DesktopWorkerToHostMessage::Scene(DesktopSceneUpdate {
                        animation_active: scene.metadata.animation_active,
                        scene,
                    }),
                );
                next_worker_sequence += 1;
                write_desktop_ipc_frame(&mut stdout, &scene_update)
                    .context("failed to write worker initial scene")?;
            }
            DesktopHostToWorkerMessage::SnapshotRequest { request_id } => {
                if let Some(runtime) = runtime.as_ref() {
                    let snapshot = DesktopProtocolEnvelope::new(
                        next_worker_sequence,
                        DesktopWorkerToHostMessage::Snapshot(DesktopSnapshotResponse {
                            request_id,
                            snapshot: runtime.snapshot(),
                        }),
                    );
                    next_worker_sequence += 1;
                    write_desktop_ipc_frame(&mut stdout, &snapshot)
                        .context("failed to write worker snapshot response")?;
                } else {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker received snapshot request {request_id} before initialization"
                    ));
                }
            }
            DesktopHostToWorkerMessage::Shutdown {
                reason:
                    DesktopWorkerShutdownReason::HostExit
                    | DesktopWorkerShutdownReason::Reload
                    | DesktopWorkerShutdownReason::ProtocolMismatch,
            } => break,
            DesktopHostToWorkerMessage::Input(input) => {
                let mut changed = false;
                match input {
                    DesktopInputEvent::Key(key) => {
                        if key.pressed
                            && let Some(runtime) = runtime.as_mut()
                        {
                            let outcome =
                                runtime.handle_key_input(desktop_key_event_to_key_input(&key));
                            if matches!(outcome, KeyOutcome::ForceReload) {
                                let reload_requested = DesktopProtocolEnvelope::new(
                                    next_worker_sequence,
                                    DesktopWorkerToHostMessage::ReloadRequested,
                                );
                                next_worker_sequence += 1;
                                write_desktop_ipc_frame(&mut stdout, &reload_requested)
                                    .context("failed to write worker reload request")?;
                            } else {
                                changed = true;
                            }
                        }
                    }
                    DesktopInputEvent::Window(DesktopWindowEvent::Resized {
                        width,
                        height,
                        scale_factor,
                    }) => {
                        latest_window.width = width;
                        latest_window.height = height;
                        latest_window.scale_factor = scale_factor;
                        changed = true;
                    }
                    DesktopInputEvent::Window(DesktopWindowEvent::Focused(focused)) => {
                        latest_window.focused = focused;
                    }
                    DesktopInputEvent::Mouse(_) => {}
                }
                if changed && let Some(runtime) = runtime.as_ref() {
                    write_worker_scene_update(
                        &mut stdout,
                        &mut next_worker_sequence,
                        runtime,
                        &latest_window,
                    )
                    .context("failed to write worker input scene")?;
                }
            }
            DesktopHostToWorkerMessage::SessionEvents(batch) => {
                let mut changed = false;
                if let Some(runtime) = runtime.as_mut() {
                    for event in batch.events {
                        if let Some(session_event) =
                            desktop_wire_session_event_to_runtime_event(event)
                        {
                            runtime.apply_session_event(session_event);
                            changed = true;
                        }
                    }
                }
                if changed && let Some(runtime) = runtime.as_ref() {
                    write_worker_scene_update(
                        &mut stdout,
                        &mut next_worker_sequence,
                        runtime,
                        &latest_window,
                    )
                    .context("failed to write worker session event scene")?;
                }
            }
            DesktopHostToWorkerMessage::MetricsAck { .. } => {}
        }
    }

    Ok(())
}

#[cfg(test)]
fn desktop_scene_for_worker_init(init: &DesktopWorkerInit) -> DesktopScene {
    let mut scene = DesktopScene::new(DesktopSceneViewport::new(
        init.window.width as f32,
        init.window.height as f32,
        init.window.scale_factor,
    ));
    scene.metadata.title = init
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.title.clone());
    scene.metadata.content_ready = init.snapshot.is_some();
    scene.push(DesktopDisplayCommand::Clear(DesktopColor::rgba(
        0.02, 0.024, 0.03, 1.0,
    )));
    scene
}

fn desktop_scene_for_worker_runtime(
    runtime: &DesktopAppRuntime<DesktopApp>,
    window: &DesktopWindowState,
) -> DesktopScene {
    let mut scene = DesktopScene::new(DesktopSceneViewport::new(
        window.width as f32,
        window.height as f32,
        window.scale_factor,
    ));
    scene.push(DesktopDisplayCommand::Clear(DesktopColor::rgba(
        0.02, 0.024, 0.03, 1.0,
    )));
    runtime.build_scene(scene)
}

fn write_worker_scene_update(
    stdout: &mut impl Write,
    next_worker_sequence: &mut u64,
    runtime: &DesktopAppRuntime<DesktopApp>,
    window: &DesktopWindowState,
) -> Result<()> {
    let scene = desktop_scene_for_worker_runtime(runtime, window);
    let scene_update = DesktopProtocolEnvelope::new(
        *next_worker_sequence,
        DesktopWorkerToHostMessage::Scene(DesktopSceneUpdate {
            animation_active: scene.metadata.animation_active,
            scene,
        }),
    );
    *next_worker_sequence += 1;
    write_desktop_ipc_frame(stdout, &scene_update)?;
    Ok(())
}

fn desktop_key_event_to_key_input(event: &DesktopKeyEvent) -> KeyInput {
    let modifiers = desktop_key_modifiers_to_winit(event.modifiers);
    let key = desktop_key_string_to_winit_key(&event.key, event.text.as_deref());
    to_key_input(&key, modifiers)
}

fn desktop_key_modifiers_to_winit(modifiers: DesktopKeyModifiers) -> ModifiersState {
    let mut state = ModifiersState::empty();
    if modifiers.shift {
        state |= ModifiersState::SHIFT;
    }
    if modifiers.ctrl {
        state |= ModifiersState::CONTROL;
    }
    if modifiers.alt {
        state |= ModifiersState::ALT;
    }
    if modifiers.super_key {
        state |= ModifiersState::SUPER;
    }
    state
}

fn desktop_key_string_to_winit_key(key: &str, text: Option<&str>) -> Key {
    match key {
        "Escape" => Key::Named(NamedKey::Escape),
        "Enter" => Key::Named(NamedKey::Enter),
        "Tab" => Key::Named(NamedKey::Tab),
        "Backspace" => Key::Named(NamedKey::Backspace),
        "Delete" => Key::Named(NamedKey::Delete),
        "PageUp" => Key::Named(NamedKey::PageUp),
        "PageDown" => Key::Named(NamedKey::PageDown),
        "ArrowUp" => Key::Named(NamedKey::ArrowUp),
        "ArrowDown" => Key::Named(NamedKey::ArrowDown),
        "ArrowLeft" => Key::Named(NamedKey::ArrowLeft),
        "ArrowRight" => Key::Named(NamedKey::ArrowRight),
        "Home" => Key::Named(NamedKey::Home),
        "End" => Key::Named(NamedKey::End),
        "Space" => Key::Named(NamedKey::Space),
        _ => Key::Character(text.unwrap_or(key).to_string().into()),
    }
}

fn desktop_wire_session_event_to_runtime_event(
    event: DesktopSessionEventWire,
) -> Option<session_launch::DesktopSessionEvent> {
    match event {
        DesktopSessionEventWire::Status { message } => Some(
            session_launch::DesktopSessionEvent::Status(DesktopSessionStatus::external(message)),
        ),
        DesktopSessionEventWire::AssistantTextDelta { text } => {
            Some(session_launch::DesktopSessionEvent::TextDelta(text))
        }
        DesktopSessionEventWire::ToolStarted { id, title } => {
            Some(session_launch::DesktopSessionEvent::ToolStarted {
                id: (!id.is_empty()).then_some(id),
                name: title,
            })
        }
        DesktopSessionEventWire::ToolFinished { id, title, success } => {
            Some(session_launch::DesktopSessionEvent::ToolFinished {
                id: (!id.is_empty()).then_some(id),
                name: title,
                summary: String::new(),
                is_error: !success,
            })
        }
        DesktopSessionEventWire::Error { message } => {
            Some(session_launch::DesktopSessionEvent::Error(message))
        }
        DesktopSessionEventWire::RawJson { .. } => None,
    }
}

fn desktop_mode_from_args<'a>(args: impl IntoIterator<Item = &'a str>) -> DesktopMode {
    if args.into_iter().any(|arg| arg == "--workspace") {
        DesktopMode::WorkspacePrototype
    } else {
        DesktopMode::SingleSession
    }
}

fn desktop_process_role_from_args<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> DesktopProcessRole {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let role = arg
            .strip_prefix("--desktop-process-role=")
            .or_else(|| {
                (arg == "--desktop-process-role")
                    .then(|| args.next())
                    .flatten()
            })
            .or_else(|| {
                (arg == "--desktop-host")
                    .then_some("host")
                    .or_else(|| (arg == "--desktop-app-worker").then_some("worker"))
            });
        if let Some(role) = role {
            return match role {
                "host" | "stable-host" | "stable_host" => DesktopProcessRole::StableHost,
                "worker" | "app-worker" | "app_worker" => DesktopProcessRole::AppWorker,
                _ => DesktopProcessRole::Standalone,
            };
        }
    }
    DesktopProcessRole::StableHost
}

fn desktop_resume_session_id_from_args<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--resume" {
            return args.next().map(str::to_string);
        }
        if let Some(session_id) = arg.strip_prefix("--resume=") {
            return (!session_id.is_empty()).then(|| session_id.to_string());
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DesktopReloadWindowPlacement {
    position: Option<PhysicalPosition<i32>>,
    inner_size: PhysicalSize<u32>,
}

impl DesktopReloadWindowPlacement {
    fn from_window(window: &Window) -> Option<Self> {
        let inner_size = window.inner_size();
        if !desktop_reload_window_size_is_valid(inner_size) {
            return None;
        }
        Some(Self {
            position: window.outer_position().ok(),
            inner_size,
        })
    }

    fn from_env_value(raw: &str) -> Option<Self> {
        let parts = raw.split(',').collect::<Vec<_>>();
        if parts.len() != 4 {
            return None;
        }

        let position = match (parts[0], parts[1]) {
            ("_", "_") => None,
            (x, y) => Some(PhysicalPosition::new(x.parse().ok()?, y.parse().ok()?)),
        };
        let inner_size = PhysicalSize::new(parts[2].parse().ok()?, parts[3].parse().ok()?);
        if !desktop_reload_window_size_is_valid(inner_size) {
            return None;
        }
        Some(Self {
            position,
            inner_size,
        })
    }

    fn to_env_value(self) -> String {
        let (x, y) = match self.position {
            Some(position) => (position.x.to_string(), position.y.to_string()),
            None => ("_".to_string(), "_".to_string()),
        };
        format!(
            "{x},{y},{},{}",
            self.inner_size.width, self.inner_size.height
        )
    }

    fn apply_to_window_builder(self, mut window_builder: WindowBuilder) -> WindowBuilder {
        window_builder = window_builder.with_inner_size(self.inner_size);
        if let Some(position) = self.position {
            window_builder = window_builder.with_position(position);
        }
        window_builder
    }
}

fn desktop_reload_window_size_is_valid(size: PhysicalSize<u32>) -> bool {
    (1..=DESKTOP_RELOAD_MAX_RESTORED_DIMENSION).contains(&size.width)
        && (1..=DESKTOP_RELOAD_MAX_RESTORED_DIMENSION).contains(&size.height)
}

#[derive(Clone, Debug, Default)]
struct DesktopReloadStartup {
    window_placement: Option<DesktopReloadWindowPlacement>,
    handoff: Option<DesktopReloadStartupHandoff>,
}

impl DesktopReloadStartup {
    fn from_env() -> Self {
        let raw_window_placement = std::env::var(DESKTOP_RELOAD_WINDOW_ENV).ok();
        let ready_file = std::env::var_os(DESKTOP_RELOAD_HANDOFF_READY_ENV).map(PathBuf::from);
        let release_file = std::env::var_os(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV).map(PathBuf::from);
        unsafe {
            std::env::remove_var(DESKTOP_RELOAD_WINDOW_ENV);
            std::env::remove_var(DESKTOP_RELOAD_HANDOFF_READY_ENV);
            std::env::remove_var(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV);
        }

        let window_placement = raw_window_placement.as_deref().and_then(|raw| {
            let placement = DesktopReloadWindowPlacement::from_env_value(raw);
            if placement.is_none() {
                desktop_log::warn(format_args!(
                    "jcode-desktop: ignoring invalid reload window placement {raw:?}"
                ));
            }
            placement
        });
        let handoff = match (ready_file, release_file) {
            (Some(ready_file), Some(release_file)) => Some(DesktopReloadStartupHandoff {
                ready_file,
                release_file,
            }),
            (None, None) => None,
            _ => {
                desktop_log::warn(format_args!(
                    "jcode-desktop: ignoring incomplete reload handoff environment"
                ));
                None
            }
        };

        Self {
            window_placement,
            handoff,
        }
    }

    fn hidden_until_handoff_release(&self) -> bool {
        self.handoff.is_some()
    }
}

#[derive(Clone, Debug)]
struct DesktopReloadStartupHandoff {
    ready_file: PathBuf,
    release_file: PathBuf,
}

impl DesktopReloadStartupHandoff {
    fn signal_ready_and_wait_for_release(&self) {
        if let Err(error) = write_desktop_reload_marker(&self.ready_file) {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to signal reload readiness: {error:#}"
            ));
            return;
        }

        desktop_log::info(format_args!(
            "jcode-desktop: reload child ready, waiting for parent release"
        ));
        let deadline = Instant::now() + DESKTOP_RELOAD_STARTUP_RELEASE_TIMEOUT;
        while Instant::now() < deadline {
            if self.release_file.exists() {
                cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
                return;
            }
            std::thread::sleep(DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL);
        }

        desktop_log::warn(format_args!(
            "jcode-desktop: reload parent did not release handoff within {}ms; showing replacement window anyway",
            DESKTOP_RELOAD_STARTUP_RELEASE_TIMEOUT.as_millis()
        ));
        cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
    }
}

#[derive(Clone, Debug)]
struct DesktopReloadHandoff {
    ready_file: PathBuf,
    release_file: PathBuf,
    window_placement: Option<DesktopReloadWindowPlacement>,
}

impl DesktopReloadHandoff {
    fn new(window: &Window) -> Result<Self> {
        let dir = desktop_reload_handoff_temp_dir();
        fs::create_dir_all(&dir).with_context(|| {
            format!(
                "failed to create desktop reload handoff directory {}",
                dir.display()
            )
        })?;
        Ok(Self {
            ready_file: dir.join("ready"),
            release_file: dir.join("release"),
            window_placement: DesktopReloadWindowPlacement::from_window(window),
        })
    }

    fn apply_to_command(&self, command: &mut Command) {
        if let Some(placement) = self.window_placement {
            command.env(DESKTOP_RELOAD_WINDOW_ENV, placement.to_env_value());
        }
        command.env(DESKTOP_RELOAD_HANDOFF_READY_ENV, &self.ready_file);
        command.env(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV, &self.release_file);
    }

    fn watcher(&self) -> DesktopReloadHandoffWatcher {
        DesktopReloadHandoffWatcher {
            ready_file: self.ready_file.clone(),
            release_file: self.release_file.clone(),
            spawned_at: Instant::now(),
        }
    }

    fn cleanup(&self) {
        cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
    }
}

#[derive(Clone, Debug)]
struct DesktopReloadHandoffWatcher {
    ready_file: PathBuf,
    release_file: PathBuf,
    spawned_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopReloadHandoffPoll {
    Waiting,
    Ready,
    TimedOut,
}

impl DesktopReloadHandoffWatcher {
    fn poll(&self) -> Result<DesktopReloadHandoffPoll> {
        if self.ready_file.exists() {
            write_desktop_reload_marker(&self.release_file)?;
            return Ok(DesktopReloadHandoffPoll::Ready);
        }
        if self.spawned_at.elapsed() >= DESKTOP_RELOAD_HANDOFF_TIMEOUT {
            return Ok(DesktopReloadHandoffPoll::TimedOut);
        }
        Ok(DesktopReloadHandoffPoll::Waiting)
    }

    fn cleanup(&self) {
        cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
    }
}

fn desktop_reload_handoff_temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "jcode-desktop-reload-{}-{nonce}",
        std::process::id()
    ))
}

fn write_desktop_reload_marker(path: &Path) -> Result<()> {
    fs::write(path, format!("{}\n", std::process::id()))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn cleanup_desktop_reload_handoff_files(ready_file: &Path, release_file: &Path) {
    let _ = fs::remove_file(ready_file);
    let _ = fs::remove_file(release_file);
    if ready_file.parent() == release_file.parent()
        && let Some(parent) = ready_file.parent()
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("jcode-desktop-reload-"))
    {
        let _ = fs::remove_dir(parent);
    }
}

struct DesktopHotReloader {
    relaunch: Option<DesktopRelaunch>,
    strategy: DesktopReloadStrategy,
    observed_modified: Option<std::time::SystemTime>,
    last_checked: Instant,
    pending_handoff: Option<DesktopReloadHandoffWatcher>,
    app_worker: Option<DesktopWorkerConnection>,
}

#[derive(Default)]
struct DesktopWorkerDrain {
    latest_scene: Option<DesktopScene>,
    reload_requested: bool,
}

impl DesktopHotReloader {
    const CHECK_INTERVAL: Duration = Duration::from_millis(750);

    fn new(strategy: DesktopReloadStrategy) -> Self {
        let relaunch = DesktopRelaunch::from_current_process();
        let observed_modified = relaunch.as_ref().and_then(|relaunch| {
            binary_modified_time(&desktop_reload_binary_candidate(&relaunch.binary))
        });
        Self {
            relaunch,
            strategy,
            observed_modified,
            last_checked: Instant::now(),
            pending_handoff: None,
            app_worker: None,
        }
    }

    fn next_wake(&self, now: Instant) -> Option<Instant> {
        if self.pending_handoff.is_some() {
            return Some(now + DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL);
        }
        if self.app_worker.is_some() {
            return Some(now + DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL);
        }
        self.relaunch.as_ref()?;
        Some(std::cmp::max(now, self.last_checked + Self::CHECK_INTERVAL))
    }

    fn drain_app_worker_messages(&mut self) -> DesktopWorkerDrain {
        let Some(worker) = self.app_worker.as_ref() else {
            return DesktopWorkerDrain::default();
        };
        let mut drained = DesktopWorkerDrain::default();
        while let Some(message) = worker.try_recv() {
            match message {
                Ok(DesktopWorkerToHostMessage::Ready(ready)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker ready; pid={} mode={:?}",
                        ready.worker_pid, ready.mode
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Scene(scene_update)) => {
                    drained.latest_scene = Some(scene_update.scene);
                }
                Ok(DesktopWorkerToHostMessage::ReloadRequested) => {
                    drained.reload_requested = true;
                }
                Ok(DesktopWorkerToHostMessage::Snapshot(snapshot)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker snapshot response {}; mode={}",
                        snapshot.request_id, snapshot.snapshot.mode
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Metrics(metrics)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker reported {} metric(s)",
                        metrics.metrics.len()
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Log(log)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker log {:?}: {}",
                        log.level, log.message
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Exited(exit)) => {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: app worker exited code={:?} reason={:?}",
                        exit.code, exit.reason
                    ));
                }
                Err(error) => {
                    desktop_log::error(format_args!(
                        "jcode-desktop: failed to read app worker message: {error:#}"
                    ));
                    break;
                }
            }
        }
        drained
    }

    fn has_app_worker(&self) -> bool {
        self.app_worker.is_some()
    }

    fn send_app_worker_input(&mut self, input: DesktopInputEvent) -> Result<()> {
        self.send_app_worker_message(DesktopHostToWorkerMessage::Input(input))
    }

    fn send_app_worker_message(&mut self, message: DesktopHostToWorkerMessage) -> Result<()> {
        let Some(worker) = self.app_worker.as_mut() else {
            return Ok(());
        };
        worker.send(message)
    }

    fn start_app_worker_for_current_binary(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        reason: &'static str,
    ) {
        let Some(relaunch) = self.relaunch.clone() else {
            desktop_log::warn(format_args!(
                "jcode-desktop: cannot start app worker for {reason}; current process cannot be relaunched"
            ));
            return;
        };
        let binary = desktop_reload_binary_candidate(&relaunch.binary);
        self.restart_app_worker(app, window, &relaunch, binary, reason);
    }

    fn poll(&mut self, app: &DesktopApp, window: &Window) -> bool {
        if self.poll_pending_handoff() {
            return true;
        }
        if self.pending_handoff.is_some() {
            return false;
        }
        if self.last_checked.elapsed() < Self::CHECK_INTERVAL {
            return false;
        }
        self.last_checked = Instant::now();

        let Some(relaunch) = self.relaunch.clone() else {
            return false;
        };
        let binary = desktop_reload_binary_candidate(&relaunch.binary);
        let Some(current_modified) = binary_modified_time(&binary) else {
            return false;
        };
        let observed_modified = self.observed_modified;
        self.observed_modified = Some(current_modified);
        if observed_modified.is_some_and(|observed| current_modified > observed) {
            return self.reload_with_strategy(app, window, &relaunch, binary, "hot reload");
        }
        false
    }

    fn force_reload(&mut self, app: &DesktopApp, window: &Window) -> bool {
        if self.poll_pending_handoff() {
            return true;
        }
        if self.pending_handoff.is_some() {
            desktop_log::warn(format_args!(
                "jcode-desktop: force reload requested while another reload handoff is pending"
            ));
            return false;
        }
        let Some(relaunch) = self.relaunch.clone() else {
            desktop_log::warn(format_args!(
                "jcode-desktop: force reload requested but current process cannot be relaunched"
            ));
            return false;
        };
        let binary = desktop_reload_binary_candidate(&relaunch.binary);
        self.reload_with_strategy(app, window, &relaunch, binary, "force reload")
    }

    fn reload_with_strategy(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        relaunch: &DesktopRelaunch,
        binary: PathBuf,
        reason: &'static str,
    ) -> bool {
        match self.strategy {
            DesktopReloadStrategy::FullProcessHandoff => {
                self.reload_full_process_handoff(app, window, relaunch, binary, reason)
            }
            DesktopReloadStrategy::AppWorkerRestart => {
                desktop_log::info(format_args!(
                    "jcode-desktop: {reason} requested app-worker restart; keeping stable host window alive"
                ));
                self.restart_app_worker(app, window, relaunch, binary, reason);
                false
            }
        }
    }

    fn restart_app_worker(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        relaunch: &DesktopRelaunch,
        binary: PathBuf,
        reason: &'static str,
    ) {
        if let Some(worker) = self.app_worker.take()
            && let Err(error) = worker.kill()
        {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to stop previous app worker before {reason}: {error:#}"
            ));
        }

        let worker_relaunch = relaunch.for_app(app, binary).for_app_worker();
        match worker_relaunch.spawn_app_worker() {
            Ok(mut worker) => {
                if let Err(error) =
                    worker.send(DesktopHostToWorkerMessage::Initialize(DesktopWorkerInit {
                        mode: desktop_worker_mode_for_app(app),
                        snapshot: Some(app.snapshot()),
                        window: desktop_window_state(window),
                    }))
                {
                    desktop_log::error(format_args!(
                        "jcode-desktop: failed to initialize app worker for {reason}: {error:#}"
                    ));
                    if let Err(kill_error) = worker.kill() {
                        desktop_log::warn(format_args!(
                            "jcode-desktop: failed to kill uninitialized app worker: {kill_error:#}"
                        ));
                    }
                    return;
                }
                desktop_log::info(format_args!(
                    "jcode-desktop: app worker restarted for {reason}; pid={}",
                    worker.child_id()
                ));
                self.app_worker = Some(worker);
            }
            Err(error) => desktop_log::error(format_args!(
                "jcode-desktop: failed to restart app worker for {reason}: {error:#}"
            )),
        }
    }

    fn reload_full_process_handoff(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        relaunch: &DesktopRelaunch,
        binary: PathBuf,
        reason: &'static str,
    ) -> bool {
        let relaunch = relaunch.for_app(app, binary);
        match relaunch.spawn_for_window(window) {
            Ok(Some(handoff)) => {
                self.pending_handoff = Some(handoff);
                false
            }
            Ok(None) => true,
            Err(error) => {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to {reason} desktop: {error:#}"
                ));
                false
            }
        }
    }

    fn poll_pending_handoff(&mut self) -> bool {
        let Some(pending_handoff) = self.pending_handoff.as_ref() else {
            return false;
        };
        match pending_handoff.poll() {
            Ok(DesktopReloadHandoffPoll::Waiting) => false,
            Ok(DesktopReloadHandoffPoll::Ready) => {
                desktop_log::info(format_args!(
                    "jcode-desktop: reload replacement is ready; exiting old process"
                ));
                true
            }
            Ok(DesktopReloadHandoffPoll::TimedOut) => {
                desktop_log::warn(format_args!(
                    "jcode-desktop: reload replacement did not become ready within {}ms; keeping old process alive",
                    DESKTOP_RELOAD_HANDOFF_TIMEOUT.as_millis()
                ));
                if let Some(pending_handoff) = self.pending_handoff.take() {
                    pending_handoff.cleanup();
                }
                false
            }
            Err(error) => {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to release reload replacement: {error:#}"
                ));
                true
            }
        }
    }
}

fn desktop_worker_mode_for_app(app: &DesktopApp) -> DesktopWorkerMode {
    match app {
        DesktopApp::SingleSession(_) => DesktopWorkerMode::SingleSession,
        DesktopApp::Workspace(_) => DesktopWorkerMode::Workspace,
    }
}

fn desktop_window_state(window: &Window) -> DesktopWindowState {
    let size = window.inner_size();
    DesktopWindowState {
        width: size.width,
        height: size.height,
        scale_factor: window.scale_factor() as f32,
        focused: window.has_focus(),
    }
}

#[derive(Clone, Debug)]
struct DesktopRelaunch {
    binary: PathBuf,
    args: Vec<OsString>,
}

impl DesktopRelaunch {
    fn from_current_process() -> Option<Self> {
        let mut args = std::env::args_os();
        let argv0 = args.next()?;
        let binary = match resolve_invoked_binary(&argv0) {
            Some(binary) => binary,
            None => match std::env::current_exe() {
                Ok(binary) => binary,
                Err(_) => return None,
            },
        };
        Some(Self {
            binary,
            args: args.collect(),
        })
    }

    fn spawn_for_window(&self, window: &Window) -> Result<Option<DesktopReloadHandoffWatcher>> {
        let handoff = match DesktopReloadHandoff::new(window) {
            Ok(handoff) => Some(handoff),
            Err(error) => {
                desktop_log::warn(format_args!(
                    "jcode-desktop: reload handoff unavailable, falling back to immediate relaunch: {error:#}"
                ));
                None
            }
        };
        desktop_log::info(format_args!(
            "jcode-desktop: hot reloading into {} with args {:?}{}",
            self.binary.display(),
            self.args,
            if handoff.is_some() {
                " using handoff"
            } else {
                ""
            }
        ));
        let mut command = Command::new(&self.binary);
        command.args(&self.args);
        command.env_remove(DESKTOP_RELOAD_WINDOW_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_READY_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV);
        if let Some(handoff) = handoff.as_ref() {
            handoff.apply_to_command(&mut command);
        }
        if let Err(error) = command.spawn() {
            if let Some(handoff) = handoff.as_ref() {
                handoff.cleanup();
            }
            return Err(error)
                .with_context(|| format!("failed to spawn {}", self.binary.display()));
        }
        Ok(handoff.as_ref().map(DesktopReloadHandoff::watcher))
    }

    fn for_app(&self, app: &DesktopApp, binary: PathBuf) -> Self {
        if let DesktopApp::Workspace(workspace) = app
            && let Err(error) = desktop_prefs::save_preferences(&workspace.preferences())
        {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to persist workspace state before hot reload: {error:#}"
            ));
        }

        let mut args = desktop_args_without_resume(&self.args);
        match app {
            DesktopApp::Workspace(_) => ensure_desktop_workspace_arg(&mut args),
            DesktopApp::SingleSession(_) => {
                if let Some(session_id) = app.single_session_live_id() {
                    args.push(OsString::from("--resume"));
                    args.push(OsString::from(session_id));
                }
            }
        }
        Self { binary, args }
    }

    fn for_app_worker(&self) -> Self {
        let mut args = desktop_args_without_process_role(&self.args);
        args.push(OsString::from("--desktop-process-role"));
        args.push(OsString::from("app-worker"));
        Self {
            binary: self.binary.clone(),
            args,
        }
    }

    fn spawn_app_worker(&self) -> Result<DesktopWorkerConnection> {
        desktop_log::info(format_args!(
            "jcode-desktop: spawning app worker {} with args {:?}",
            self.binary.display(),
            self.args
        ));
        let mut command = Command::new(&self.binary);
        command.args(&self.args);
        command.env_remove(DESKTOP_RELOAD_WINDOW_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_READY_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV);
        DesktopWorkerConnection::spawn(&mut command)
            .with_context(|| format!("failed to spawn app worker {}", self.binary.display()))
    }
}

fn ensure_desktop_workspace_arg(args: &mut Vec<OsString>) {
    let has_mode_arg = args.iter().any(|arg| {
        arg == "--workspace"
            || arg == "--new"
            || arg == "--resume"
            || arg.to_str().is_some_and(|value| {
                value.starts_with("--resume=") || value.starts_with("jcode://")
            })
    });
    if !has_mode_arg {
        args.push(OsString::from("--workspace"));
    }
}

fn desktop_args_without_resume(args: &[OsString]) -> Vec<OsString> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--resume" {
            skip_next = true;
            continue;
        }
        if arg
            .to_str()
            .is_some_and(|value| value.starts_with("--resume="))
        {
            continue;
        }
        filtered.push(arg.clone());
    }
    filtered
}

fn desktop_args_without_process_role(args: &[OsString]) -> Vec<OsString> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--desktop-process-role" {
            skip_next = true;
            continue;
        }
        if arg == "--desktop-host" || arg == "--desktop-app-worker" {
            continue;
        }
        if arg
            .to_str()
            .is_some_and(|value| value.starts_with("--desktop-process-role="))
        {
            continue;
        }
        filtered.push(arg.clone());
    }
    filtered
}

fn desktop_key_event_from_winit(
    key: &Key,
    modifiers: ModifiersState,
    pressed: bool,
) -> DesktopKeyEvent {
    DesktopKeyEvent {
        key: desktop_key_name(key),
        text: desktop_key_text(key),
        pressed,
        modifiers: desktop_key_modifiers(modifiers),
    }
}

fn desktop_key_name(key: &Key) -> String {
    match key {
        Key::Character(value) => value.to_string(),
        Key::Named(named) => format!("{named:?}"),
        other => format!("{other:?}"),
    }
}

fn desktop_key_text(key: &Key) -> Option<String> {
    match key {
        Key::Character(value) => Some(value.to_string()),
        _ => None,
    }
}

fn desktop_key_modifiers(modifiers: ModifiersState) -> DesktopKeyModifiers {
    DesktopKeyModifiers {
        shift: modifiers.shift_key(),
        ctrl: modifiers.control_key(),
        alt: modifiers.alt_key(),
        super_key: modifiers.super_key(),
    }
}

fn desktop_mouse_wheel_event(delta: MouseScrollDelta) -> DesktopMouseEvent {
    let (delta_x, delta_y) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(position) => (position.x as f32, position.y as f32),
    };
    DesktopMouseEvent::Wheel { delta_x, delta_y }
}

fn forward_app_worker_input(hot_reloader: &mut DesktopHotReloader, input: DesktopInputEvent) {
    if let Err(error) = hot_reloader.send_app_worker_input(input) {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to forward input to app worker: {error:#}"
        ));
    }
}

fn forward_desktop_session_event_batch_to_worker(
    hot_reloader: &mut DesktopHotReloader,
    batch: &DesktopSessionEventBatch,
) {
    if !hot_reloader.has_app_worker() {
        return;
    }
    let wire = DesktopSessionEventBatchWire {
        events: batch
            .events
            .iter()
            .map(desktop_session_event_to_wire)
            .collect(),
        raw_event_count: batch.raw_event_count,
        raw_payload_bytes: batch.raw_payload_bytes,
    };
    if let Err(error) =
        hot_reloader.send_app_worker_message(DesktopHostToWorkerMessage::SessionEvents(wire))
    {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to forward session events to app worker: {error:#}"
        ));
    }
}

fn desktop_session_event_to_wire(
    event: &session_launch::DesktopSessionEvent,
) -> DesktopSessionEventWire {
    match event {
        session_launch::DesktopSessionEvent::Status(status) => DesktopSessionEventWire::Status {
            message: status.label(),
        },
        session_launch::DesktopSessionEvent::TextDelta(text)
        | session_launch::DesktopSessionEvent::TextReplace(text) => {
            DesktopSessionEventWire::AssistantTextDelta { text: text.clone() }
        }
        session_launch::DesktopSessionEvent::ToolStarted { id, name }
        | session_launch::DesktopSessionEvent::ToolExecuting { id, name } => {
            DesktopSessionEventWire::ToolStarted {
                id: id.clone().unwrap_or_default(),
                title: name.clone(),
            }
        }
        session_launch::DesktopSessionEvent::ToolFinished {
            id, name, is_error, ..
        } => DesktopSessionEventWire::ToolFinished {
            id: id.clone().unwrap_or_default(),
            title: name.clone(),
            success: !*is_error,
        },
        session_launch::DesktopSessionEvent::Error(message) => DesktopSessionEventWire::Error {
            message: message.clone(),
        },
        other => DesktopSessionEventWire::RawJson {
            event_type: desktop_session_event_type_name(other).to_string(),
            payload: format!("{other:?}"),
        },
    }
}

fn desktop_session_event_type_name(event: &session_launch::DesktopSessionEvent) -> &'static str {
    match event {
        session_launch::DesktopSessionEvent::Status(_) => "status",
        session_launch::DesktopSessionEvent::SessionStarted { .. } => "session_started",
        session_launch::DesktopSessionEvent::SessionRenamed { .. } => "session_renamed",
        session_launch::DesktopSessionEvent::TextDelta(_) => "text_delta",
        session_launch::DesktopSessionEvent::TextReplace(_) => "text_replace",
        session_launch::DesktopSessionEvent::ToolStarted { .. } => "tool_started",
        session_launch::DesktopSessionEvent::ToolExecuting { .. } => "tool_executing",
        session_launch::DesktopSessionEvent::ToolInput { .. } => "tool_input",
        session_launch::DesktopSessionEvent::ToolFinished { .. } => "tool_finished",
        session_launch::DesktopSessionEvent::ModelChanged { .. } => "model_changed",
        session_launch::DesktopSessionEvent::ModelCatalog { .. } => "model_catalog",
        session_launch::DesktopSessionEvent::ModelCatalogError { .. } => "model_catalog_error",
        session_launch::DesktopSessionEvent::StdinRequest { .. } => "stdin_request",
        session_launch::DesktopSessionEvent::ReloadProgress { .. } => "reload_progress",
        session_launch::DesktopSessionEvent::RuntimeMetadata { .. } => "runtime_metadata",
        session_launch::DesktopSessionEvent::TokenUsage { .. } => "token_usage",
        session_launch::DesktopSessionEvent::SystemNotice { .. } => "system_notice",
        session_launch::DesktopSessionEvent::SessionCloseRequested { .. } => {
            "session_close_requested"
        }
        session_launch::DesktopSessionEvent::Reloading { .. } => "reloading",
        session_launch::DesktopSessionEvent::Reloaded { .. } => "reloaded",
        session_launch::DesktopSessionEvent::Done => "done",
        session_launch::DesktopSessionEvent::Error(_) => "error",
    }
}

fn desktop_reload_binary_candidate(invoked_binary: &Path) -> PathBuf {
    let Some(repo_dir) = find_desktop_repo_dir() else {
        return invoked_binary.to_path_buf();
    };
    desktop_reload_binary_candidate_from(invoked_binary, &repo_dir)
}

fn desktop_reload_binary_candidate_from(invoked_binary: &Path, repo_dir: &Path) -> PathBuf {
    let selfdev = desktop_selfdev_binary_path(repo_dir);
    if paths_refer_to_same_file(invoked_binary, &selfdev)
        || binary_is_newer_than(&selfdev, invoked_binary)
    {
        selfdev
    } else {
        invoked_binary.to_path_buf()
    }
}

fn desktop_selfdev_binary_path(repo_dir: &Path) -> PathBuf {
    repo_dir
        .join("target")
        .join("selfdev")
        .join(desktop_binary_name())
}

fn desktop_binary_name() -> &'static str {
    if cfg!(windows) {
        "jcode-desktop.exe"
    } else {
        "jcode-desktop"
    }
}

fn binary_is_newer_than(candidate: &Path, baseline: &Path) -> bool {
    let Some(candidate_modified) = binary_modified_time(candidate) else {
        return false;
    };
    match binary_modified_time(baseline) {
        Some(baseline_modified) => candidate_modified > baseline_modified,
        None => true,
    }
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn find_desktop_repo_dir() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    find_desktop_repo_in_ancestors(&manifest_dir)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| find_desktop_repo_in_ancestors(&path))
        })
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| find_desktop_repo_in_ancestors(&path))
        })
}

fn find_desktop_repo_in_ancestors(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| is_jcode_desktop_repo(candidate))
        .map(Path::to_path_buf)
}

fn is_jcode_desktop_repo(candidate: &Path) -> bool {
    if !candidate.join("crates/jcode-desktop/Cargo.toml").is_file() {
        return false;
    }
    std::fs::read_to_string(candidate.join("Cargo.toml"))
        .map(|cargo_toml| cargo_toml.contains("name = \"jcode\""))
        .unwrap_or(false)
}

fn binary_modified_time(path: &Path) -> Option<std::time::SystemTime> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return None,
    };
    metadata.modified().ok()
}

fn resolve_invoked_binary(argv0: &OsString) -> Option<PathBuf> {
    let path = PathBuf::from(argv0);
    if path.components().count() > 1 {
        return Some(path);
    }

    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(&path))
        .find(|candidate| candidate.is_file())
}

#[allow(clippy::large_enum_variant)]
enum DesktopApp {
    SingleSession(SingleSessionApp),
    Workspace(Workspace),
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct DesktopAppDebugSnapshot {
    mode: &'static str,
    title: String,
    live_session_id: Option<String>,
    status: Option<String>,
    is_processing: bool,
    body_text: String,
}

impl DesktopApp {
    fn mode(&self) -> &'static str {
        match self {
            Self::SingleSession(_) => "single_session",
            Self::Workspace(_) => "workspace",
        }
    }

    fn is_single_session(&self) -> bool {
        matches!(self, Self::SingleSession(_))
    }

    fn is_workspace(&self) -> bool {
        matches!(self, Self::Workspace(_))
    }

    fn has_background_work(&self) -> bool {
        matches!(self, Self::SingleSession(app) if app.has_background_work())
    }

    fn has_frame_animation(&self) -> bool {
        matches!(self, Self::SingleSession(app) if app.has_frame_animation())
    }

    fn status_title(&self) -> String {
        match self {
            Self::SingleSession(app) => app.status_title(),
            Self::Workspace(workspace) => workspace.status_title(),
        }
    }

    fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        match self {
            Self::SingleSession(app) => app.handle_key(key),
            Self::Workspace(workspace) => workspace.handle_key(key),
        }
    }

    fn promote_focused_workspace_session(&mut self) -> bool {
        let Self::Workspace(workspace) = self else {
            return false;
        };
        let Some(card) = workspace.focused_session_card() else {
            return false;
        };
        let session_id = card.session_id.clone();
        let mut single_session = SingleSessionApp::new(Some(card));
        single_session.initialize_resumed_session(&session_id);
        single_session.hydrate_resumed_session_from_disk(&session_id);
        *self = Self::SingleSession(single_session);
        true
    }

    fn apply_session_event(&mut self, event: session_launch::DesktopSessionEvent) {
        if let Self::SingleSession(app) = self {
            app.apply_session_event(event);
        }
    }

    fn set_single_session_status_label(&mut self, label: impl Into<String>) {
        if let Self::SingleSession(app) = self {
            app.set_status_label(label);
        }
    }

    fn take_github_issue_sync_request(&mut self) -> bool {
        match self {
            Self::SingleSession(app) => app.take_github_issue_sync_request(),
            Self::Workspace(_) => false,
        }
    }

    fn note_github_issue_sync_already_running(&mut self) {
        if let Self::SingleSession(app) = self {
            app.note_github_issue_sync_already_running();
        }
    }

    fn apply_github_issue_sync_result(
        &mut self,
        result: std::result::Result<desktop_issue_cache::GitHubIssueSyncSummary, String>,
    ) {
        if let Self::SingleSession(app) = self {
            app.apply_github_issue_sync_result(result);
        }
    }

    fn preview_single_session_reasoning_effort_cycle(
        &mut self,
        direction: i8,
    ) -> ReasoningEffortCycleOutcome {
        match self {
            Self::SingleSession(app) => app.preview_reasoning_effort_cycle(direction),
            Self::Workspace(_) => ReasoningEffortCycleOutcome::Unavailable,
        }
    }

    fn preview_single_session_reasoning_effort_set(&mut self, effort: &str) -> Option<String> {
        match self {
            Self::SingleSession(app) => app.preview_reasoning_effort_set(effort),
            Self::Workspace(_) => None,
        }
    }

    fn set_reasoning_effort_via_active_session(&mut self, effort: String) -> anyhow::Result<()> {
        match self {
            Self::SingleSession(app) => app.set_reasoning_effort_via_active_session(effort),
            Self::Workspace(_) => {
                anyhow::bail!("reasoning effort changes require single-session mode")
            }
        }
    }

    fn set_single_session_handle(&mut self, handle: session_launch::DesktopSessionHandle) {
        if let Self::SingleSession(app) = self {
            app.set_session_handle(handle);
        }
    }

    fn apply_single_session_switcher_cards(&mut self, cards: Vec<workspace::SessionCard>) {
        if let Self::SingleSession(app) = self {
            app.apply_session_switcher_cards(cards);
        }
    }

    fn cancel_single_session_generation(&mut self) {
        if let Self::SingleSession(app) = self {
            app.cancel_generation();
        }
    }

    fn attach_clipboard_image(&mut self, media_type: String, base64_data: String) {
        match self {
            Self::SingleSession(app) => app.attach_image(media_type, base64_data),
            Self::Workspace(workspace) => {
                workspace.attach_image(media_type, base64_data);
            }
        }
    }

    fn accepts_clipboard_image_paste(&self) -> bool {
        match self {
            Self::SingleSession(app) => app.accepts_clipboard_image_paste(),
            Self::Workspace(workspace) => workspace.mode == InputMode::Insert,
        }
    }

    fn paste_text(&mut self, text: &str) {
        match self {
            Self::SingleSession(app) => app.paste_text(text),
            Self::Workspace(workspace) => {
                workspace.paste_text(text);
            }
        }
    }

    fn send_single_session_stdin_response(
        &mut self,
        request_id: String,
        input: String,
    ) -> anyhow::Result<()> {
        match self {
            Self::SingleSession(app) => app.send_stdin_response(request_id, input),
            Self::Workspace(_) => {
                anyhow::bail!("stdin responses are only supported in single-session mode")
            }
        }
    }

    fn take_next_queued_single_session_draft(&mut self) -> Option<(String, Vec<(String, String)>)> {
        match self {
            Self::SingleSession(app) => app.take_next_queued_draft(),
            Self::Workspace(_) => None,
        }
    }

    fn begin_single_session_selection_at(
        &mut self,
        x: f32,
        y: f32,
        size: PhysicalSize<u32>,
    ) -> bool {
        if let Self::SingleSession(app) = self {
            let lines = single_session_visible_body(app, size);
            if let Some(point) = single_session_body_point_at_position(size, x, y, &lines) {
                app.begin_selection(point);
                return true;
            }
        }
        false
    }

    fn update_single_session_selection_at(
        &mut self,
        x: f32,
        y: f32,
        size: PhysicalSize<u32>,
    ) -> bool {
        if let Self::SingleSession(app) = self {
            let lines = single_session_visible_body(app, size);
            if let Some(point) = single_session_body_point_at_position(size, x, y, &lines) {
                app.update_selection(point);
                return true;
            }
        }
        false
    }

    fn begin_single_session_draft_selection_at(
        &mut self,
        x: f32,
        y: f32,
        size: PhysicalSize<u32>,
    ) -> bool {
        if let Self::SingleSession(app) = self
            && let Some((line, column)) = single_session_draft_line_col_at_position(app, size, x, y)
        {
            app.begin_draft_selection(SelectionPoint { line, column });
            return true;
        }
        false
    }

    fn update_single_session_draft_selection_at(
        &mut self,
        x: f32,
        y: f32,
        size: PhysicalSize<u32>,
    ) -> bool {
        if let Self::SingleSession(app) = self
            && let Some((line, column)) = single_session_draft_line_col_at_position(app, size, x, y)
        {
            app.update_draft_selection(SelectionPoint { line, column });
            return true;
        }
        false
    }

    fn selected_single_session_draft_text(&mut self) -> Option<String> {
        if let Self::SingleSession(app) = self {
            return app.selected_draft_text();
        }
        None
    }

    fn selected_single_session_text(&mut self, size: PhysicalSize<u32>) -> Option<String> {
        if let Self::SingleSession(app) = self {
            let lines = single_session_visible_body(app, size);
            let selected = app.selected_text_from_lines(&lines);
            app.clear_selection();
            return selected;
        }
        None
    }

    fn scroll_single_session_body(
        &mut self,
        lines: impl Into<f64>,
        size: PhysicalSize<u32>,
        metrics_cache: &mut SingleSessionScrollMetricsCache,
    ) -> bool {
        if let Self::SingleSession(app) = self {
            let previous_scroll_lines = app.body_scroll_lines;
            app.scroll_body_lines(lines);
            if let Some(metrics) = metrics_cache.metrics(app, size) {
                app.body_scroll_lines = app.body_scroll_lines.min(metrics.max_scroll_lines as f32);
            } else {
                app.body_scroll_lines = 0.0;
            }
            return (app.body_scroll_lines - previous_scroll_lines).abs()
                >= SCROLL_FRACTIONAL_EPSILON;
        }
        false
    }

    fn single_session_smooth_scroll_lines(
        &self,
        pending_lines: f32,
        size: PhysicalSize<u32>,
        metrics_cache: &mut SingleSessionScrollMetricsCache,
    ) -> f32 {
        let Self::SingleSession(app) = self else {
            return 0.0;
        };
        let Some(metrics) = metrics_cache.metrics(app, size) else {
            return 0.0;
        };
        let base_scroll = app.body_scroll_lines.min(metrics.max_scroll_lines as f32);
        (base_scroll + pending_lines).clamp(0.0, metrics.max_scroll_lines as f32) - base_scroll
    }

    fn single_session_live_id(&self) -> Option<String> {
        match self {
            Self::SingleSession(app) => app.live_session_id.clone(),
            Self::Workspace(_) => None,
        }
    }

    #[cfg(test)]
    fn debug_snapshot(&self) -> DesktopAppDebugSnapshot {
        match self {
            Self::SingleSession(app) => DesktopAppDebugSnapshot {
                mode: "single_session",
                title: app.title(),
                live_session_id: app.live_session_id.clone(),
                status: app.status.clone(),
                is_processing: app.is_processing,
                body_text: app.body_lines().join("\n"),
            },
            Self::Workspace(workspace) => DesktopAppDebugSnapshot {
                mode: "workspace",
                title: workspace.status_title(),
                live_session_id: None,
                status: None,
                is_processing: false,
                body_text: workspace.status_title(),
            },
        }
    }
}

fn show_desktop_reload_notice(app: &mut DesktopApp) {
    app.set_single_session_status_label("desktop UI reloaded");
}

fn to_key_input(key: &Key, modifiers: ModifiersState) -> KeyInput {
    match key {
        Key::Named(NamedKey::Escape) => KeyInput::Escape,
        Key::Named(NamedKey::Space) => KeyInput::Character(" ".to_string()),
        Key::Named(NamedKey::Copy) => KeyInput::CopyLatestResponse,
        Key::Named(NamedKey::Cut) => KeyInput::CutInputLine,
        Key::Named(NamedKey::Paste) => KeyInput::PasteText,
        Key::Named(NamedKey::Undo) => KeyInput::UndoInput,
        Key::Named(NamedKey::Enter) if modifiers.control_key() => KeyInput::QueueDraft,
        Key::Named(NamedKey::Enter) if modifiers.shift_key() || modifiers.alt_key() => {
            KeyInput::Enter
        }
        Key::Named(NamedKey::Enter) => KeyInput::SubmitDraft,
        Key::Named(NamedKey::Tab) if modifiers.control_key() && modifiers.shift_key() => {
            KeyInput::CycleModel(-1)
        }
        Key::Named(NamedKey::Tab) if modifiers.control_key() => KeyInput::CycleModel(1),
        Key::Named(NamedKey::Tab) => KeyInput::Autocomplete,
        Key::Named(NamedKey::Backspace) if modifiers.control_key() || modifiers.alt_key() => {
            KeyInput::DeletePreviousWord
        }
        Key::Named(NamedKey::Backspace) => KeyInput::Backspace,
        Key::Named(NamedKey::Delete) => KeyInput::DeleteNextChar,
        Key::Named(NamedKey::PageUp) => KeyInput::ScrollBodyPages(1),
        Key::Named(NamedKey::PageDown) => KeyInput::ScrollBodyPages(-1),
        Key::Named(NamedKey::ArrowUp) if modifiers.control_key() => KeyInput::RetrieveQueuedDraft,
        Key::Named(NamedKey::ArrowUp) if modifiers.alt_key() => KeyInput::JumpPrompt(-1),
        Key::Named(NamedKey::ArrowDown) if modifiers.alt_key() => KeyInput::JumpPrompt(1),
        Key::Named(NamedKey::ArrowUp) => KeyInput::ModelPickerMove(-1),
        Key::Named(NamedKey::ArrowDown) => KeyInput::ModelPickerMove(1),
        Key::Named(NamedKey::ArrowLeft) if modifiers.alt_key() => {
            KeyInput::CycleReasoningEffort(-1)
        }
        Key::Named(NamedKey::ArrowRight) if modifiers.alt_key() => {
            KeyInput::CycleReasoningEffort(1)
        }
        Key::Named(NamedKey::ArrowLeft) if modifiers.control_key() => KeyInput::MoveCursorWordLeft,
        Key::Named(NamedKey::ArrowRight) if modifiers.control_key() => {
            KeyInput::MoveCursorWordRight
        }
        Key::Named(NamedKey::ArrowLeft) => KeyInput::MoveCursorLeft,
        Key::Named(NamedKey::ArrowRight) => KeyInput::MoveCursorRight,
        Key::Named(NamedKey::Home) if modifiers.control_key() => KeyInput::ScrollBodyToTop,
        Key::Named(NamedKey::End) if modifiers.control_key() => KeyInput::ScrollBodyToBottom,
        Key::Named(NamedKey::Home) => KeyInput::MoveToLineStart,
        Key::Named(NamedKey::End) => KeyInput::MoveToLineEnd,
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("a") => {
            KeyInput::MoveToLineStart
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("e") => {
            KeyInput::MoveToLineEnd
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("b") => {
            KeyInput::MoveCursorWordLeft
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("f") => {
            KeyInput::MoveCursorWordRight
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("u") => {
            KeyInput::DeleteToLineStart
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers)
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("k") =>
        {
            KeyInput::CopyLatestCodeBlock
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("k") => {
            KeyInput::DeleteToLineEnd
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("w") => {
            KeyInput::DeletePreviousWord
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers) && text.eq_ignore_ascii_case("x") =>
        {
            KeyInput::CutInputLine
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers) && text.eq_ignore_ascii_case("z") =>
        {
            KeyInput::UndoInput
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers)
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("c") =>
        {
            KeyInput::CopyLatestResponse
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers)
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("t") =>
        {
            KeyInput::CopyTranscript
        }
        Key::Character(text)
            if modifiers.control_key()
                && (text.eq_ignore_ascii_case("c") || text.eq_ignore_ascii_case("d")) =>
        {
            KeyInput::CancelGeneration
        }
        Key::Character(text) if modifiers.super_key() && text.eq_ignore_ascii_case("c") => {
            KeyInput::CopyLatestResponse
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("b") => {
            KeyInput::MoveCursorWordLeft
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("f") => {
            KeyInput::MoveCursorWordRight
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("d") => {
            KeyInput::DeleteNextWord
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("v") => {
            KeyInput::PasteText
        }
        Key::Character(text) if modifiers.control_key() && text == "[" => KeyInput::JumpPrompt(-1),
        Key::Character(text) if modifiers.control_key() && text == "]" => KeyInput::JumpPrompt(1),
        Key::Character(text) if modifiers.super_key() && text.eq_ignore_ascii_case("k") => {
            KeyInput::ScrollBodyLines(1)
        }
        Key::Character(text) if modifiers.super_key() && text.eq_ignore_ascii_case("j") => {
            KeyInput::ScrollBodyLines(-1)
        }
        Key::Character(text)
            if (modifiers.control_key() || modifiers.super_key())
                && text.eq_ignore_ascii_case("q") =>
        {
            KeyInput::ExitApp
        }
        Key::Character(text) if modifiers.control_key() && text == ";" => KeyInput::SpawnPanel,
        Key::Character(text) if modifiers.control_key() && (text == "?" || text == "/") => {
            KeyInput::HotkeyHelp
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("s") =>
        {
            KeyInput::ToggleSessionInfo
        }
        Key::Character(text)
            if modifiers.control_key()
                && (text.eq_ignore_ascii_case("p") || text.eq_ignore_ascii_case("o")) =>
        {
            KeyInput::OpenSessionSwitcher
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("r") => {
            KeyInput::RefreshSessions
        }
        Key::Character(text) if modifiers.control_key() && (text == "-" || text == "_") => {
            KeyInput::AdjustTextScale(-1)
        }
        Key::Character(text) if modifiers.control_key() && (text == "=" || text == "+") => {
            KeyInput::AdjustTextScale(1)
        }
        Key::Character(text) if modifiers.control_key() && text == "0" => KeyInput::ResetTextScale,
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers) && text.eq_ignore_ascii_case("v") =>
        {
            KeyInput::PasteText
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("i") =>
        {
            KeyInput::ClearAttachedImages
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("i") => {
            KeyInput::AttachClipboardImage
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("m") =>
        {
            KeyInput::OpenModelPicker
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("m") => {
            KeyInput::CycleModel(1)
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("n") => {
            KeyInput::CycleModel(-1)
        }
        Key::Character(text) if modifiers.control_key() && text == "1" => {
            KeyInput::SetPanelSize(PanelSizePreset::Quarter)
        }
        Key::Character(text) if modifiers.control_key() && text == "2" => {
            KeyInput::SetPanelSize(PanelSizePreset::Half)
        }
        Key::Character(text) if modifiers.control_key() && text == "3" => {
            KeyInput::SetPanelSize(PanelSizePreset::ThreeQuarter)
        }
        Key::Character(text) if modifiers.control_key() && text == "4" => {
            KeyInput::SetPanelSize(PanelSizePreset::Full)
        }
        Key::Character(_)
            if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() =>
        {
            KeyInput::Other
        }
        Key::Character(text) => KeyInput::Character(text.to_string()),
        _ => KeyInput::Other,
    }
}

fn desktop_clipboard_shortcut_modifier(modifiers: ModifiersState) -> bool {
    modifiers.control_key() || modifiers.alt_key() || modifiers.super_key()
}

fn is_space_key(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Space)) || matches!(key, Key::Character(text) if text == " ")
}

fn workspace_space_hold_progress(
    app: &DesktopApp,
    started_at: Option<Instant>,
    consumed: bool,
) -> Option<f32> {
    let DesktopApp::Workspace(workspace) = app else {
        return None;
    };
    let started_at = started_at?;
    if consumed {
        return None;
    }
    let threshold = workspace.space_hold_toggle_duration();
    if threshold.is_zero() {
        return Some(1.0);
    }
    Some(
        (Instant::now()
            .saturating_duration_since(started_at)
            .as_secs_f32()
            / threshold.as_secs_f32())
        .clamp(0.0, 1.0),
    )
}

fn apply_desktop_session_event_batch(
    app: &mut DesktopApp,
    events: Vec<session_launch::DesktopSessionEvent>,
) -> bool {
    apply_desktop_session_event_batch_with_stats(app, events).visible_changed
}

#[derive(Debug, Clone)]
struct DesktopSessionApplyStats {
    visible_changed: bool,
    event_count: usize,
    text_delta_bytes: usize,
    session_card_refresh_requested: bool,
    elapsed: Duration,
}

fn apply_desktop_session_event_batch_with_stats(
    app: &mut DesktopApp,
    events: Vec<session_launch::DesktopSessionEvent>,
) -> DesktopSessionApplyStats {
    if events.is_empty() {
        return DesktopSessionApplyStats {
            visible_changed: false,
            event_count: 0,
            text_delta_bytes: 0,
            session_card_refresh_requested: false,
            elapsed: Duration::ZERO,
        };
    }
    let started = Instant::now();
    let event_count = events.len();
    let mut text_delta_bytes = 0usize;
    let mut visible_changed = false;
    let mut session_card_refresh_requested = false;
    for event in events {
        log_desktop_session_event_error(&event);
        if let session_launch::DesktopSessionEvent::TextDelta(text) = &event {
            text_delta_bytes += text.len();
        }
        session_card_refresh_requested |= desktop_session_event_refreshes_session_card(&event);
        visible_changed |= desktop_session_event_affects_visible_state(&event);
        app.apply_session_event(event);
    }
    let elapsed = started.elapsed();
    log_desktop_slow_interaction(
        "session_event_apply",
        elapsed,
        serde_json::json!({
            "events": event_count,
            "text_delta_bytes": text_delta_bytes,
        }),
    );
    DesktopSessionApplyStats {
        visible_changed,
        event_count,
        text_delta_bytes,
        session_card_refresh_requested,
        elapsed,
    }
}

fn log_desktop_session_event_error(event: &session_launch::DesktopSessionEvent) {
    match event {
        session_launch::DesktopSessionEvent::Error(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: session error event: {}",
                desktop_log::truncate_for_log(error, 2048)
            ));
        }
        session_launch::DesktopSessionEvent::ModelCatalogError { error } => {
            desktop_log::error(format_args!(
                "jcode-desktop: model catalog error event: {}",
                desktop_log::truncate_for_log(error, 2048)
            ));
        }
        session_launch::DesktopSessionEvent::ModelChanged {
            model,
            provider_name,
            error: Some(error),
        } => {
            desktop_log::error(format_args!(
                "jcode-desktop: model switch failed model={} provider={} error={}",
                desktop_log::truncate_for_log(model, 256),
                provider_name
                    .as_deref()
                    .map(|provider| desktop_log::truncate_for_log(provider, 256))
                    .unwrap_or_else(|| "<unknown>".to_string()),
                desktop_log::truncate_for_log(error, 2048)
            ));
        }
        session_launch::DesktopSessionEvent::ToolFinished {
            id: _,
            name,
            summary,
            is_error: true,
        } => {
            desktop_log::warn(format_args!(
                "jcode-desktop: tool failed name={} summary={}",
                desktop_log::truncate_for_log(name, 256),
                desktop_log::truncate_for_log(summary, 2048)
            ));
        }
        _ => {}
    }
}

fn desktop_session_event_refreshes_session_card(
    event: &session_launch::DesktopSessionEvent,
) -> bool {
    matches!(
        event,
        session_launch::DesktopSessionEvent::SessionStarted { .. }
            | session_launch::DesktopSessionEvent::SessionRenamed { .. }
            | session_launch::DesktopSessionEvent::Reloaded { .. }
            | session_launch::DesktopSessionEvent::Done
            | session_launch::DesktopSessionEvent::Error(_)
    )
}

#[allow(clippy::too_many_arguments)]
fn log_desktop_session_event_batch_profile(
    raw_event_count: usize,
    raw_payload_bytes: usize,
    accumulated_for: Duration,
    ui_queue_delay: Duration,
    apply_stats: &DesktopSessionApplyStats,
    redraw_requested: bool,
    redraw_deferred: bool,
    session_card_refresh_spawned: bool,
) {
    if raw_event_count < 128
        && raw_payload_bytes < 8 * 1024
        && accumulated_for < Duration::from_millis(40)
        && ui_queue_delay < DESKTOP_INPUT_LATENCY_BUDGET
        && apply_stats.elapsed < DESKTOP_120FPS_FRAME_BUDGET
        && !apply_stats.session_card_refresh_requested
        && !session_card_refresh_spawned
    {
        return;
    }
    emit_desktop_profile_event(
        "jcode-desktop-session-event-profile",
        serde_json::json!({
            "raw_events": raw_event_count,
            "coalesced_events": apply_stats.event_count,
            "raw_payload_bytes": raw_payload_bytes,
            "text_delta_bytes": apply_stats.text_delta_bytes,
            "forwarder_accumulated_ms": accumulated_for.as_secs_f64() * 1000.0,
            "ui_queue_delay_ms": ui_queue_delay.as_secs_f64() * 1000.0,
            "apply_ms": apply_stats.elapsed.as_secs_f64() * 1000.0,
            "visible_changed": apply_stats.visible_changed,
            "redraw_requested": redraw_requested,
            "redraw_deferred": redraw_deferred,
            "session_card_refresh_requested": apply_stats.session_card_refresh_requested,
            "session_card_refresh_spawned": session_card_refresh_spawned,
        }),
    );
}

fn log_desktop_session_card_refresh_profile(
    session_id: &str,
    loaded_in: Duration,
    card_found: bool,
    applied: bool,
) {
    if loaded_in < Duration::from_millis(40) && card_found {
        return;
    }
    emit_desktop_profile_event(
        "jcode-desktop-session-card-refresh-profile",
        serde_json::json!({
            "session_id": session_id,
            "loaded_in_ms": duration_ms(loaded_in),
            "card_found": card_found,
            "applied": applied,
            "ui_thread_blocking": false,
        }),
    );
}

fn log_desktop_session_cards_load_profile(
    purpose: DesktopSessionCardsPurpose,
    loaded_in: Duration,
    card_count: usize,
    applied: bool,
) {
    if loaded_in < Duration::from_millis(40) && applied {
        return;
    }
    emit_desktop_profile_event(
        "jcode-desktop-session-cards-load-profile",
        serde_json::json!({
            "purpose": format!("{purpose:?}"),
            "loaded_in_ms": duration_ms(loaded_in),
            "card_count": card_count,
            "applied": applied,
            "ui_thread_blocking": false,
        }),
    );
}

fn log_desktop_preferences_save_profile(
    saved_in: Duration,
    queued_for: Duration,
    coalesced_saves: usize,
    error: Option<&str>,
) {
    if saved_in < Duration::from_millis(40) && coalesced_saves <= 1 && error.is_none() {
        return;
    }
    emit_desktop_profile_event(
        "jcode-desktop-preferences-save-profile",
        serde_json::json!({
            "saved_in_ms": duration_ms(saved_in),
            "queued_for_ms": duration_ms(queued_for),
            "coalesced_saves": coalesced_saves,
            "error": error,
            "ui_thread_blocking": false,
        }),
    );
}

fn log_desktop_crashed_sessions_restore_profile(restored: usize, errors: usize, elapsed: Duration) {
    if elapsed < Duration::from_millis(40) && errors == 0 {
        return;
    }
    emit_desktop_profile_event(
        "jcode-desktop-crashed-sessions-restore-profile",
        serde_json::json!({
            "restored": restored,
            "errors": errors,
            "elapsed_ms": duration_ms(elapsed),
            "ui_thread_blocking": false,
        }),
    );
}

#[derive(Debug, Clone)]
struct DesktopStreamEndToEndBenchmark {
    raw_events: usize,
    batches: usize,
    coalesced_events: usize,
    paints: usize,
    max_batch_raw_events: usize,
    max_batch_payload_bytes: usize,
    total_wall: Duration,
    max_forwarder_accumulated: Duration,
    max_apply: Duration,
    max_no_paint_gap: Duration,
    max_batch_to_paint: Duration,
    stream_left_queued_after_first_batch: bool,
}

impl DesktopStreamEndToEndBenchmark {
    fn passes_no_paint_budget(&self) -> bool {
        self.max_no_paint_gap <= DESKTOP_NO_PAINT_BUDGET
    }

    fn passes_interaction_budget(&self) -> bool {
        self.max_apply <= DESKTOP_120FPS_FRAME_BUDGET
            && self.max_forwarder_accumulated <= DESKTOP_NO_PAINT_BUDGET
            && self.max_batch_to_paint <= DESKTOP_NO_PAINT_BUDGET
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "raw_events": self.raw_events,
            "batches": self.batches,
            "coalesced_events": self.coalesced_events,
            "paints": self.paints,
            "max_batch_raw_events": self.max_batch_raw_events,
            "max_batch_payload_bytes": self.max_batch_payload_bytes,
            "total_wall_ms": duration_ms(self.total_wall),
            "max_forwarder_accumulated_ms": duration_ms(self.max_forwarder_accumulated),
            "max_apply_ms": duration_ms(self.max_apply),
            "max_no_paint_gap_ms": duration_ms(self.max_no_paint_gap),
            "max_batch_to_paint_ms": duration_ms(self.max_batch_to_paint),
            "stream_left_queued_after_first_batch": self.stream_left_queued_after_first_batch,
            "passes_no_paint_budget": self.passes_no_paint_budget(),
            "passes_interaction_budget": self.passes_interaction_budget(),
        })
    }
}

fn run_desktop_stream_end_to_end_benchmark(raw_events: usize) -> DesktopStreamEndToEndBenchmark {
    let raw_events = raw_events.max(1);
    let (tx, rx) = mpsc::channel();
    for index in 0..raw_events {
        tx.send(session_launch::DesktopSessionEvent::TextDelta(format!(
            "{} ",
            index + 1
        )))
        .unwrap();
    }
    drop(tx);

    let started = Instant::now();
    let mut next_forward_at = started;
    let mut app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let mut batches = 0usize;
    let mut coalesced_events = 0usize;
    let mut paints = 0usize;
    let mut max_batch_raw_events = 0usize;
    let mut max_batch_payload_bytes = 0usize;
    let mut max_forwarder_accumulated = Duration::ZERO;
    let mut max_apply = Duration::ZERO;
    let mut max_no_paint_gap = Duration::ZERO;
    let mut max_batch_to_paint = Duration::ZERO;
    let mut last_paint_at = started;
    let mut pending_batch_since: Option<Instant> = None;
    let mut stream_left_queued_after_first_batch = false;

    while let Ok(first_event) = rx.try_recv() {
        let now = Instant::now();
        if now < next_forward_at {
            std::thread::sleep(next_forward_at.saturating_duration_since(now));
        }

        let batch = collect_desktop_session_event_batch(first_event, &rx);
        if batches == 0 {
            stream_left_queued_after_first_batch = batch.raw_event_count < raw_events;
        }
        let forwarded_at = Instant::now();
        next_forward_at = forwarded_at + BACKEND_EVENT_FORWARD_INTERVAL;

        batches += 1;
        coalesced_events += batch.events.len();
        max_batch_raw_events = max_batch_raw_events.max(batch.raw_event_count);
        max_batch_payload_bytes = max_batch_payload_bytes.max(batch.raw_payload_bytes);
        max_forwarder_accumulated = max_forwarder_accumulated.max(batch.accumulated_for());
        pending_batch_since.get_or_insert(batch.first_received_at);

        let apply_stats = apply_desktop_session_event_batch_with_stats(&mut app, batch.events);
        max_apply = max_apply.max(apply_stats.elapsed);
        if apply_stats.visible_changed {
            let paint_now = Instant::now();
            if paint_now.saturating_duration_since(last_paint_at) >= BACKEND_REDRAW_FRAME_INTERVAL {
                paints += 1;
                max_no_paint_gap =
                    max_no_paint_gap.max(paint_now.saturating_duration_since(last_paint_at));
                if let Some(pending_since) = pending_batch_since.take() {
                    max_batch_to_paint =
                        max_batch_to_paint.max(paint_now.saturating_duration_since(pending_since));
                }
                last_paint_at = paint_now;
            }
        }
    }

    if let Some(pending_since) = pending_batch_since.take() {
        let paint_now = Instant::now();
        paints += 1;
        max_no_paint_gap = max_no_paint_gap.max(paint_now.saturating_duration_since(last_paint_at));
        max_batch_to_paint =
            max_batch_to_paint.max(paint_now.saturating_duration_since(pending_since));
    }

    DesktopStreamEndToEndBenchmark {
        raw_events,
        batches,
        coalesced_events,
        paints,
        max_batch_raw_events,
        max_batch_payload_bytes,
        total_wall: started.elapsed(),
        max_forwarder_accumulated,
        max_apply,
        max_no_paint_gap,
        max_batch_to_paint,
        stream_left_queued_after_first_batch,
    }
}

fn desktop_session_event_affects_visible_state(
    event: &session_launch::DesktopSessionEvent,
) -> bool {
    !matches!(event, session_launch::DesktopSessionEvent::ToolInput { .. })
}

#[cfg(test)]
fn apply_pending_session_events(
    app: &mut DesktopApp,
    session_event_rx: &mpsc::Receiver<session_launch::DesktopSessionEvent>,
) -> bool {
    let mut events = Vec::new();
    while let Ok(event) = session_event_rx.try_recv() {
        events.push(event);
    }
    apply_desktop_session_event_batch(app, events)
}

impl DesktopAppDriver for DesktopApp {
    type KeyInput = KeyInput;
    type KeyOutcome = KeyOutcome;

    fn mode(&self) -> &'static str {
        DesktopApp::mode(self)
    }

    fn status_title(&self) -> String {
        DesktopApp::status_title(self)
    }

    fn live_session_id(&self) -> Option<String> {
        DesktopApp::single_session_live_id(self)
    }

    fn has_background_work(&self) -> bool {
        DesktopApp::has_background_work(self)
    }

    fn has_frame_animation(&self) -> bool {
        DesktopApp::has_frame_animation(self)
    }

    fn handle_key_input(&mut self, key: Self::KeyInput) -> Self::KeyOutcome {
        DesktopApp::handle_key(self, key)
    }

    fn apply_session_event(&mut self, event: session_launch::DesktopSessionEvent) {
        DesktopApp::apply_session_event(self, event);
    }

    fn build_scene(&self, context: DesktopSceneBuildContext) -> desktop_scene::DesktopScene {
        desktop_app_scene(self, context.scene)
    }

    fn snapshot(&self) -> DesktopUiSnapshot {
        DesktopUiSnapshot::new(
            DesktopApp::mode(self),
            DesktopApp::status_title(self),
            DesktopApp::single_session_live_id(self),
            desktop_surface_snapshot(self),
        )
    }

    fn restore_snapshot(
        &mut self,
        snapshot: DesktopUiSnapshot,
    ) -> Result<(), DesktopSnapshotRestoreError> {
        if snapshot.version != DESKTOP_UI_SNAPSHOT_VERSION {
            return Err(DesktopSnapshotRestoreError::UnsupportedVersion {
                version: snapshot.version,
            });
        }
        if snapshot.mode != DesktopApp::mode(self) {
            return Err(DesktopSnapshotRestoreError::UnsupportedMode {
                mode: snapshot.mode,
            });
        }
        Ok(())
    }
}

fn desktop_app_scene(app: &DesktopApp, mut scene: DesktopScene) -> DesktopScene {
    scene.metadata.title = Some(app.status_title());
    scene.metadata.animation_active = app.has_frame_animation();
    scene.metadata.content_ready = true;
    if scene.display_list.commands.is_empty() {
        scene.push(DesktopDisplayCommand::Clear(DesktopColor::from_array(
            BACKGROUND_TOP_LEFT,
        )));
    }
    scene
}

fn desktop_surface_snapshot(app: &DesktopApp) -> DesktopSurfaceSnapshot {
    match app {
        DesktopApp::SingleSession(single_session) => {
            DesktopSurfaceSnapshot::SingleSession(DesktopSingleSessionSnapshot {
                session_title: single_session
                    .session
                    .as_ref()
                    .map(|session| session.title.clone()),
                draft: single_session.draft.clone(),
                draft_cursor: single_session.draft_cursor,
                body_scroll_millis: (single_session.body_scroll_lines * 1000.0).round() as i32,
                detail_scroll: single_session.detail_scroll,
                show_help: single_session.show_help,
                show_session_info: single_session.show_session_info,
                pending_image_count: single_session.pending_images.len(),
                model_picker_open: single_session.model_picker.open,
                session_switcher_open: single_session.session_switcher.open,
                stdin_response_active: single_session.stdin_response.is_some(),
            })
        }
        DesktopApp::Workspace(workspace) => {
            let focused_session_id = workspace
                .surfaces
                .iter()
                .find(|surface| surface.id == workspace.focused_id)
                .and_then(|surface| surface.session_id.clone());
            DesktopSurfaceSnapshot::Workspace(DesktopWorkspaceSnapshot {
                input_mode: format!("{:?}", workspace.mode),
                focused_surface_id: workspace.focused_id,
                focused_session_id,
                zoomed: workspace.zoomed,
                detail_scroll: workspace.detail_scroll,
                draft: workspace.draft.clone(),
                draft_cursor: workspace.draft_cursor,
                pending_image_count: workspace.pending_images.len(),
                surfaces: workspace
                    .surfaces
                    .iter()
                    .map(|surface| DesktopWorkspaceSurfaceSnapshot {
                        id: surface.id,
                        kind: format!("{:?}", surface.kind),
                        title: surface.title.clone(),
                        session_id: surface.session_id.clone(),
                        lane: surface.lane,
                        column: surface.column,
                        color_index: surface.color_index,
                    })
                    .collect(),
            })
        }
    }
}

fn apply_single_session_error(app: &mut DesktopApp, error: anyhow::Error) {
    desktop_log::error(format_args!("jcode-desktop: UI action failed: {error:#}"));
    app.apply_session_event(session_launch::DesktopSessionEvent::Error(format!(
        "{error:#}"
    )));
}

#[derive(Default)]
struct DesktopClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl DesktopClipboard {
    fn clipboard(&mut self) -> Result<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().context("failed to access clipboard")?);
        }
        self.clipboard
            .as_mut()
            .context("failed to retain clipboard handle")
    }

    fn set_text(&mut self, text: &str) -> Result<()> {
        self.with_clipboard_retry("failed to set clipboard text", |clipboard| {
            clipboard.set_text(text.to_string())
        })
    }

    fn get_text(&mut self) -> Result<String> {
        self.with_clipboard_retry("clipboard does not contain text", |clipboard| {
            clipboard.get_text()
        })
    }

    fn get_image(&mut self) -> Result<arboard::ImageData<'static>> {
        self.with_clipboard_retry("clipboard does not contain an image", |clipboard| {
            clipboard.get_image()
        })
    }

    fn with_clipboard_retry<T>(
        &mut self,
        context: &'static str,
        mut operation: impl FnMut(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
    ) -> Result<T> {
        const CLIPBOARD_RETRY_ATTEMPTS: usize = 3;
        const CLIPBOARD_RETRY_DELAY: Duration = Duration::from_millis(20);

        for attempt in 0..CLIPBOARD_RETRY_ATTEMPTS {
            let result = operation(self.clipboard()?);
            match result {
                Ok(value) => return Ok(value),
                Err(error)
                    if matches!(&error, arboard::Error::ClipboardOccupied)
                        && attempt + 1 < CLIPBOARD_RETRY_ATTEMPTS =>
                {
                    std::thread::sleep(CLIPBOARD_RETRY_DELAY);
                }
                Err(error) => {
                    if !matches!(
                        &error,
                        arboard::Error::ContentNotAvailable | arboard::Error::ClipboardOccupied
                    ) {
                        self.clipboard = None;
                    }
                    return Err(error).context(context);
                }
            }
        }

        anyhow::bail!("clipboard remained occupied after retrying")
    }
}

fn copy_text_to_clipboard(
    clipboard: &mut DesktopClipboard,
    text: &str,
    success_notice: &'static str,
    app: &mut DesktopApp,
) {
    match clipboard.set_text(text) {
        Ok(()) => app.set_single_session_status_label(success_notice),
        Err(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to update clipboard after {success_notice}: {error:#}"
            ));
            app.apply_session_event(session_launch::DesktopSessionEvent::Error(format!(
                "failed to update clipboard after {success_notice}: {error:#}"
            )));
        }
    }
}

fn paste_clipboard_into_app(clipboard: &mut DesktopClipboard, app: &mut DesktopApp) -> Result<()> {
    match clipboard_text(clipboard) {
        Ok(text) => {
            if paste_clipboard_text(app, &text) || !app.accepts_clipboard_image_paste() {
                return Ok(());
            }
            paste_clipboard_image_into_app(clipboard, app)
                .with_context(|| "clipboard text was empty and no pasteable image was available")
        }
        Err(text_error) if app.accepts_clipboard_image_paste() => {
            paste_clipboard_image_into_app(clipboard, app)
                .with_context(|| format!("clipboard did not contain pasteable text: {text_error}"))
        }
        Err(error) => Err(error),
    }
}

fn paste_clipboard_text(app: &mut DesktopApp, text: &str) -> bool {
    let text = normalize_clipboard_text(text);
    if text.is_empty() {
        return false;
    }
    app.paste_text(&text);
    true
}

fn paste_clipboard_image_into_app(
    clipboard: &mut DesktopClipboard,
    app: &mut DesktopApp,
) -> Result<()> {
    let (media_type, base64_data) = clipboard_image_png_base64(clipboard)?;
    app.attach_clipboard_image(media_type, base64_data);
    Ok(())
}

fn normalize_clipboard_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn clipboard_image_png_base64(clipboard: &mut DesktopClipboard) -> Result<(String, String)> {
    let image = clipboard.get_image()?;
    let width = u32::try_from(image.width).context("clipboard image is too wide")?;
    let height = u32::try_from(image.height).context("clipboard image is too tall")?;
    let rgba = image.bytes.into_owned();
    let buffer = image::RgbaImage::from_raw(width, height, rgba)
        .context("clipboard image data had unexpected dimensions")?;
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .context("failed to encode clipboard image as png")?;
    Ok((
        "image/png".to_string(),
        base64::engine::general_purpose::STANDARD.encode(cursor.into_inner()),
    ))
}

fn clipboard_text(clipboard: &mut DesktopClipboard) -> Result<String> {
    clipboard.get_text()
}

#[derive(Clone, Debug, Default)]
struct ScrollLineAccumulator {
    velocity_lines_per_second: f32,
    last_event_at: Option<Instant>,
    last_frame_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollAnimationFrame {
    scroll_lines: Option<f32>,
    active: bool,
}

impl ScrollLineAccumulator {
    fn scroll_lines(&mut self, delta: MouseScrollDelta, now: Instant) -> Option<f32> {
        if self
            .last_event_at
            .is_some_and(|last| now.saturating_duration_since(last) > SCROLL_GESTURE_IDLE_RESET)
        {
            self.stop();
        }
        self.last_event_at = Some(now);
        self.last_frame_at = Some(now);
        self.input_delta(mouse_scroll_delta_lines(delta))
    }

    fn frame(&mut self, now: Instant) -> ScrollAnimationFrame {
        let Some(last_frame_at) = self.last_frame_at else {
            self.last_frame_at = Some(now);
            return ScrollAnimationFrame {
                scroll_lines: None,
                active: self.is_active(),
            };
        };

        let dt = now
            .saturating_duration_since(last_frame_at)
            .as_secs_f32()
            .min(SCROLL_FRAME_MAX_DT_SECONDS);
        self.last_frame_at = Some(now);

        if dt <= 0.0 || !self.is_active() {
            return ScrollAnimationFrame {
                scroll_lines: None,
                active: self.is_active(),
            };
        }

        let scroll_lines = if self.velocity_lines_per_second.abs() >= SCROLL_MOMENTUM_STOP_VELOCITY
        {
            let lines = self.velocity_lines_per_second * dt;
            let decay = (-SCROLL_MOMENTUM_DECAY_PER_SECOND * dt).exp();
            self.velocity_lines_per_second *= decay;
            if self.velocity_lines_per_second.abs() < SCROLL_MOMENTUM_STOP_VELOCITY {
                self.velocity_lines_per_second = 0.0;
            }
            (lines.abs() >= SCROLL_FRACTIONAL_EPSILON).then_some(lines)
        } else {
            self.velocity_lines_per_second = 0.0;
            None
        };

        ScrollAnimationFrame {
            scroll_lines,
            active: self.is_active(),
        }
    }

    fn reset(&mut self) {
        self.stop();
        self.last_event_at = None;
        self.last_frame_at = None;
    }

    fn stop(&mut self) {
        self.velocity_lines_per_second = 0.0;
    }

    fn pending_lines(&self) -> f32 {
        0.0
    }

    fn is_active(&self) -> bool {
        self.velocity_lines_per_second.abs() >= SCROLL_MOMENTUM_STOP_VELOCITY
    }

    fn input_delta(&mut self, lines: f32) -> Option<f32> {
        if !lines.is_finite() || lines.abs() < SCROLL_FRACTIONAL_EPSILON {
            return None;
        }

        let lines = lines.clamp(
            -MAX_MOUSE_SCROLL_LINES_PER_EVENT,
            MAX_MOUSE_SCROLL_LINES_PER_EVENT,
        );
        if self.velocity_lines_per_second.abs() >= SCROLL_MOMENTUM_STOP_VELOCITY
            && self.velocity_lines_per_second.signum() != lines.signum()
        {
            self.stop();
        }

        self.velocity_lines_per_second = (self.velocity_lines_per_second
            + lines * SCROLL_MOMENTUM_GAIN)
            .clamp(-SCROLL_MOMENTUM_MAX_VELOCITY, SCROLL_MOMENTUM_MAX_VELOCITY);
        Some(lines)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SingleSessionScrollMotionFrame {
    visual_scroll_lines: f32,
    smooth_scroll_lines: f32,
    active: bool,
}

#[derive(Clone, Debug, Default)]
struct SingleSessionScrollMotion {
    initialized: bool,
    start_lines: f32,
    current_lines: f32,
    target_lines: f32,
    started_at: Option<Instant>,
}

impl SingleSessionScrollMotion {
    fn frame(&mut self, target_lines: f32, now: Instant) -> SingleSessionScrollMotionFrame {
        let target_lines = if target_lines.is_finite() {
            target_lines.max(0.0)
        } else {
            0.0
        };

        if !self.initialized || animation::desktop_reduced_motion_enabled() {
            self.initialized = true;
            self.start_lines = target_lines;
            self.current_lines = target_lines;
            self.target_lines = target_lines;
            self.started_at = None;
            return SingleSessionScrollMotionFrame {
                visual_scroll_lines: target_lines,
                smooth_scroll_lines: 0.0,
                active: false,
            };
        }

        if (self.target_lines - target_lines).abs() >= SCROLL_FRACTIONAL_EPSILON {
            self.start_lines = self.current_lines;
            self.target_lines = target_lines;
            self.started_at = Some(now);
        }

        if let Some(started_at) = self.started_at {
            let progress = (now.saturating_duration_since(started_at).as_secs_f32()
                / SINGLE_SESSION_SCROLL_ANIMATION_DURATION.as_secs_f32())
            .clamp(0.0, 1.0);
            let eased = animation::ease_out_cubic(progress);
            self.current_lines = animation::lerp(self.start_lines, self.target_lines, eased);
            if progress >= 1.0
                || (self.current_lines - self.target_lines).abs() < SCROLL_FRACTIONAL_EPSILON
            {
                self.current_lines = self.target_lines;
                self.started_at = None;
            }
        }

        SingleSessionScrollMotionFrame {
            visual_scroll_lines: self.current_lines,
            smooth_scroll_lines: self.current_lines - target_lines,
            active: self.is_active(),
        }
    }

    fn is_active(&self) -> bool {
        self.started_at.is_some()
            || (self.current_lines - self.target_lines).abs() >= SCROLL_FRACTIONAL_EPSILON
    }

    fn clear(&mut self) {
        self.initialized = false;
        self.start_lines = 0.0;
        self.current_lines = 0.0;
        self.target_lines = 0.0;
        self.started_at = None;
    }
}

#[cfg(test)]
fn mouse_scroll_lines(delta: MouseScrollDelta) -> Option<f32> {
    ScrollLineAccumulator::default().scroll_lines(delta, Instant::now())
}

fn mouse_scroll_delta_lines(delta: MouseScrollDelta) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y * MOUSE_WHEEL_LINES_PER_DETENT,
        MouseScrollDelta::PixelDelta(position) => position.y as f32 / body_scroll_line_pixels(),
    }
}

fn body_scroll_line_pixels() -> f32 {
    let typography = single_session_typography();
    typography.body_size * typography.body_line_height
}

fn desktop_spinner_tick(_now: Instant) -> u64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    (millis / DESKTOP_SPINNER_FRAME_MS) as u64
}

fn single_session_text_buffer_cache_key(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    _tick: u64,
    rendered_body_key: u64,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    rendered_body_key.hash(&mut hasher);
    (size.width, size.height).hash(&mut hasher);
    app.is_welcome_timeline_visible().hash(&mut hasher);
    app.has_activity_indicator().hash(&mut hasher);
    app.text_scale().to_bits().hash(&mut hasher);
    app.header_title().hash(&mut hasher);
    app.welcome_hero_text().hash(&mut hasher);
    app.inline_widget_styled_lines().hash(&mut hasher);
    app.composer_text().hash(&mut hasher);
    hasher.finish()
}

fn single_session_body_text_window_bounds(viewport: &SingleSessionBodyViewport) -> (usize, usize) {
    let start = viewport
        .start_line
        .saturating_sub(SINGLE_SESSION_BODY_TEXT_WINDOW_BEFORE_LINES);
    let end = viewport
        .start_line
        .saturating_add(viewport.lines.len())
        .saturating_add(SINGLE_SESSION_BODY_TEXT_WINDOW_AFTER_LINES)
        .min(viewport.total_lines);
    (start, end.max(start))
}

fn single_session_body_text_window_contains(
    window_start: usize,
    window_end: usize,
    viewport: &SingleSessionBodyViewport,
) -> bool {
    let visible_end = viewport.start_line.saturating_add(viewport.lines.len());
    window_start <= viewport.start_line && visible_end <= window_end
}

#[derive(Default)]
struct SingleSessionScrollMetricsCache {
    key: Option<u64>,
    total_lines: usize,
    raw_body_key: Option<u64>,
    raw_body_lines: Vec<SingleSessionStyledLine>,
    streaming_base_key: Option<u64>,
    streaming_base_total_lines: usize,
}

impl SingleSessionScrollMetricsCache {
    fn metrics(
        &mut self,
        app: &SingleSessionApp,
        size: PhysicalSize<u32>,
    ) -> Option<SingleSessionBodyScrollMetrics> {
        let body_layout_size = single_session_body_layout_cache_size(app, size);
        let key = app.rendered_body_cache_key(body_layout_size);
        if self.key != Some(key) {
            if !app.streaming_response.is_empty() {
                let base_key = app.rendered_body_static_cache_key(body_layout_size);
                if self.streaming_base_key != Some(base_key) {
                    if let Some(base_lines) =
                        single_session_rendered_static_body_lines_for_streaming(app, size, 0)
                    {
                        self.streaming_base_total_lines = base_lines.len();
                        self.streaming_base_key = Some(base_key);
                    } else {
                        self.streaming_base_key = None;
                        self.streaming_base_total_lines = 0;
                    }
                }
                if self.streaming_base_key == Some(base_key) {
                    self.total_lines = self.streaming_base_total_lines
                        + single_session_streaming_response_rendered_body_line_count(app, size);
                } else {
                    self.total_lines =
                        single_session_rendered_body_lines_for_tick(app, size, 0).len();
                }
            } else {
                let raw_key = app.rendered_body_cache_key((0, 0));
                if self.raw_body_key != Some(raw_key) {
                    self.raw_body_lines = app.body_styled_lines_for_tick(0);
                    self.raw_body_key = Some(raw_key);
                }
                self.total_lines = single_session_rendered_body_lines_from_raw_ref(
                    app,
                    size,
                    &self.raw_body_lines,
                )
                .len();
                self.streaming_base_key = None;
                self.streaming_base_total_lines = 0;
            }
            self.key = Some(key);
        }
        single_session_body_scroll_metrics_for_total_lines(app, size, self.total_lines)
    }

    fn clear(&mut self) {
        self.key = None;
        self.total_lines = 0;
        self.raw_body_key = None;
        self.raw_body_lines.clear();
        self.streaming_base_key = None;
        self.streaming_base_total_lines = 0;
    }
}

#[derive(Clone)]
struct DesktopFrameStageProfile {
    name: &'static str,
    duration: Duration,
}

struct DesktopFrameProfile {
    started_at: Instant,
    last_checkpoint: Instant,
    stages: Vec<DesktopFrameStageProfile>,
}

impl DesktopFrameProfile {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started_at: now,
            last_checkpoint: now,
            stages: Vec::with_capacity(20),
        }
    }

    fn checkpoint(&mut self, name: &'static str) {
        let now = Instant::now();
        self.stages.push(DesktopFrameStageProfile {
            name,
            duration: now.saturating_duration_since(self.last_checkpoint),
        });
        self.last_checkpoint = now;
    }

    fn total_duration(&self) -> Duration {
        self.last_checkpoint
            .saturating_duration_since(self.started_at)
    }

    fn stage_duration(&self, name: &'static str) -> Duration {
        self.stages
            .iter()
            .filter(|stage| stage.name == name)
            .fold(Duration::ZERO, |total, stage| total + stage.duration)
    }

    fn cpu_duration(&self) -> Duration {
        self.stages
            .iter()
            .filter(|stage| !matches!(stage.name, "surface_acquire" | "queue_submit" | "present"))
            .fold(Duration::ZERO, |total, stage| total + stage.duration)
    }
}

#[derive(Clone, Copy)]
struct DesktopFrameContext {
    mode: &'static str,
    smooth_scroll_lines: f32,
    text_buffer_count: usize,
    text_area_count: usize,
    primitive_vertices: usize,
    body_line_count: usize,
    viewport_line_count: usize,
    body_text_window_line_count: usize,
    streaming_text_line_count: usize,
    inline_widget_line_count: usize,
    text_prepared: bool,
    primitive_geometry_cache_hit: bool,
}

#[derive(Clone)]
struct DesktopRenderFrameResult {
    animation_active: bool,
    content_ready: bool,
    frame_wall: Duration,
    frame_cpu: Duration,
    context: DesktopFrameContext,
    stages: Vec<DesktopFrameStageProfile>,
}

#[derive(Clone)]
struct DesktopFrameSlowSample {
    wall: Duration,
    cpu: Duration,
    surface_acquire: Duration,
    queue_submit: Duration,
    present: Duration,
    score: Duration,
    stages: Vec<DesktopFrameStageProfile>,
    context: DesktopFrameContext,
}

struct DesktopFrameProfiler {
    enabled: bool,
    log_all: bool,
    budget: Duration,
    report_interval: Duration,
    frames: usize,
    slow_cpu_frames: usize,
    present_stall_frames: usize,
    worst: Option<DesktopFrameSlowSample>,
    last_report: Option<Instant>,
}

impl DesktopFrameProfiler {
    fn new() -> Self {
        let mode = desktop_frame_profile_mode();
        let enabled = desktop_frame_profile_enabled(mode.as_deref());
        let log_all = desktop_frame_profile_log_all(mode.as_deref());
        let budget =
            duration_millis_env("JCODE_DESKTOP_FRAME_BUDGET_MS", DESKTOP_120FPS_FRAME_BUDGET);
        Self {
            enabled,
            log_all,
            budget,
            report_interval: DESKTOP_FRAME_PROFILE_REPORT_INTERVAL,
            frames: 0,
            slow_cpu_frames: 0,
            present_stall_frames: 0,
            worst: None,
            last_report: None,
        }
    }

    fn observe(&mut self, profile: DesktopFrameProfile, context: DesktopFrameContext) {
        if !self.enabled {
            return;
        }

        self.frames += 1;
        let wall = profile.total_duration();
        let cpu = profile.cpu_duration();
        let surface_acquire = profile.stage_duration("surface_acquire");
        let queue_submit = profile.stage_duration("queue_submit");
        let present = profile.stage_duration("present");
        let cpu_slow = cpu >= self.budget;
        let present_stall = surface_acquire >= DESKTOP_PRESENT_STALL_BUDGET
            || queue_submit >= DESKTOP_PRESENT_STALL_BUDGET
            || present >= DESKTOP_PRESENT_STALL_BUDGET;
        if cpu_slow || present_stall || self.log_all {
            if cpu_slow {
                self.slow_cpu_frames += 1;
            }
            if present_stall {
                self.present_stall_frames += 1;
            }
            let score = cpu.max(surface_acquire).max(queue_submit).max(present);
            let replace_worst = self
                .worst
                .as_ref()
                .is_none_or(|sample| score > sample.score);
            if replace_worst {
                self.worst = Some(DesktopFrameSlowSample {
                    wall,
                    cpu,
                    surface_acquire,
                    queue_submit,
                    present,
                    score,
                    stages: profile.stages,
                    context,
                });
            }
        }

        let now = Instant::now();
        let report_due = self.last_report.is_none_or(|last_report| {
            now.saturating_duration_since(last_report) >= self.report_interval
        });
        if report_due && (self.slow_cpu_frames > 0 || self.present_stall_frames > 0 || self.log_all)
        {
            self.report(now);
        }
    }

    fn report(&mut self, now: Instant) {
        if let Some(worst) = self.worst.as_ref() {
            emit_desktop_profile_event(
                "jcode-desktop-frame-profile",
                serde_json::json!({
                    "cpu_budget_ms": duration_ms(self.budget),
                    "present_stall_budget_ms": duration_ms(DESKTOP_PRESENT_STALL_BUDGET),
                    "window_frames": self.frames,
                    "slow_frames": self.slow_cpu_frames,
                    "slow_cpu_frames": self.slow_cpu_frames,
                    "present_stall_frames": self.present_stall_frames,
                    "worst_frame_ms": duration_ms(worst.wall),
                    "worst_wall_ms": duration_ms(worst.wall),
                    "worst_cpu_ms": duration_ms(worst.cpu),
                    "surface_acquire_ms": duration_ms(worst.surface_acquire),
                    "queue_submit_ms": duration_ms(worst.queue_submit),
                    "present_ms": duration_ms(worst.present),
                    "submit_present_ms": duration_ms(worst.queue_submit + worst.present),
                    "mode": worst.context.mode,
                    "smooth_scroll_lines": worst.context.smooth_scroll_lines,
                    "text_buffer_count": worst.context.text_buffer_count,
                    "text_area_count": worst.context.text_area_count,
                    "primitive_vertices": worst.context.primitive_vertices,
                    "body_line_count": worst.context.body_line_count,
                    "viewport_line_count": worst.context.viewport_line_count,
                    "body_text_window_line_count": worst.context.body_text_window_line_count,
                    "streaming_text_line_count": worst.context.streaming_text_line_count,
                    "inline_widget_line_count": worst.context.inline_widget_line_count,
                    "text_prepared": worst.context.text_prepared,
                    "primitive_geometry_cache_hit": worst.context.primitive_geometry_cache_hit,
                    "stages": worst.stages.iter().map(|stage| serde_json::json!({
                        "name": stage.name,
                        "ms": duration_ms(stage.duration),
                    })).collect::<Vec<_>>(),
                }),
            );
        }
        self.frames = 0;
        self.slow_cpu_frames = 0;
        self.present_stall_frames = 0;
        self.worst = None;
        self.last_report = Some(now);
    }
}

#[derive(Clone, Copy)]
struct DesktopPendingInteraction {
    kind: &'static str,
    started_at: Instant,
    count: usize,
}

struct DesktopInteractionLatencyProfiler {
    enabled: bool,
    log_all: bool,
    budget: Duration,
    pending: Option<DesktopPendingInteraction>,
}

impl DesktopInteractionLatencyProfiler {
    fn new() -> Self {
        let mode = desktop_frame_profile_mode();
        let enabled = desktop_frame_profile_enabled(mode.as_deref());
        let log_all = desktop_frame_profile_log_all(mode.as_deref());
        let budget = duration_millis_env(
            "JCODE_DESKTOP_INPUT_LATENCY_BUDGET_MS",
            DESKTOP_INPUT_LATENCY_BUDGET,
        );
        Self {
            enabled,
            log_all,
            budget,
            pending: None,
        }
    }

    fn mark(&mut self, kind: &'static str, started_at: Instant) {
        if !self.enabled {
            return;
        }
        match self.pending.as_mut() {
            Some(pending) => {
                if started_at < pending.started_at {
                    pending.started_at = started_at;
                }
                if pending.kind != kind {
                    pending.kind = "mixed";
                }
                pending.count += 1;
            }
            None => {
                self.pending = Some(DesktopPendingInteraction {
                    kind,
                    started_at,
                    count: 1,
                });
            }
        }
    }

    fn pending_kind(&self) -> Option<&'static str> {
        self.pending.as_ref().map(|pending| pending.kind)
    }

    fn observe_presented(&mut self, frame: &DesktopRenderFrameResult) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if !self.enabled {
            return;
        }
        let latency = Instant::now().saturating_duration_since(pending.started_at);
        if latency < self.budget && !self.log_all {
            return;
        }
        eprintln!(
            "jcode-desktop-latency-profile {}",
            serde_json::json!({
                "kind": pending.kind,
                "interaction_count": pending.count,
                "latency_budget_ms": duration_ms(self.budget),
                "latency_ms": duration_ms(latency),
                "frame_wall_ms": duration_ms(frame.frame_wall),
                "frame_cpu_ms": duration_ms(frame.frame_cpu),
                "mode": frame.context.mode,
                "smooth_scroll_lines": frame.context.smooth_scroll_lines,
                "text_buffer_count": frame.context.text_buffer_count,
                "text_area_count": frame.context.text_area_count,
                "primitive_vertices": frame.context.primitive_vertices,
                "body_line_count": frame.context.body_line_count,
                "viewport_line_count": frame.context.viewport_line_count,
                "body_text_window_line_count": frame.context.body_text_window_line_count,
                "streaming_text_line_count": frame.context.streaming_text_line_count,
                "inline_widget_line_count": frame.context.inline_widget_line_count,
                "text_prepared": frame.context.text_prepared,
                "stages": frame.stages.iter().map(|stage| serde_json::json!({
                    "name": stage.name,
                    "ms": duration_ms(stage.duration),
                })).collect::<Vec<_>>(),
            })
        );
    }
}

#[derive(Clone, Copy)]
struct NoPaintWatchdogContext {
    active: bool,
    mode: &'static str,
    has_background_work: bool,
    frame_animation_active: bool,
    pending_backend_redraw: bool,
    pending_interaction_kind: Option<&'static str>,
}

struct DesktopNoPaintWatchdog {
    enabled: bool,
    log_all: bool,
    budget: Duration,
    last_presented_at: Instant,
    last_reported_at: Option<Instant>,
    last_redraw_request_at: Option<Instant>,
}

impl DesktopNoPaintWatchdog {
    fn new() -> Self {
        let now = Instant::now();
        Self::new_with_start(now)
    }

    fn new_with_start(now: Instant) -> Self {
        let mode = desktop_frame_profile_mode();
        Self::new_with_start_and_mode(now, mode.as_deref())
    }

    fn new_with_start_and_mode(now: Instant, mode: Option<&str>) -> Self {
        let enabled = desktop_frame_profile_enabled(mode);
        let log_all = desktop_frame_profile_log_all(mode);
        let budget =
            duration_millis_env("JCODE_DESKTOP_NO_PAINT_BUDGET_MS", DESKTOP_NO_PAINT_BUDGET);
        Self {
            enabled,
            log_all,
            budget,
            last_presented_at: now,
            last_reported_at: None,
            last_redraw_request_at: None,
        }
    }

    fn observe_presented(&mut self, now: Instant, _frame: &DesktopRenderFrameResult) {
        self.last_presented_at = now;
        self.last_reported_at = None;
        self.last_redraw_request_at = None;
    }

    fn observe_active_tick(&mut self, now: Instant, context: NoPaintWatchdogContext) -> bool {
        if !self.enabled {
            return false;
        }
        if !context.active {
            self.last_reported_at = None;
            self.last_redraw_request_at = None;
            return false;
        }
        let gap = now.saturating_duration_since(self.last_presented_at);
        if gap < self.budget && !self.log_all {
            return false;
        }

        let report_due = self.last_reported_at.is_none_or(|last_reported| {
            now.saturating_duration_since(last_reported) >= DESKTOP_FRAME_PROFILE_REPORT_INTERVAL
        });
        if report_due {
            self.last_reported_at = Some(now);
            emit_desktop_profile_event(
                "jcode-desktop-no-paint-profile",
                serde_json::json!({
                    "budget_ms": duration_ms(self.budget),
                    "gap_ms": duration_ms(gap),
                    "mode": context.mode,
                    "has_background_work": context.has_background_work,
                    "frame_animation_active": context.frame_animation_active,
                    "pending_backend_redraw": context.pending_backend_redraw,
                    "pending_interaction_kind": context.pending_interaction_kind,
                }),
            );
        }

        let redraw_due = self.last_redraw_request_at.is_none_or(|last_request| {
            now.saturating_duration_since(last_request) >= BACKEND_REDRAW_FRAME_INTERVAL
        });
        if redraw_due {
            self.last_redraw_request_at = Some(now);
        }
        redraw_due
    }
}

#[cfg(test)]
mod desktop_no_paint_watchdog_tests {
    use super::*;

    #[test]
    fn no_paint_watchdog_requests_redraw_after_active_gap_budget() {
        let start = Instant::now();
        let mut watchdog = DesktopNoPaintWatchdog::new_with_start_and_mode(start, Some("1"));
        let context = NoPaintWatchdogContext {
            active: true,
            mode: "single_session",
            has_background_work: true,
            frame_animation_active: false,
            pending_backend_redraw: false,
            pending_interaction_kind: Some("backend_events"),
        };

        assert!(!watchdog.observe_active_tick(start + watchdog.budget / 2, context));
        assert!(watchdog.observe_active_tick(start + watchdog.budget, context));
        assert!(!watchdog.observe_active_tick(
            start + watchdog.budget + BACKEND_REDRAW_FRAME_INTERVAL / 2,
            context
        ));
        assert!(watchdog.observe_active_tick(
            start + watchdog.budget + BACKEND_REDRAW_FRAME_INTERVAL,
            context
        ));
    }

    #[test]
    fn no_paint_watchdog_resets_when_idle_or_presented() {
        let start = Instant::now();
        let mut watchdog = DesktopNoPaintWatchdog::new_with_start_and_mode(start, Some("1"));
        let active_context = NoPaintWatchdogContext {
            active: true,
            mode: "single_session",
            has_background_work: true,
            frame_animation_active: false,
            pending_backend_redraw: false,
            pending_interaction_kind: None,
        };
        let idle_context = NoPaintWatchdogContext {
            active: false,
            ..active_context
        };

        assert!(watchdog.observe_active_tick(start + watchdog.budget, active_context));
        assert!(
            !watchdog.observe_active_tick(start + watchdog.budget + watchdog.budget, idle_context)
        );
        assert!(watchdog.last_redraw_request_at.is_none());
    }

    #[test]
    fn no_paint_watchdog_is_off_without_explicit_frame_profile_mode() {
        let start = Instant::now();
        let mut watchdog = DesktopNoPaintWatchdog::new_with_start_and_mode(start, None);
        let context = NoPaintWatchdogContext {
            active: true,
            mode: "single_session",
            has_background_work: true,
            frame_animation_active: false,
            pending_backend_redraw: false,
            pending_interaction_kind: Some("backend_events"),
        };

        assert!(!watchdog.enabled);
        assert!(!watchdog.observe_active_tick(start + DESKTOP_NO_PAINT_BUDGET * 4, context));
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

static DESKTOP_PROFILE_LOG_TX: OnceLock<Option<mpsc::Sender<DesktopProfileLogLine>>> =
    OnceLock::new();
static DESKTOP_PROFILE_LAUNCH_ID: OnceLock<String> = OnceLock::new();

#[derive(Debug)]
struct DesktopProfileLogLine {
    stderr_line: String,
    jsonl_line: String,
}

fn desktop_profile_log_path() -> Option<PathBuf> {
    if std::env::var_os("JCODE_DESKTOP_PROFILE_LOG").is_some_and(|value| !env_flag_enabled(value)) {
        return None;
    }
    if let Some(path) = std::env::var_os("JCODE_DESKTOP_PROFILE_LOG_PATH") {
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(
        cache_root
            .join("jcode")
            .join("desktop")
            .join("performance.log"),
    )
}

fn desktop_profile_stderr_enabled() -> bool {
    std::env::var_os("JCODE_DESKTOP_PROFILE_STDERR").is_none_or(env_flag_enabled)
}

fn desktop_profile_launch_id() -> &'static str {
    DESKTOP_PROFILE_LAUNCH_ID
        .get_or_init(|| {
            let timestamp_unix_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
                .unwrap_or_default();
            format!("{timestamp_unix_ms}-{}", std::process::id())
        })
        .as_str()
}

fn desktop_profile_log_sender() -> Option<&'static mpsc::Sender<DesktopProfileLogLine>> {
    DESKTOP_PROFILE_LOG_TX
        .get_or_init(|| {
            let path = desktop_profile_log_path();
            let stderr_enabled = desktop_profile_stderr_enabled();
            if path.is_none() && !stderr_enabled {
                return None;
            }
            let (tx, rx) = mpsc::channel::<DesktopProfileLogLine>();
            match std::thread::Builder::new()
                .name("jcode-desktop-profile-log".to_string())
                .spawn(move || {
                    let mut file = path.and_then(|path| {
                        if let Some(parent) = path.parent()
                            && let Err(error) = std::fs::create_dir_all(parent)
                        {
                            desktop_log::error(format_args!(
                                "jcode-desktop: failed to create profile log directory {}: {error}",
                                parent.display()
                            ));
                            return None;
                        }
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(file) => Some(file),
                            Err(error) => {
                                desktop_log::error(format_args!(
                                    "jcode-desktop: failed to open profile log {}: {error}",
                                    path.display()
                                ));
                                None
                            }
                        }
                    });
                    while let Ok(line) = rx.recv() {
                        if stderr_enabled {
                            eprintln!("{}", line.stderr_line);
                        }
                        if let Some(profile_file) = file.as_mut()
                            && let Err(error) = writeln!(profile_file, "{}", line.jsonl_line)
                        {
                            desktop_log::error(format_args!(
                                "jcode-desktop: failed to write profile log: {error}"
                            ));
                            file = None;
                        }
                    }
                }) {
                Ok(_) => Some(tx),
                Err(error) => {
                    desktop_log::error(format_args!(
                        "jcode-desktop: failed to start profile logger: {error:#}"
                    ));
                    None
                }
            }
        })
        .as_ref()
}

fn emit_desktop_profile_event(event: &'static str, payload: serde_json::Value) {
    let timestamp_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default();
    if let Some(tx) = desktop_profile_log_sender() {
        let stderr_line = format!("{event} {payload}");
        let jsonl_line = serde_json::json!({
            "timestamp_unix_ms": timestamp_unix_ms,
            "launch_id": desktop_profile_launch_id(),
            "build_hash": desktop_build_hash_label(),
            "pid": std::process::id(),
            "event": event,
            "payload": payload,
        })
        .to_string();
        if tx
            .send(DesktopProfileLogLine {
                stderr_line,
                jsonl_line,
            })
            .is_err()
        {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to queue profile event {event}, logger is closed"
            ));
        }
    }
}

fn log_desktop_slow_interaction(
    kind: &'static str,
    duration: Duration,
    details: serde_json::Value,
) {
    if duration < DESKTOP_120FPS_FRAME_BUDGET {
        return;
    }
    let mode = desktop_frame_profile_mode();
    if !desktop_frame_profile_enabled(mode.as_deref()) {
        return;
    }
    emit_desktop_profile_event(
        "jcode-desktop-interaction-profile",
        serde_json::json!({
            "kind": kind,
            "budget_ms": duration_ms(DESKTOP_120FPS_FRAME_BUDGET),
            "duration_ms": duration_ms(duration),
            "details": details,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn single_session_streaming_primitive_geometry_cache_key(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
    spinner_tick: u64,
    smooth_scroll_lines: f32,
    welcome_hero_reveal_progress: f32,
    tool_motion_cache_key: u64,
    inline_widget_list_reflow_cache_key: u64,
    inline_widget_preview_pane_cache_key: u64,
    composer_motion_cache_key: u64,
    attachment_chip_motion_cache_key: u64,
    stdin_overlay_motion_cache_key: u64,
    transcript_message_motion_cache_key: u64,
    transcript_motion_cache_key: u64,
    inline_markdown_motion_cache_key: u64,
    activity_cue_motion_cache_key: u64,
    scrollbar_motion_cache_key: u64,
    body_key: Option<u64>,
    body_line_count: usize,
) -> Option<u64> {
    let body_key = body_key?;
    if app.streaming_response.is_empty()
        || app.show_help
        || app.model_picker.open
        || app.model_picker.loading
        || app.session_switcher.open
        || app.session_switcher.loading
        || app.render_inline_widget_line_count() > 0
        || app.stdin_response.is_some()
        || app.has_active_selection()
        || app.is_welcome_timeline_visible()
    {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    (size.width, size.height).hash(&mut hasher);
    app.text_scale().to_bits().hash(&mut hasher);
    app.body_scroll_lines.to_bits().hash(&mut hasher);
    smooth_scroll_lines.to_bits().hash(&mut hasher);
    focus_pulse.to_bits().hash(&mut hasher);
    welcome_hero_reveal_progress.to_bits().hash(&mut hasher);
    tool_motion_cache_key.hash(&mut hasher);
    inline_widget_list_reflow_cache_key.hash(&mut hasher);
    inline_widget_preview_pane_cache_key.hash(&mut hasher);
    composer_motion_cache_key.hash(&mut hasher);
    attachment_chip_motion_cache_key.hash(&mut hasher);
    stdin_overlay_motion_cache_key.hash(&mut hasher);
    transcript_message_motion_cache_key.hash(&mut hasher);
    transcript_motion_cache_key.hash(&mut hasher);
    inline_markdown_motion_cache_key.hash(&mut hasher);
    activity_cue_motion_cache_key.hash(&mut hasher);
    scrollbar_motion_cache_key.hash(&mut hasher);
    spinner_tick.hash(&mut hasher);
    app.is_processing.hash(&mut hasher);
    app.status.hash(&mut hasher);
    app.error.hash(&mut hasher);
    app.pending_images.len().hash(&mut hasher);
    app.messages.len().hash(&mut hasher);
    app.draft.len().hash(&mut hasher);
    app.draft_cursor.hash(&mut hasher);
    app.draft
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        .hash(&mut hasher);
    body_key.hash(&mut hasher);
    body_line_count.hash(&mut hasher);
    Some(hasher.finish())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AppModeTransitionFrame {
    previous_opacity: f32,
    previous_scale: f32,
    current_opacity: f32,
    current_scale: f32,
}

#[derive(Default)]
struct AppModeTransitionState {
    last_mode: Option<&'static str>,
    started_at: Option<Instant>,
    previous_vertices: Vec<Vertex>,
    last_vertices: Vec<Vertex>,
}

impl AppModeTransitionState {
    fn frame(&mut self, mode: &'static str, now: Instant) -> Option<AppModeTransitionFrame> {
        if animation::desktop_reduced_motion_enabled() {
            self.last_mode = Some(mode);
            self.started_at = None;
            self.previous_vertices.clear();
            return None;
        }

        match self.last_mode {
            None => {
                self.last_mode = Some(mode);
                return None;
            }
            Some(previous_mode) if previous_mode != mode => {
                self.last_mode = Some(mode);
                self.previous_vertices.clear();
                self.previous_vertices
                    .extend_from_slice(&self.last_vertices);
                self.started_at = (!self.previous_vertices.is_empty()).then_some(now);
            }
            Some(_) => {}
        }

        let started_at = self.started_at?;
        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / APP_MODE_TRANSITION_DURATION.as_secs_f32())
        .clamp(0.0, 1.0);
        if progress >= 1.0 {
            self.started_at = None;
            self.previous_vertices.clear();
            return None;
        }

        let eased = animation::ease_out_cubic(progress);
        Some(AppModeTransitionFrame {
            previous_opacity: 1.0 - eased,
            previous_scale: animation::lerp(1.0, 0.985, eased),
            current_opacity: eased,
            current_scale: animation::lerp(0.985, 1.0, eased),
        })
    }

    fn previous_vertices(&self) -> &[Vertex] {
        &self.previous_vertices
    }

    fn remember_uploaded_vertices(&mut self, vertices: &[Vertex]) {
        self.last_vertices.clear();
        self.last_vertices.extend_from_slice(vertices);
    }

    fn clear(&mut self) {
        self.last_mode = None;
        self.started_at = None;
        self.previous_vertices.clear();
        self.last_vertices.clear();
    }
}

fn compose_app_mode_transition_vertices(
    output: &mut Vec<Vertex>,
    previous_vertices: &[Vertex],
    current_vertices: &[Vertex],
    frame: AppModeTransitionFrame,
) {
    output.clear();
    append_app_mode_transition_vertices(
        output,
        previous_vertices,
        frame.previous_opacity,
        frame.previous_scale,
    );
    append_app_mode_transition_vertices(
        output,
        current_vertices,
        frame.current_opacity,
        frame.current_scale,
    );
}

fn append_app_mode_transition_vertices(
    output: &mut Vec<Vertex>,
    vertices: &[Vertex],
    opacity: f32,
    scale: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }
    output.extend(vertices.iter().map(|vertex| {
        let mut color = vertex.color;
        color[3] *= opacity;
        Vertex {
            position: [vertex.position[0] * scale, vertex.position[1] * scale],
            color,
        }
    }));
}

struct Canvas {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: Option<wgpu::RenderPipeline>,
    hero_mask_renderer: Option<HeroMaskRenderer>,
    font_system_loader: Option<JoinHandle<FontSystem>>,
    font_system: Option<FontSystem>,
    swash_cache: SwashCache,
    text_atlas: Option<TextAtlas>,
    text_renderer: Option<TextRenderer>,
    text_needs_prepare: bool,
    streaming_text_atlas: Option<TextAtlas>,
    streaming_text_renderer: Option<TextRenderer>,
    streaming_text_needs_prepare: bool,
    size: PhysicalSize<u32>,
    surface_zero_sized: bool,
    viewport_animation: AnimatedViewport,
    surface_transitions: SurfaceTransitionAnimator,
    workspace_surface_exit_cache: HashMap<u64, workspace::Surface>,
    workspace_text_pane_cache: HashMap<u64, CachedWorkspaceSingleSessionTextPane>,
    focus_pulse: FocusPulse,
    status_color_transition: ColorTransition,
    status_text_transition: StatusTextTransition,
    inline_widget_selection_motion: InlineWidgetSelectionMotionRegistry,
    inline_widget_list_reflow_motion: InlineWidgetListReflowMotionRegistry,
    inline_widget_preview_pane_motion: InlineWidgetPreviewPaneMotionRegistry,
    composer_motion: ComposerMotionRegistry,
    attachment_chip_motion: AttachmentChipMotionRegistry,
    stdin_overlay_motion: StdinOverlayMotionRegistry,
    transcript_card_motion: TranscriptCardMotionRegistry,
    inline_markdown_pill_motion: InlineMarkdownPillMotionRegistry,
    streaming_activity_cue_motion: StreamingActivityCueMotionRegistry,
    tool_card_motion: ToolCardMotionRegistry,
    single_session_scrollbar_motion: SingleSessionScrollbarMotionRegistry,
    primitive_vertex_buffer: Option<wgpu::Buffer>,
    primitive_vertex_capacity: usize,
    primitive_vertices_cache_key: Option<u64>,
    primitive_vertices_cache: Vec<Vertex>,
    primitive_frame_vertices: Vec<Vertex>,
    primitive_workspace_vertices: Vec<Vertex>,
    app_mode_transition: AppModeTransitionState,
    app_mode_transition_vertices: Vec<Vertex>,
    single_session_scroll_motion: SingleSessionScrollMotion,
    transcript_message_motion: TranscriptMessageMotionRegistry,
    needs_initial_frame: bool,
    boot_frame_presented: bool,
    first_render_completed: bool,
    defer_initial_text_frame: bool,
    single_session_text_cache_key: Option<u64>,
    single_session_text_key: Option<SingleSessionTextKey>,
    single_session_text_buffers: Vec<Buffer>,
    single_session_raw_body_key: Option<u64>,
    single_session_raw_body_lines: Vec<SingleSessionStyledLine>,
    single_session_body_key: Option<u64>,
    single_session_body_lines: Vec<SingleSessionStyledLine>,
    single_session_streaming_base_key: Option<u64>,
    single_session_streaming_base_len: usize,
    single_session_streaming_response_len: usize,
    single_session_streaming_fade_started_at: Option<Instant>,
    single_session_streaming_handoff_started_at: Option<Instant>,
    single_session_streaming_text_key: Option<u64>,
    single_session_streaming_text_start_line: Option<usize>,
    single_session_streaming_text_end_line: Option<usize>,
    single_session_streaming_text_opacity_bits: Option<u32>,
    single_session_streaming_text_buffer: Option<Buffer>,
    single_session_body_text_scroll_start: Option<usize>,
    single_session_body_text_window_start: Option<usize>,
    single_session_body_text_window_end: Option<usize>,
    welcome_hero_reveal_key: Option<String>,
    welcome_hero_reveal_started_at: Option<Instant>,
    frame_profiler: DesktopFrameProfiler,
}

impl Canvas {
    async fn new(window: Arc<Window>, startup_trace: DesktopStartupTrace) -> Result<Self> {
        let initial_window_size = window.inner_size();
        let size = non_zero_size(initial_window_size);
        let font_system_loader = Some(spawn_desktop_font_system_loader());
        startup_trace.mark("font loader spawned");
        let (surface, adapter) = request_startup_adapter(
            window.clone(),
            desktop_wgpu_startup_backends(),
            startup_trace,
        )
        .await?;
        startup_trace.mark("wgpu adapter ready");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("jcode-desktop-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .context("failed to create wgpu device")?;
        startup_trace.mark("wgpu device ready");
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let present_mode = if capabilities.present_modes.contains(&PresentMode::Fifo) {
            PresentMode::Fifo
        } else {
            capabilities.present_modes[0]
        };
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&CompositeAlphaMode::Opaque)
        {
            CompositeAlphaMode::Opaque
        } else {
            capabilities.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        startup_trace.mark("surface configured");
        startup_trace.mark("first-frame GPU core ready");
        let swash_cache = SwashCache::new();
        Ok(Self {
            surface,
            device,
            queue,
            config,
            render_pipeline: None,
            hero_mask_renderer: None,
            font_system_loader,
            font_system: None,
            swash_cache,
            text_atlas: None,
            text_renderer: None,
            text_needs_prepare: true,
            streaming_text_atlas: None,
            streaming_text_renderer: None,
            streaming_text_needs_prepare: false,
            size,
            surface_zero_sized: !desktop_surface_size_is_renderable(initial_window_size),
            viewport_animation: AnimatedViewport::default(),
            surface_transitions: SurfaceTransitionAnimator::default(),
            workspace_surface_exit_cache: HashMap::new(),
            workspace_text_pane_cache: HashMap::new(),
            focus_pulse: FocusPulse::default(),
            status_color_transition: ColorTransition::default(),
            status_text_transition: StatusTextTransition::default(),
            inline_widget_selection_motion: InlineWidgetSelectionMotionRegistry::default(),
            inline_widget_list_reflow_motion: InlineWidgetListReflowMotionRegistry::default(),
            inline_widget_preview_pane_motion: InlineWidgetPreviewPaneMotionRegistry::default(),
            composer_motion: ComposerMotionRegistry::default(),
            attachment_chip_motion: AttachmentChipMotionRegistry::default(),
            stdin_overlay_motion: StdinOverlayMotionRegistry::default(),
            transcript_card_motion: TranscriptCardMotionRegistry::default(),
            inline_markdown_pill_motion: InlineMarkdownPillMotionRegistry::default(),
            streaming_activity_cue_motion: StreamingActivityCueMotionRegistry::default(),
            tool_card_motion: ToolCardMotionRegistry::default(),
            single_session_scrollbar_motion: SingleSessionScrollbarMotionRegistry::default(),
            primitive_vertex_buffer: None,
            primitive_vertex_capacity: 0,
            primitive_vertices_cache_key: None,
            primitive_vertices_cache: Vec::new(),
            primitive_frame_vertices: Vec::new(),
            primitive_workspace_vertices: Vec::new(),
            app_mode_transition: AppModeTransitionState::default(),
            app_mode_transition_vertices: Vec::new(),
            single_session_scroll_motion: SingleSessionScrollMotion::default(),
            transcript_message_motion: TranscriptMessageMotionRegistry::default(),
            needs_initial_frame: true,
            boot_frame_presented: false,
            first_render_completed: false,
            defer_initial_text_frame: false,
            single_session_text_cache_key: None,
            single_session_text_key: None,
            single_session_text_buffers: Vec::new(),
            single_session_raw_body_key: None,
            single_session_raw_body_lines: Vec::new(),
            single_session_body_key: None,
            single_session_body_lines: Vec::new(),
            single_session_streaming_base_key: None,
            single_session_streaming_base_len: 0,
            single_session_streaming_response_len: 0,
            single_session_streaming_fade_started_at: None,
            single_session_streaming_handoff_started_at: None,
            single_session_streaming_text_key: None,
            single_session_streaming_text_start_line: None,
            single_session_streaming_text_end_line: None,
            single_session_streaming_text_opacity_bits: None,
            single_session_streaming_text_buffer: None,
            single_session_body_text_scroll_start: None,
            single_session_body_text_window_start: None,
            single_session_body_text_window_end: None,
            welcome_hero_reveal_key: None,
            welcome_hero_reveal_started_at: None,
            frame_profiler: DesktopFrameProfiler::new(),
        })
    }

    fn suspend_for_zero_size(&mut self, size: PhysicalSize<u32>) {
        if !self.surface_zero_sized {
            desktop_log::info(format_args!(
                "jcode-desktop: suspending surface rendering while window is zero-sized ({}x{})",
                size.width, size.height
            ));
        }
        self.surface_zero_sized = true;
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if !desktop_surface_size_is_renderable(size) {
            self.suspend_for_zero_size(size);
            return;
        }

        let was_zero_sized = self.surface_zero_sized;
        self.surface_zero_sized = false;
        if was_zero_sized {
            desktop_log::info(format_args!(
                "jcode-desktop: resuming surface rendering at {}x{}",
                size.width, size.height
            ));
        }

        if self.size == size && !was_zero_sized {
            return;
        }

        self.size = size;
        self.primitive_vertices_cache_key = None;
        self.primitive_vertices_cache.clear();
        self.primitive_frame_vertices.clear();
        self.workspace_text_pane_cache.clear();
        self.app_mode_transition.clear();
        self.app_mode_transition_vertices.clear();
        self.single_session_scroll_motion.clear();
        self.transcript_message_motion.clear();
        self.inline_widget_preview_pane_motion.clear();
        self.streaming_activity_cue_motion.clear();
        self.single_session_streaming_handoff_started_at = None;
        self.first_render_completed = false;
        self.text_needs_prepare = true;
        if self.single_session_streaming_text_buffer.is_some() {
            self.streaming_text_needs_prepare = true;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    fn refresh_cached_single_session_text_buffers(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        now: Instant,
        smooth_scroll_lines: f32,
        rendered_body_key: u64,
        rendered_body_changed: bool,
    ) {
        let tick = desktop_spinner_tick(now);
        let viewport = single_session_body_viewport_from_lines(
            app,
            render_size,
            smooth_scroll_lines,
            &self.single_session_body_lines,
        );
        let text_cache_key =
            single_session_text_buffer_cache_key(app, render_size, tick, rendered_body_key);
        let key = single_session_text_key_for_tick_with_rendered_body(
            app,
            render_size,
            tick,
            smooth_scroll_lines,
            &self.single_session_body_lines,
        );
        let text_key_changed = self.single_session_text_key.as_ref() != Some(&key);
        if self.single_session_text_cache_key != Some(text_cache_key) || text_key_changed {
            let desired_body_window = self.single_session_body_buffer_window_bounds(app, &viewport);
            let body_window_contains = if let (Some(window_start), Some(window_end)) = (
                self.single_session_body_text_window_start,
                self.single_session_body_text_window_end,
            ) {
                self.single_session_body_buffer_window_contains(
                    app,
                    window_start,
                    window_end,
                    &viewport,
                )
            } else {
                false
            };
            let Some(font_system) = self.font_system.as_mut() else {
                self.single_session_text_cache_key = None;
                self.single_session_text_key = None;
                self.single_session_text_buffers.clear();
                self.single_session_body_text_scroll_start = None;
                self.single_session_body_text_window_start = None;
                self.single_session_body_text_window_end = None;
                return;
            };
            let previous_key = self.single_session_text_key.take();
            let mut old_buffers = std::mem::take(&mut self.single_session_text_buffers);
            let body_content_changed_in_buffer =
                rendered_body_changed && app.streaming_response.is_empty();
            let body_layout_compatible = previous_key.as_ref().is_some_and(|previous| {
                single_session_body_text_buffer_layout_compatible(
                    previous.size,
                    render_size,
                    app.text_scale(),
                )
            });
            let mut can_reuse_body_buffer = old_buffers.len() > 1
                && body_window_contains
                && !body_content_changed_in_buffer
                && body_layout_compatible;
            if old_buffers.len() > 1
                && (!body_window_contains
                    || body_content_changed_in_buffer
                    || !body_layout_compatible)
            {
                let (window_start, window_end) = desired_body_window;
                old_buffers[1] = single_session_body_text_buffer_from_lines(
                    font_system,
                    &self.single_session_body_lines[window_start..window_end],
                    render_size,
                    app.text_scale(),
                );
                self.single_session_body_text_window_start = Some(window_start);
                self.single_session_body_text_window_end = Some(window_end);
                self.single_session_body_text_scroll_start = None;
                can_reuse_body_buffer = true;
            }
            self.single_session_text_buffers =
                single_session_text_buffers_from_key_reusing_unchanged(
                    &key,
                    previous_key.as_ref(),
                    old_buffers,
                    can_reuse_body_buffer,
                    render_size,
                    font_system,
                );
            self.single_session_text_key = Some(key);
            self.single_session_text_cache_key = Some(text_cache_key);
            if !can_reuse_body_buffer {
                self.single_session_body_text_scroll_start = None;
                self.single_session_body_text_window_start = None;
                self.single_session_body_text_window_end = None;
            }
            self.text_needs_prepare = true;
        }
        self.sync_single_session_body_text_window(app, render_size, &viewport);
    }

    fn sync_single_session_body_text_window(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        viewport: &SingleSessionBodyViewport,
    ) {
        let desired_body_window = self.single_session_body_buffer_window_bounds(app, viewport);
        if let (Some(window_start), Some(window_end)) = (
            self.single_session_body_text_window_start,
            self.single_session_body_text_window_end,
        ) && self.single_session_body_buffer_window_contains(
            app,
            window_start,
            window_end,
            viewport,
        ) {
            self.sync_single_session_body_text_scroll(viewport.start_line, window_start);
            self.sync_single_session_streaming_text_buffer(app, render_size, viewport);
            return;
        }

        let (window_start, window_end) = desired_body_window;
        let window_lines = self.single_session_body_lines[window_start..window_end].to_vec();
        if let Some(font_system) = self.font_system.as_mut()
            && let Some(body_buffer) = self.single_session_text_buffers.get_mut(1)
        {
            *body_buffer = single_session_body_text_buffer_from_lines(
                font_system,
                &window_lines,
                render_size,
                app.text_scale(),
            );
            self.single_session_body_text_window_start = Some(window_start);
            self.single_session_body_text_window_end = Some(window_end);
            self.single_session_body_text_scroll_start = None;
            self.sync_single_session_body_text_scroll(viewport.start_line, window_start);
        }
        self.sync_single_session_streaming_text_buffer(app, render_size, viewport);
    }

    fn single_session_body_buffer_window_bounds(
        &self,
        app: &SingleSessionApp,
        viewport: &SingleSessionBodyViewport,
    ) -> (usize, usize) {
        let (window_start, window_end) = single_session_body_text_window_bounds(viewport);
        if app.streaming_response.is_empty() || self.single_session_streaming_base_len == 0 {
            return (window_start, window_end);
        }
        let visible_static_start = viewport
            .start_line
            .min(self.single_session_streaming_base_len);
        let visible_static_end = viewport
            .start_line
            .saturating_add(viewport.lines.len())
            .min(self.single_session_streaming_base_len);
        let start = visible_static_start
            .saturating_sub(SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_BEFORE_LINES);
        let end = visible_static_end
            .saturating_add(SINGLE_SESSION_STREAMING_BODY_TEXT_WINDOW_AFTER_LINES)
            .min(self.single_session_streaming_base_len)
            .max(start);
        (start, end)
    }

    fn single_session_body_buffer_window_contains(
        &self,
        app: &SingleSessionApp,
        window_start: usize,
        window_end: usize,
        viewport: &SingleSessionBodyViewport,
    ) -> bool {
        if app.streaming_response.is_empty() || self.single_session_streaming_base_len == 0 {
            return single_session_body_text_window_contains(window_start, window_end, viewport);
        }
        let (desired_start, desired_end) =
            self.single_session_body_buffer_window_bounds(app, viewport);
        window_start == desired_start && window_end == desired_end
    }

    fn sync_single_session_streaming_text_buffer(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        viewport: &SingleSessionBodyViewport,
    ) {
        self.update_single_session_streaming_fade(app);
        let Some((start_line, end_line)) =
            self.single_session_streaming_visible_range(app, viewport)
        else {
            if app.streaming_response.is_empty()
                && self.single_session_streaming_handoff_started_at.is_some()
                && self.single_session_streaming_text_buffer.is_some()
            {
                self.streaming_text_needs_prepare = true;
                return;
            }
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.single_session_streaming_handoff_started_at = None;
            self.streaming_text_needs_prepare = false;
            return;
        };

        let mut hasher = DefaultHasher::new();
        (render_size.width, render_size.height).hash(&mut hasher);
        app.text_scale().to_bits().hash(&mut hasher);
        start_line.hash(&mut hasher);
        end_line.hash(&mut hasher);
        self.single_session_body_lines[start_line..end_line].hash(&mut hasher);
        let key = hasher.finish();
        if self.single_session_streaming_text_key == Some(key) {
            return;
        }

        if let Some(font_system) = self.font_system.as_mut() {
            let lines = self.single_session_body_lines[start_line..end_line].to_vec();
            self.single_session_streaming_text_buffer =
                Some(single_session_body_text_buffer_from_lines_with_opacity(
                    font_system,
                    &lines,
                    render_size,
                    app.text_scale(),
                    1.0,
                ));
            self.single_session_streaming_text_key = Some(key);
            self.single_session_streaming_text_start_line = Some(start_line);
            self.single_session_streaming_text_end_line = Some(end_line);
            self.single_session_streaming_text_opacity_bits = Some(1.0f32.to_bits());
            self.streaming_text_needs_prepare = true;
        }
    }

    fn update_single_session_streaming_fade(&mut self, app: &SingleSessionApp) {
        let now = Instant::now();
        let previous_len = self.single_session_streaming_response_len;
        let response_len = app.streaming_response.len();
        let has_visible_streaming_buffer = self.single_session_streaming_text_buffer.is_some()
            && self.single_session_streaming_text_start_line.is_some()
            && self.single_session_streaming_text_end_line.is_some();
        self.single_session_streaming_handoff_started_at =
            streaming_text_handoff_start_after_len_change(
                previous_len,
                response_len,
                has_visible_streaming_buffer,
                self.single_session_streaming_handoff_started_at,
                now,
            );
        self.single_session_streaming_fade_started_at = streaming_text_fade_start_after_len_change(
            previous_len,
            response_len,
            self.single_session_streaming_fade_started_at,
            now,
        );
        self.single_session_streaming_response_len = response_len;
    }

    fn single_session_streaming_arrival_style(
        &mut self,
        now: Instant,
    ) -> StreamingTextArrivalStyle {
        if let Some(started_at) = self.single_session_streaming_handoff_started_at {
            let style =
                streaming_text_handoff_style_for_elapsed(now.saturating_duration_since(started_at));
            if style.active {
                return style;
            }
            self.single_session_streaming_handoff_started_at = None;
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.streaming_text_needs_prepare = false;
            return style;
        }

        let Some(started_at) = self.single_session_streaming_fade_started_at else {
            return StreamingTextArrivalStyle {
                opacity: 1.0,
                y_offset_pixels: 0.0,
                active: false,
            };
        };
        let style =
            streaming_text_arrival_style_for_elapsed(now.saturating_duration_since(started_at));
        if !style.active {
            self.single_session_streaming_fade_started_at = None;
            return StreamingTextArrivalStyle {
                opacity: 1.0,
                y_offset_pixels: 0.0,
                active: false,
            };
        }
        style
    }

    fn update_single_session_streaming_text_buffer_opacity(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        opacity: f32,
    ) {
        let opacity = opacity.clamp(0.0, 1.0);
        let quantized_opacity = (opacity * 255.0).round() / 255.0;
        let opacity_bits = quantized_opacity.to_bits();
        if self.single_session_streaming_text_opacity_bits == Some(opacity_bits) {
            return;
        }
        let (Some(start_line), Some(end_line), Some(font_system)) = (
            self.single_session_streaming_text_start_line,
            self.single_session_streaming_text_end_line,
            self.font_system.as_mut(),
        ) else {
            return;
        };
        if start_line >= end_line || end_line > self.single_session_body_lines.len() {
            return;
        }

        let lines = self.single_session_body_lines[start_line..end_line].to_vec();
        self.single_session_streaming_text_buffer =
            Some(single_session_body_text_buffer_from_lines_with_opacity(
                font_system,
                &lines,
                render_size,
                app.text_scale(),
                quantized_opacity,
            ));
        self.single_session_streaming_text_opacity_bits = Some(opacity_bits);
        self.streaming_text_needs_prepare = true;
    }

    fn single_session_streaming_visible_range(
        &self,
        app: &SingleSessionApp,
        viewport: &SingleSessionBodyViewport,
    ) -> Option<(usize, usize)> {
        if app.streaming_response.is_empty() || self.single_session_streaming_base_len == 0 {
            return None;
        }
        let streaming_start_line = self
            .single_session_streaming_base_len
            .saturating_add(usize::from(!app.messages.is_empty()));
        let visible_start = viewport.start_line;
        let visible_end = viewport.start_line.saturating_add(viewport.lines.len());
        let start = streaming_start_line.max(visible_start);
        let end = self.single_session_body_lines.len().min(visible_end);
        (start < end).then_some((start, end))
    }

    fn sync_single_session_body_text_scroll(&mut self, start_line: usize, window_start: usize) {
        if self.single_session_body_text_scroll_start == Some(start_line) {
            return;
        }
        if let Some(body_buffer) = self.single_session_text_buffers.get_mut(1) {
            body_buffer.set_scroll(
                start_line
                    .saturating_sub(window_start)
                    .min(i32::MAX as usize) as i32,
            );
            self.single_session_body_text_scroll_start = Some(start_line);
            self.text_needs_prepare = true;
        }
    }

    fn ensure_font_system(&mut self) {
        if self.font_system.is_some() {
            return;
        }
        self.font_system = Some(
            self.font_system_loader
                .take()
                .and_then(|loader| match loader.join() {
                    Ok(font_system) => Some(font_system),
                    Err(_) => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: font system loader thread panicked"
                        ));
                        None
                    }
                })
                .unwrap_or_else(create_desktop_font_system),
        );
    }

    fn ensure_render_pipeline(&mut self) {
        if self.render_pipeline.is_some() {
            return;
        }
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("jcode-desktop-primitive-shader"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("jcode-desktop-primitive-pipeline-layout"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });
        self.render_pipeline = Some(self.device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("jcode-desktop-primitive-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[Vertex::layout()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.config.format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
            },
        ));
    }

    fn ensure_hero_mask_renderer(&mut self) {
        if self.hero_mask_renderer.is_some() {
            return;
        }
        self.hero_mask_renderer = Some(HeroMaskRenderer::new(&self.device, self.config.format));
    }

    fn ensure_text_renderer(&mut self) {
        if self.text_renderer.is_some() {
            return;
        }
        let mut text_atlas = TextAtlas::new(&self.device, &self.queue, self.config.format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &self.device,
            wgpu::MultisampleState::default(),
            None,
        );
        self.text_atlas = Some(text_atlas);
        self.text_renderer = Some(text_renderer);
        self.text_needs_prepare = true;
    }

    fn ensure_streaming_text_renderer(&mut self) {
        if self.streaming_text_renderer.is_some() {
            return;
        }
        let mut text_atlas = TextAtlas::new(&self.device, &self.queue, self.config.format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &self.device,
            wgpu::MultisampleState::default(),
            None,
        );
        self.streaming_text_atlas = Some(text_atlas);
        self.streaming_text_renderer = Some(text_renderer);
        self.streaming_text_needs_prepare = true;
    }

    fn release_streaming_text_renderer_if_idle(&mut self, has_streaming_text_buffer: bool) {
        if !streaming_text_renderer_should_release(
            has_streaming_text_buffer,
            self.streaming_text_renderer.is_some(),
            self.streaming_text_atlas.is_some(),
        ) {
            return;
        }

        self.streaming_text_renderer = None;
        self.streaming_text_atlas = None;
        self.streaming_text_needs_prepare = false;
    }

    fn cached_single_session_body_lines(
        &mut self,
        app: &SingleSessionApp,
        render_size: PhysicalSize<u32>,
        tick: u64,
    ) -> (u64, bool) {
        let body_layout_size = single_session_body_layout_cache_size(app, render_size);
        let key = app.rendered_body_cache_key(body_layout_size);
        if self.single_session_body_key == Some(key) {
            return (key, false);
        }

        if !app.streaming_response.is_empty() {
            self.single_session_raw_body_key = None;
            self.single_session_raw_body_lines.clear();
            let base_key = app.rendered_body_static_cache_key(body_layout_size);
            if self.single_session_streaming_base_key != Some(base_key) {
                if let Some(base_lines) =
                    single_session_rendered_static_body_lines_for_streaming(app, render_size, tick)
                {
                    self.single_session_body_lines = base_lines;
                    self.single_session_streaming_base_len = self.single_session_body_lines.len();
                    self.single_session_streaming_base_key = Some(base_key);
                    self.single_session_body_text_scroll_start = None;
                    self.single_session_body_text_window_start = None;
                    self.single_session_body_text_window_end = None;
                } else {
                    self.single_session_body_lines =
                        single_session_rendered_body_lines_for_tick(app, render_size, tick);
                    self.single_session_streaming_base_key = None;
                    self.single_session_streaming_base_len = 0;
                    self.single_session_body_key = Some(key);
                    self.single_session_body_text_scroll_start = None;
                    self.single_session_body_text_window_start = None;
                    self.single_session_body_text_window_end = None;
                    return (key, true);
                }
            } else {
                self.single_session_body_lines
                    .truncate(self.single_session_streaming_base_len);
            }
            append_single_session_streaming_response_rendered_body_lines(
                app,
                render_size,
                &mut self.single_session_body_lines,
            );
        } else {
            let raw_key = app.rendered_body_cache_key((0, 0));
            if self.single_session_raw_body_key != Some(raw_key) {
                self.single_session_raw_body_lines = app.body_styled_lines_for_tick(tick);
                self.single_session_raw_body_key = Some(raw_key);
            }
            self.single_session_body_lines = single_session_rendered_body_lines_from_raw_ref(
                app,
                render_size,
                &self.single_session_raw_body_lines,
            );
            self.single_session_streaming_base_key = None;
            self.single_session_streaming_base_len = 0;
            self.single_session_body_text_window_start = None;
            self.single_session_body_text_window_end = None;
        }
        self.single_session_body_key = Some(key);
        self.single_session_body_text_scroll_start = None;
        (key, true)
    }

    fn welcome_hero_reveal_progress(
        &mut self,
        app: &SingleSessionApp,
        now: Instant,
    ) -> (f32, bool) {
        if !app.is_welcome_timeline_visible() {
            self.welcome_hero_reveal_key = None;
            self.welcome_hero_reveal_started_at = None;
            return (1.0, false);
        }

        let key = app.welcome_hero_text();
        if self.welcome_hero_reveal_key.as_deref() != Some(key.as_str()) {
            self.welcome_hero_reveal_key = Some(key);
            self.welcome_hero_reveal_started_at = None;
        }

        let elapsed = self
            .welcome_hero_reveal_started_at
            .map(|started_at| now.saturating_duration_since(started_at))
            .unwrap_or_default();
        let progress = welcome_hero_reveal_progress_for_elapsed(elapsed);
        (progress, welcome_hero_reveal_is_active(progress))
    }

    fn render_boot_frame(&mut self) -> std::result::Result<DesktopRenderFrameResult, SurfaceError> {
        let mut frame_profile = DesktopFrameProfile::new();
        let frame = self.surface.get_current_texture()?;
        frame_profile.checkpoint("surface_acquire");
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-boot-frame"),
            });
        frame_profile.checkpoint("frame_setup");
        {
            let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-desktop-boot-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        frame_profile.checkpoint("render_pass");
        self.queue.submit(Some(encoder.finish()));
        frame_profile.checkpoint("queue_submit");
        frame.present();
        frame_profile.checkpoint("present");
        self.boot_frame_presented = true;
        let frame_wall = frame_profile.total_duration();
        let frame_cpu = frame_profile.cpu_duration();
        let context = DesktopFrameContext {
            mode: "boot",
            smooth_scroll_lines: 0.0,
            text_buffer_count: 0,
            text_area_count: 0,
            primitive_vertices: 0,
            body_line_count: 0,
            viewport_line_count: 0,
            body_text_window_line_count: 0,
            streaming_text_line_count: 0,
            inline_widget_line_count: 0,
            text_prepared: false,
            primitive_geometry_cache_hit: false,
        };
        let stages = frame_profile.stages.clone();
        self.frame_profiler.observe(frame_profile, context);
        Ok(DesktopRenderFrameResult {
            animation_active: true,
            content_ready: false,
            frame_wall,
            frame_cpu,
            context,
            stages,
        })
    }

    #[allow(dead_code)]
    fn render_scene(
        &mut self,
        scene: &DesktopScene,
    ) -> std::result::Result<DesktopRenderFrameResult, SurfaceError> {
        if !self.boot_frame_presented {
            return self.render_boot_frame();
        }

        let mut frame_profile = DesktopFrameProfile::new();
        let frame = self.surface.get_current_texture()?;
        frame_profile.checkpoint("surface_acquire");
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-scene-render"),
            });
        frame_profile.checkpoint("frame_setup");

        self.ensure_render_pipeline();
        frame_profile.checkpoint("primitive_pipeline");
        self.primitive_frame_vertices.clear();
        let clear_color =
            desktop_scene_vertices(scene, self.size, &mut self.primitive_frame_vertices)
                .unwrap_or(CLEAR_COLOR);
        let primitive_vertex_count = self.primitive_frame_vertices.len();
        upload_primitive_vertices(
            &self.device,
            &self.queue,
            &mut self.primitive_vertex_buffer,
            &mut self.primitive_vertex_capacity,
            &self.primitive_frame_vertices,
        );
        frame_profile.checkpoint("primitive_upload");

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-desktop-scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(render_pipeline) = self.render_pipeline.as_ref() {
                render_pass.set_pipeline(render_pipeline);
            }
            if primitive_vertex_count > 0
                && let Some(vertex_buffer) = self.primitive_vertex_buffer.as_ref()
            {
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.draw(0..primitive_vertex_count as u32, 0..1);
            }
        }
        frame_profile.checkpoint("render_pass");

        self.queue.submit(Some(encoder.finish()));
        frame_profile.checkpoint("queue_submit");
        frame.present();
        frame_profile.checkpoint("present");
        self.first_render_completed = true;
        let frame_wall = frame_profile.total_duration();
        let frame_cpu = frame_profile.cpu_duration();
        let context = DesktopFrameContext {
            mode: "scene",
            smooth_scroll_lines: 0.0,
            text_buffer_count: 0,
            text_area_count: 0,
            primitive_vertices: primitive_vertex_count,
            body_line_count: 0,
            viewport_line_count: 0,
            body_text_window_line_count: 0,
            streaming_text_line_count: 0,
            inline_widget_line_count: 0,
            text_prepared: false,
            primitive_geometry_cache_hit: false,
        };
        let stages = frame_profile.stages.clone();
        self.frame_profiler.observe(frame_profile, context);
        Ok(DesktopRenderFrameResult {
            animation_active: scene.metadata.animation_active,
            content_ready: scene.metadata.content_ready,
            frame_wall,
            frame_cpu,
            context,
            stages,
        })
    }

    fn render(
        &mut self,
        app: &DesktopApp,
        monitor_size: Option<PhysicalSize<u32>>,
        smooth_scroll_lines: f32,
        workspace_space_hold_progress: Option<f32>,
    ) -> std::result::Result<DesktopRenderFrameResult, SurfaceError> {
        if !self.boot_frame_presented {
            return self.render_boot_frame();
        }

        let mut frame_profile = DesktopFrameProfile::new();
        let frame = self.surface.get_current_texture()?;
        frame_profile.checkpoint("surface_acquire");
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-render-workspace"),
            });
        let now = Instant::now();
        let spinner_tick = desktop_spinner_tick(now);
        frame_profile.checkpoint("frame_setup");

        let scroll_motion_frame = if let DesktopApp::SingleSession(single_session) = app {
            self.single_session_scroll_motion
                .frame(single_session.body_scroll_lines, now)
        } else {
            self.single_session_scroll_motion.clear();
            SingleSessionScrollMotionFrame {
                visual_scroll_lines: 0.0,
                smooth_scroll_lines: 0.0,
                active: false,
            }
        };
        let smooth_scroll_lines = smooth_scroll_lines + scroll_motion_frame.smooth_scroll_lines;
        frame_profile.checkpoint("scroll_motion");

        let (welcome_hero_reveal_progress, welcome_hero_reveal_active) =
            if let DesktopApp::SingleSession(single_session) = app {
                self.welcome_hero_reveal_progress(single_session, now)
            } else {
                self.welcome_hero_reveal_key = None;
                self.welcome_hero_reveal_started_at = None;
                (1.0, false)
            };
        frame_profile.checkpoint("welcome_reveal");

        let (
            workspace_render_layout_for_frame,
            workspace_surface_frames_for_frame,
            workspace_status_color_for_frame,
            workspace_status_text_frame,
        ) = if let DesktopApp::Workspace(workspace) = app {
            let target_layout = workspace_render_layout(workspace, self.size, monitor_size);
            let render_layout = self.viewport_animation.frame(target_layout, now);
            let surface_targets =
                workspace_surface_transition_targets(workspace, self.size, render_layout);
            let surface_frames = WorkspaceSurfaceTransitionFrames::new(
                self.surface_transitions.frame(surface_targets, now),
                self.surface_transitions.is_animating(),
            );
            update_workspace_surface_exit_cache(
                &mut self.workspace_surface_exit_cache,
                workspace,
                &surface_frames,
            );
            let status_color = self
                .status_color_transition
                .frame(workspace_status_bar_target_color(workspace), now);
            let status_text_frame = self
                .status_text_transition
                .frame(workspace_status_text(workspace), now);
            (
                Some(render_layout),
                Some(surface_frames),
                Some(status_color),
                Some(status_text_frame),
            )
        } else {
            self.surface_transitions.clear();
            self.workspace_surface_exit_cache.clear();
            self.status_color_transition.clear();
            self.status_text_transition.clear();
            (None, None, None, None)
        };

        let single_session_issue_layout_for_frame =
            if let DesktopApp::SingleSession(single_session) = app {
                issue_browser_layout(single_session, self.size)
            } else {
                IssueBrowserLayout::hidden(self.size)
            };
        let single_session_render_size = single_session_issue_layout_for_frame.chat_size();

        let mut single_session_rendered_body_key = None;
        let mut workspace_text_panes = Vec::new();
        let mut workspace_text_pane_cache_for_frame = None;
        let mut body_line_count = 0usize;
        let mut viewport_line_count = 0usize;
        let mut body_text_window_line_count = 0usize;
        let mut streaming_text_line_count = 0usize;
        let mut inline_widget_line_count = 0usize;
        let defer_text_this_frame = self.defer_initial_text_frame;
        if defer_text_this_frame {
            self.defer_initial_text_frame = false;
            self.single_session_text_cache_key = None;
            self.single_session_text_buffers.clear();
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.single_session_streaming_handoff_started_at = None;
            self.streaming_text_needs_prepare = false;
            self.single_session_body_text_scroll_start = None;
            self.single_session_body_text_window_start = None;
            self.single_session_body_text_window_end = None;
        } else if let DesktopApp::SingleSession(single_session) = app {
            let (rendered_body_key, rendered_body_changed) = self.cached_single_session_body_lines(
                single_session,
                single_session_render_size,
                spinner_tick,
            );
            single_session_rendered_body_key = Some(rendered_body_key);
            body_line_count = self.single_session_body_lines.len();
            inline_widget_line_count = single_session.render_inline_widget_visible_line_count();
            frame_profile.checkpoint("body_lines_cache");
            self.ensure_font_system();
            frame_profile.checkpoint("font_system");
            self.refresh_cached_single_session_text_buffers(
                single_session,
                single_session_render_size,
                now,
                smooth_scroll_lines,
                rendered_body_key,
                rendered_body_changed,
            );
            frame_profile.checkpoint("text_buffers_cache");
        } else {
            self.single_session_text_cache_key = None;
            self.single_session_text_key = None;
            self.single_session_text_buffers.clear();
            self.tool_card_motion.clear();
            self.single_session_streaming_text_key = None;
            self.single_session_streaming_text_start_line = None;
            self.single_session_streaming_text_end_line = None;
            self.single_session_streaming_text_opacity_bits = None;
            self.single_session_streaming_text_buffer = None;
            self.single_session_streaming_handoff_started_at = None;
            self.streaming_text_needs_prepare = false;
            self.single_session_body_text_scroll_start = None;
            self.single_session_body_text_window_start = None;
            self.single_session_body_text_window_end = None;
            if let (DesktopApp::Workspace(workspace), Some(render_layout)) =
                (app, workspace_render_layout_for_frame)
            {
                self.ensure_font_system();
                if let Some(font_system) = self.font_system.as_mut() {
                    let cache = workspace_text_pane_cache_for_frame
                        .get_or_insert_with(|| std::mem::take(&mut self.workspace_text_pane_cache));
                    workspace_text_panes = build_workspace_single_session_text_panes(
                        cache,
                        workspace,
                        self.size,
                        render_layout,
                        workspace_surface_frames_for_frame.as_ref(),
                        font_system,
                    );
                    frame_profile.checkpoint("workspace_text_panes");
                    if !workspace_text_panes.is_empty() {
                        self.text_needs_prepare = true;
                    }
                }
            }
        }
        frame_profile.checkpoint("text_cache");
        if !self.single_session_text_buffers.is_empty() || !workspace_text_panes.is_empty() {
            self.ensure_text_renderer();
        }
        if self.single_session_streaming_text_buffer.is_some() {
            self.ensure_streaming_text_renderer();
        }
        frame_profile.checkpoint("text_renderer");
        self.ensure_render_pipeline();
        frame_profile.checkpoint("primitive_pipeline");
        let streaming_text_arrival_style = self.single_session_streaming_arrival_style(now);
        if let DesktopApp::SingleSession(single_session) = app {
            self.update_single_session_streaming_text_buffer_opacity(
                single_session,
                single_session_render_size,
                streaming_text_arrival_style.opacity,
            );
        }
        if streaming_text_arrival_style.active
            && self.single_session_streaming_text_buffer.is_some()
        {
            self.streaming_text_needs_prepare = true;
        }
        let has_streaming_text_buffer = self.single_session_streaming_text_buffer.is_some();
        self.release_streaming_text_renderer_if_idle(has_streaming_text_buffer);
        let text_buffers = &self.single_session_text_buffers;
        let has_text_buffers = !text_buffers.is_empty() || !workspace_text_panes.is_empty();
        let mut text_area_count = 0usize;
        let mut text_prepared = false;
        let single_session_viewport = if let DesktopApp::SingleSession(single_session) = app {
            let viewport = single_session_body_viewport_from_lines(
                single_session,
                single_session_render_size,
                smooth_scroll_lines,
                &self.single_session_body_lines,
            );
            viewport_line_count = viewport.lines.len();
            body_text_window_line_count = self
                .single_session_body_text_window_start
                .zip(self.single_session_body_text_window_end)
                .map(|(start, end)| end.saturating_sub(start))
                .unwrap_or_default();
            streaming_text_line_count = self
                .single_session_streaming_text_start_line
                .zip(self.single_session_streaming_text_end_line)
                .map(|(start, end)| end.saturating_sub(start))
                .unwrap_or_default();
            Some(viewport)
        } else {
            None
        };
        let welcome_hero_uses_runtime_mask = matches!(
            app,
            DesktopApp::SingleSession(single_session)
                if !single_session_issue_layout_for_frame.visible()
                    && welcome_hero_runtime_mask_supported(&single_session.welcome_hero_text())
        );
        if welcome_hero_reveal_active && !welcome_hero_uses_runtime_mask {
            self.text_needs_prepare = true;
        }
        if self.text_needs_prepare {
            let text_areas = if let DesktopApp::SingleSession(single_session) = app {
                if let Some(viewport) = single_session_viewport.clone() {
                    let areas =
                        single_session_text_areas_for_app_with_cached_body_viewport_and_reveal(
                            single_session,
                            text_buffers,
                            single_session_render_size,
                            smooth_scroll_lines,
                            viewport,
                            welcome_hero_reveal_progress,
                        );
                    if single_session_issue_layout_for_frame.visible() {
                        areas
                            .into_iter()
                            .filter_map(|area| {
                                let area = offset_workspace_text_area(
                                    area,
                                    single_session_issue_layout_for_frame.chat,
                                );
                                (area.bounds.right > area.bounds.left
                                    && area.bounds.bottom > area.bounds.top)
                                    .then_some(area)
                            })
                            .collect()
                    } else {
                        areas
                    }
                } else {
                    desktop_log::error(format_args!(
                        "jcode-desktop: missing single-session viewport while preparing text"
                    ));
                    Vec::new()
                }
            } else if !workspace_text_panes.is_empty() {
                workspace_single_session_text_areas(&workspace_text_panes)
            } else {
                single_session_text_areas(text_buffers, single_session_render_size)
            };
            text_area_count = text_areas.len();
            frame_profile.checkpoint("text_areas");
            if text_areas.is_empty() {
                self.text_needs_prepare = false;
            } else {
                match (
                    self.font_system.as_mut(),
                    self.text_atlas.as_mut(),
                    self.text_renderer.as_mut(),
                ) {
                    (Some(font_system), Some(text_atlas), Some(text_renderer)) => {
                        if let Err(error) = text_renderer.prepare(
                            &self.device,
                            &self.queue,
                            font_system,
                            text_atlas,
                            Resolution {
                                width: self.config.width,
                                height: self.config.height,
                            },
                            text_areas,
                            &mut self.swash_cache,
                        ) {
                            desktop_log::error(format_args!(
                                "jcode-desktop: failed to prepare text, recreating renderer: {error:?}"
                            ));
                            self.text_renderer = None;
                            self.text_atlas = None;
                            self.text_needs_prepare = true;
                        } else {
                            text_prepared = true;
                            self.text_needs_prepare = false;
                        }
                    }
                    _ => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: text renderer state was incomplete before prepare, recreating renderer"
                        ));
                        self.text_renderer = None;
                        self.text_atlas = None;
                        self.text_needs_prepare = true;
                    }
                }
            }
        } else {
            frame_profile.checkpoint("text_areas");
        }
        frame_profile.checkpoint("text_prepare_static");
        drop(workspace_text_panes);
        if let Some(cache) = workspace_text_pane_cache_for_frame.take() {
            self.workspace_text_pane_cache = cache;
        }
        if self.streaming_text_needs_prepare {
            let streaming_text_areas = if let (
                DesktopApp::SingleSession(single_session),
                Some(viewport),
                Some(buffer),
                Some(start_line),
            ) = (
                app,
                single_session_viewport.clone(),
                self.single_session_streaming_text_buffer.as_ref(),
                self.single_session_streaming_text_start_line,
            ) {
                let area = single_session_streaming_text_area_for_cached_body_viewport(
                    single_session,
                    buffer,
                    single_session_render_size,
                    viewport,
                    start_line,
                    streaming_text_arrival_style.opacity,
                    streaming_text_arrival_style.y_offset_pixels,
                );
                if single_session_issue_layout_for_frame.visible() {
                    let area = offset_workspace_text_area(
                        area,
                        single_session_issue_layout_for_frame.chat,
                    );
                    (area.bounds.right > area.bounds.left && area.bounds.bottom > area.bounds.top)
                        .then_some(area)
                        .into_iter()
                        .collect()
                } else {
                    vec![area]
                }
            } else {
                Vec::new()
            };
            text_area_count += streaming_text_areas.len();
            if streaming_text_areas.is_empty() {
                self.streaming_text_needs_prepare = false;
            } else {
                match (
                    self.font_system.as_mut(),
                    self.streaming_text_atlas.as_mut(),
                    self.streaming_text_renderer.as_mut(),
                ) {
                    (Some(font_system), Some(text_atlas), Some(text_renderer)) => {
                        if let Err(error) = text_renderer.prepare(
                            &self.device,
                            &self.queue,
                            font_system,
                            text_atlas,
                            Resolution {
                                width: self.config.width,
                                height: self.config.height,
                            },
                            streaming_text_areas,
                            &mut self.swash_cache,
                        ) {
                            desktop_log::error(format_args!(
                                "jcode-desktop: failed to prepare streaming text, recreating renderer: {error:?}"
                            ));
                            self.streaming_text_renderer = None;
                            self.streaming_text_atlas = None;
                            self.streaming_text_needs_prepare = true;
                        } else {
                            text_prepared = true;
                            self.streaming_text_needs_prepare = false;
                        }
                    }
                    _ => {
                        desktop_log::error(format_args!(
                            "jcode-desktop: streaming text renderer state was incomplete before prepare, recreating renderer"
                        ));
                        self.streaming_text_renderer = None;
                        self.streaming_text_atlas = None;
                        self.streaming_text_needs_prepare = true;
                    }
                }
            }
        }
        frame_profile.checkpoint("text_prepare_streaming");

        let mut primitive_geometry_cache_hit = false;
        let (mut vertices, mut animation_active): (Cow<'_, [Vertex]>, bool) = match app {
            DesktopApp::SingleSession(single_session) => {
                let focus_pulse = self.focus_pulse.frame(1, now);
                let inline_selection_motion = self
                    .inline_widget_selection_motion
                    .frame(single_session, now);
                let inline_list_reflow_motion = self
                    .inline_widget_list_reflow_motion
                    .frame(single_session, now);
                let inline_preview_pane_motion = self
                    .inline_widget_preview_pane_motion
                    .frame(single_session, now);
                let composer_motion = self.composer_motion.frame(single_session, now);
                let attachment_chip_motion = self.attachment_chip_motion.frame(single_session, now);
                let stdin_overlay_motion = self.stdin_overlay_motion.frame(
                    single_session,
                    &self.single_session_body_lines,
                    now,
                );
                let tool_motion_lines = single_session_viewport
                    .as_ref()
                    .map(|viewport| viewport.lines.as_slice())
                    .unwrap_or(self.single_session_body_lines.as_slice());
                let transcript_line_height = {
                    let typography =
                        single_session_typography_for_scale(single_session.text_scale());
                    typography.body_size * typography.body_line_height
                };
                let transcript_motion = self.transcript_card_motion.frame(
                    tool_motion_lines,
                    transcript_line_height,
                    now,
                );
                let transcript_message_motion = self.transcript_message_motion.frame(
                    tool_motion_lines,
                    transcript_line_height,
                    now,
                );
                let inline_markdown_motion = self.inline_markdown_pill_motion.frame(
                    tool_motion_lines,
                    transcript_line_height,
                    now,
                );
                let activity_cue_motion = self
                    .streaming_activity_cue_motion
                    .frame(single_session, now);
                let tool_motion = self
                    .tool_card_motion
                    .frame(tool_motion_lines, now, spinner_tick);
                let scrollbar_motion = self.single_session_scrollbar_motion.frame(
                    single_session,
                    single_session_render_size,
                    self.single_session_body_lines.len(),
                    smooth_scroll_lines,
                    now,
                );
                frame_profile.checkpoint("vertices_tool_motion");
                let animation_active = self.focus_pulse.is_animating()
                    || single_session.has_background_work()
                    || inline_selection_motion.is_active()
                    || inline_list_reflow_motion.is_active()
                    || inline_preview_pane_motion.is_active()
                    || composer_motion.is_active()
                    || attachment_chip_motion.is_active()
                    || stdin_overlay_motion.is_active()
                    || transcript_message_motion.is_active()
                    || transcript_motion.is_active()
                    || inline_markdown_motion.is_active()
                    || activity_cue_motion.is_active()
                    || tool_motion.is_active()
                    || scrollbar_motion.is_active()
                    || scroll_motion_frame.active
                    || welcome_hero_reveal_active
                    || streaming_text_arrival_style.active;
                let geometry_cache_key = if single_session_issue_layout_for_frame.visible() {
                    None
                } else {
                    single_session_streaming_primitive_geometry_cache_key(
                        single_session,
                        single_session_render_size,
                        focus_pulse,
                        spinner_tick,
                        smooth_scroll_lines,
                        welcome_hero_reveal_progress,
                        tool_motion.cache_key(),
                        inline_list_reflow_motion.cache_key(),
                        inline_preview_pane_motion.cache_key(),
                        composer_motion.cache_key(),
                        attachment_chip_motion.cache_key(),
                        stdin_overlay_motion.cache_key(),
                        transcript_message_motion.cache_key(),
                        transcript_motion.cache_key(),
                        inline_markdown_motion.cache_key(),
                        activity_cue_motion.cache_key(),
                        scrollbar_motion.cache_key(),
                        single_session_rendered_body_key,
                        self.single_session_body_lines.len(),
                    )
                };
                let child_vertices = if let Some(cache_key) = geometry_cache_key {
                    if self.primitive_vertices_cache_key == Some(cache_key) {
                        primitive_geometry_cache_hit = true;
                        Cow::Borrowed(self.primitive_vertices_cache.as_slice())
                    } else {
                        let vertices =
                            build_single_session_vertices_with_cached_body_and_tool_motion(
                                single_session,
                                single_session_render_size,
                                focus_pulse,
                                spinner_tick,
                                smooth_scroll_lines,
                                welcome_hero_reveal_progress,
                                &self.single_session_body_lines,
                                Some(&inline_selection_motion),
                                Some(&inline_list_reflow_motion),
                                Some(&inline_preview_pane_motion),
                                Some(&composer_motion),
                                Some(&attachment_chip_motion),
                                Some(&stdin_overlay_motion),
                                Some(&transcript_message_motion),
                                Some(&transcript_motion),
                                Some(&inline_markdown_motion),
                                Some(&activity_cue_motion),
                                &tool_motion,
                                Some(&scrollbar_motion),
                            );
                        self.primitive_vertices_cache_key = Some(cache_key);
                        self.primitive_vertices_cache = vertices;
                        Cow::Borrowed(self.primitive_vertices_cache.as_slice())
                    }
                } else {
                    self.primitive_vertices_cache_key = None;
                    Cow::Owned(
                        build_single_session_vertices_with_cached_body_and_tool_motion(
                            single_session,
                            single_session_render_size,
                            focus_pulse,
                            spinner_tick,
                            smooth_scroll_lines,
                            welcome_hero_reveal_progress,
                            &self.single_session_body_lines,
                            Some(&inline_selection_motion),
                            Some(&inline_list_reflow_motion),
                            Some(&inline_preview_pane_motion),
                            Some(&composer_motion),
                            Some(&attachment_chip_motion),
                            Some(&stdin_overlay_motion),
                            Some(&transcript_message_motion),
                            Some(&transcript_motion),
                            Some(&inline_markdown_motion),
                            Some(&activity_cue_motion),
                            &tool_motion,
                            Some(&scrollbar_motion),
                        ),
                    )
                };
                let vertices = if single_session_issue_layout_for_frame.visible() {
                    Cow::Owned(compose_single_session_issue_browser_vertices(
                        single_session,
                        single_session_issue_layout_for_frame,
                        child_vertices.as_ref(),
                        single_session_render_size,
                        self.size,
                    ))
                } else {
                    child_vertices
                };
                frame_profile.checkpoint("vertices_geometry");
                (vertices, animation_active)
            }
            DesktopApp::Workspace(workspace) => {
                self.inline_widget_selection_motion.clear();
                self.inline_widget_list_reflow_motion.clear();
                self.inline_widget_preview_pane_motion.clear();
                self.composer_motion.clear();
                self.attachment_chip_motion.clear();
                self.stdin_overlay_motion.clear();
                self.transcript_message_motion.clear();
                self.transcript_card_motion.clear();
                self.inline_markdown_pill_motion.clear();
                self.streaming_activity_cue_motion.clear();
                self.single_session_scrollbar_motion.clear();
                self.primitive_vertices_cache_key = None;
                let render_layout = workspace_render_layout_for_frame
                    .unwrap_or_else(|| workspace_render_layout(workspace, self.size, monitor_size));
                let focus_pulse = self.focus_pulse.frame(workspace.focused_id, now);
                let surface_transition_active = workspace_surface_frames_for_frame
                    .as_ref()
                    .is_some_and(|frames| frames.animating);
                let status_text_active = workspace_status_text_frame
                    .as_ref()
                    .is_some_and(StatusTextTransitionFrame::is_active);
                let animation_active = self.viewport_animation.is_animating()
                    || self.focus_pulse.is_animating()
                    || surface_transition_active
                    || self.status_color_transition.is_animating()
                    || status_text_active;
                reserve_workspace_vertex_capacity(
                    &mut self.primitive_workspace_vertices,
                    workspace,
                );
                let status_color = workspace_status_color_for_frame
                    .unwrap_or_else(|| workspace_status_bar_target_color(workspace));
                let status_text_frame = workspace_status_text_frame.as_ref();
                build_vertices_into(
                    WorkspaceVertexBuildParams {
                        workspace,
                        size: self.size,
                        render_layout,
                        focus_pulse,
                        space_hold_progress: workspace_space_hold_progress,
                        surface_frames: workspace_surface_frames_for_frame.as_ref(),
                        exiting_surfaces: &self.workspace_surface_exit_cache,
                        workspace_panel_cache: Some(&self.workspace_text_pane_cache),
                        status_color,
                        status_text_frame,
                    },
                    &mut self.primitive_workspace_vertices,
                );
                frame_profile.checkpoint("vertices_geometry");
                (
                    Cow::Borrowed(self.primitive_workspace_vertices.as_slice()),
                    animation_active,
                )
            }
        };
        frame_profile.checkpoint("vertices");
        if let DesktopApp::SingleSession(single_session) = app
            && single_session_caret_visible_for_frame(single_session, spinner_tick)
        {
            if single_session_issue_layout_for_frame.visible() {
                let mut caret_vertices = Vec::new();
                push_single_session_caret(
                    &mut caret_vertices,
                    single_session,
                    single_session_render_size,
                    text_buffers.get(2),
                );
                append_child_vertices_to_parent_with_opacity(
                    vertices.to_mut(),
                    &caret_vertices,
                    single_session_render_size,
                    single_session_issue_layout_for_frame.chat,
                    self.size,
                    1.0,
                );
            } else {
                push_single_session_caret(
                    vertices.to_mut(),
                    single_session,
                    single_session_render_size,
                    text_buffers.get(2),
                );
            }
        }
        frame_profile.checkpoint("caret");
        if let Some(mode_transition_frame) = self.app_mode_transition.frame(app.mode(), now) {
            let previous_vertices = self.app_mode_transition.previous_vertices().to_vec();
            let current_vertices = vertices.as_ref().to_vec();
            compose_app_mode_transition_vertices(
                &mut self.app_mode_transition_vertices,
                &previous_vertices,
                &current_vertices,
                mode_transition_frame,
            );
            vertices = Cow::Borrowed(self.app_mode_transition_vertices.as_slice());
            animation_active = true;
        }
        let uploaded_vertices_snapshot = vertices.as_ref().to_vec();
        self.app_mode_transition
            .remember_uploaded_vertices(&uploaded_vertices_snapshot);
        frame_profile.checkpoint("mode_transition");
        let primitive_vertex_count = vertices.len();
        upload_primitive_vertices(
            &self.device,
            &self.queue,
            &mut self.primitive_vertex_buffer,
            &mut self.primitive_vertex_capacity,
            vertices.as_ref(),
        );
        frame_profile.checkpoint("primitive_upload");

        let welcome_hero_runtime_mask_visible = matches!(
            app,
            DesktopApp::SingleSession(single_session)
                if single_session.is_welcome_timeline_visible() && welcome_hero_uses_runtime_mask
        );
        let defer_hero_mask_this_frame =
            !self.first_render_completed && welcome_hero_runtime_mask_visible;
        let hero_mask_spec = if welcome_hero_runtime_mask_visible
            && !defer_hero_mask_this_frame
            && let DesktopApp::SingleSession(single_session) = app
        {
            welcome_hero_runtime_mask_spec_for_total_lines(
                single_session,
                self.size,
                smooth_scroll_lines,
                self.single_session_body_lines.len(),
            )
        } else {
            None
        };
        if hero_mask_spec.is_some() {
            self.ensure_hero_mask_renderer();
        }
        let hero_mask_prepared = self.hero_mask_renderer.as_mut().is_some_and(|renderer| {
            renderer.prepare(
                &self.device,
                &self.queue,
                self.size,
                hero_mask_spec.as_ref(),
                welcome_hero_reveal_progress,
            )
        });
        frame_profile.checkpoint("hero_mask_prepare");

        let mut text_render_failed = false;
        let mut streaming_text_render_failed = false;
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-desktop-workspace-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(render_pipeline) = self.render_pipeline.as_ref() {
                render_pass.set_pipeline(render_pipeline);
            }
            if let Some(vertex_buffer) = self.primitive_vertex_buffer.as_ref() {
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.draw(0..primitive_vertex_count as u32, 0..1);
            }
            if hero_mask_prepared && let Some(hero_mask_renderer) = self.hero_mask_renderer.as_ref()
            {
                hero_mask_renderer.render_prepared(&mut render_pass);
            }
            if has_text_buffers
                && let (Some(text_renderer), Some(text_atlas)) =
                    (self.text_renderer.as_mut(), self.text_atlas.as_ref())
                && let Err(error) = text_renderer.render(text_atlas, &mut render_pass)
            {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to render text, recreating renderer: {error:?}"
                ));
                text_render_failed = true;
            }
            if has_streaming_text_buffer
                && let (Some(text_renderer), Some(text_atlas)) = (
                    self.streaming_text_renderer.as_mut(),
                    self.streaming_text_atlas.as_ref(),
                )
                && let Err(error) = text_renderer.render(text_atlas, &mut render_pass)
            {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to render streaming text, recreating renderer: {error:?}"
                ));
                streaming_text_render_failed = true;
            }
        }
        if text_render_failed {
            self.text_renderer = None;
            self.text_atlas = None;
            self.text_needs_prepare = true;
        }
        if streaming_text_render_failed {
            self.streaming_text_renderer = None;
            self.streaming_text_atlas = None;
            self.streaming_text_needs_prepare = true;
        }
        frame_profile.checkpoint("render_pass");

        self.queue.submit(Some(encoder.finish()));
        frame_profile.checkpoint("queue_submit");
        frame.present();
        frame_profile.checkpoint("present");
        if welcome_hero_runtime_mask_visible
            && self.welcome_hero_reveal_started_at.is_none()
            && (!welcome_hero_uses_runtime_mask || hero_mask_prepared)
        {
            self.welcome_hero_reveal_started_at = Some(Instant::now());
        }
        self.first_render_completed = true;
        let frame_wall = frame_profile.total_duration();
        let frame_cpu = frame_profile.cpu_duration();
        let context = DesktopFrameContext {
            mode: match app {
                DesktopApp::SingleSession(_) => "single_session",
                DesktopApp::Workspace(_) => "workspace",
            },
            smooth_scroll_lines,
            text_buffer_count: self.single_session_text_buffers.len()
                + usize::from(self.single_session_streaming_text_buffer.is_some()),
            text_area_count,
            primitive_vertices: primitive_vertex_count,
            body_line_count,
            viewport_line_count,
            body_text_window_line_count,
            streaming_text_line_count,
            inline_widget_line_count,
            text_prepared,
            primitive_geometry_cache_hit,
        };
        let stages = frame_profile.stages.clone();
        self.frame_profiler.observe(frame_profile, context);
        Ok(DesktopRenderFrameResult {
            animation_active: animation_active
                || defer_text_this_frame
                || defer_hero_mask_this_frame,
            content_ready: text_prepared && !defer_text_this_frame && !defer_hero_mask_this_frame,
            frame_wall,
            frame_cpu,
            context,
            stages,
        })
    }
}

fn upload_primitive_vertices(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    primitive_vertex_buffer: &mut Option<wgpu::Buffer>,
    primitive_vertex_capacity: &mut usize,
    vertices: &[Vertex],
) {
    if vertices.is_empty() {
        return;
    }

    if primitive_vertex_buffer_should_reallocate(*primitive_vertex_capacity, vertices.len()) {
        *primitive_vertex_capacity = primitive_vertex_capacity_for_len(vertices.len());
        let size =
            (*primitive_vertex_capacity * std::mem::size_of::<Vertex>()) as wgpu::BufferAddress;
        *primitive_vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("jcode-desktop-workspace-vertices"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }

    if let Some(vertex_buffer) = primitive_vertex_buffer.as_ref() {
        queue.write_buffer(vertex_buffer, 0, bytemuck::cast_slice(vertices));
    }
}

fn desktop_scene_vertices(
    scene: &DesktopScene,
    size: PhysicalSize<u32>,
    vertices: &mut Vec<Vertex>,
) -> Option<wgpu::Color> {
    vertices.clear();
    let mut clear_color = None;
    for command in &scene.display_list.commands {
        match command {
            DesktopDisplayCommand::Clear(color) => {
                clear_color = Some(desktop_scene_clear_color(*color));
                vertices.clear();
            }
            DesktopDisplayCommand::Rect(paint) => {
                push_desktop_scene_rect(vertices, paint, size);
            }
            DesktopDisplayCommand::Text(_)
            | DesktopDisplayCommand::Image(_)
            | DesktopDisplayCommand::PushClip(_)
            | DesktopDisplayCommand::PopClip
            | DesktopDisplayCommand::PushLayer { .. }
            | DesktopDisplayCommand::PopLayer => {}
        }
    }
    clear_color
}

fn push_desktop_scene_rect(
    vertices: &mut Vec<Vertex>,
    paint: &DesktopRectPaint,
    size: PhysicalSize<u32>,
) {
    if !paint.rect.is_renderable() || paint.fill.a <= 0.0 {
        return;
    }
    let rect = rect_from_desktop_scene_rect(paint.rect);
    let fill = paint.fill.to_array();
    let radius = [
        paint.radii.top_left,
        paint.radii.top_right,
        paint.radii.bottom_right,
        paint.radii.bottom_left,
    ]
    .into_iter()
    .fold(0.0_f32, f32::max);
    if radius > 0.5 {
        push_rounded_rect(vertices, rect, radius, fill, size);
    } else {
        push_rect(vertices, rect, fill, size);
    }
    if let Some(border) = paint.border
        && border.width > 0.0
        && border.color.a > 0.0
    {
        push_stroked_rect(vertices, rect, border.width, border.color.to_array(), size);
    }
}

fn rect_from_desktop_scene_rect(rect: DesktopSceneRect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn desktop_scene_clear_color(color: DesktopColor) -> wgpu::Color {
    wgpu::Color {
        r: color.r as f64,
        g: color.g as f64,
        b: color.b as f64,
        a: color.a as f64,
    }
}

fn primitive_vertex_capacity_for_len(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        len.next_power_of_two()
            .max(PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY)
    }
}

fn primitive_vertex_buffer_should_reallocate(capacity: usize, len: usize) -> bool {
    if len == 0 {
        false
    } else if capacity < len {
        true
    } else {
        capacity > PRIMITIVE_VERTEX_BUFFER_MIN_CAPACITY
            && len.saturating_mul(PRIMITIVE_VERTEX_BUFFER_SHRINK_RATIO) < capacity
    }
}

fn streaming_text_renderer_should_release(
    has_streaming_text_buffer: bool,
    renderer_live: bool,
    atlas_live: bool,
) -> bool {
    !has_streaming_text_buffer && (renderer_live || atlas_live)
}

fn single_session_caret_visible_for_frame(app: &SingleSessionApp, spinner_tick: u64) -> bool {
    spinner_tick % 6 < 3 && app.should_draw_composer_caret()
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

const HERO_MASK_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const HERO_MASK_FEATHER: f32 = 0.026;
const HERO_MASK_MAX_TEXTURE_WIDTH: u32 = 2048;
const HERO_MASK_MAX_TEXTURE_HEIGHT: u32 = 512;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct HeroMaskVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

impl HeroMaskVertex {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<HeroMaskVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct HeroRevealUniform {
    progress: f32,
    feather: f32,
    padding: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HeroMaskKey {
    phrase: String,
    width: u32,
    height: u32,
    font_size_milli: u32,
}

struct HeroMaskImage {
    width: u32,
    height: u32,
    glyph_rgba: Vec<u8>,
    reveal_rgba: Vec<u8>,
}

struct HeroMaskResources {
    key: HeroMaskKey,
    bind_group: wgpu::BindGroup,
    _glyph_texture: wgpu::Texture,
    _glyph_view: wgpu::TextureView,
    _reveal_texture: wgpu::Texture,
    _reveal_view: wgpu::TextureView,
}

struct HeroMaskRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    resources: Option<HeroMaskResources>,
    prepared: bool,
}

impl HeroMaskRenderer {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("jcode-desktop-hero-mask-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("jcode-desktop-hero-mask-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("jcode-desktop-hero-mask-shader"),
            source: wgpu::ShaderSource::Wgsl(HERO_MASK_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("jcode-desktop-hero-mask-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[HeroMaskVertex::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("jcode-desktop-hero-mask-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("jcode-desktop-hero-mask-vertices"),
            size: (6 * std::mem::size_of::<HeroMaskVertex>()) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("jcode-desktop-hero-mask-uniform"),
            size: std::mem::size_of::<HeroRevealUniform>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            vertex_buffer,
            uniform_buffer,
            resources: None,
            prepared: false,
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_size: PhysicalSize<u32>,
        spec: Option<&WelcomeHeroRuntimeMaskSpec>,
        progress: f32,
    ) -> bool {
        self.prepared = false;
        let Some(spec) = spec else {
            return false;
        };
        if target_size.width == 0 || target_size.height == 0 {
            return false;
        }
        let width = (spec.rect.width.ceil() as u32).clamp(1, HERO_MASK_MAX_TEXTURE_WIDTH);
        let height = (spec.rect.height.ceil() as u32).clamp(1, HERO_MASK_MAX_TEXTURE_HEIGHT);
        let key = HeroMaskKey {
            phrase: spec.phrase.clone(),
            width,
            height,
            font_size_milli: (spec.font_size.max(1.0) * 1000.0).round() as u32,
        };

        if self.resources.as_ref().map(|resources| &resources.key) != Some(&key) {
            let Some(mask) = build_hero_mask_image(&spec.phrase, width, height, spec.font_size)
            else {
                self.resources = None;
                return false;
            };
            self.resources = Some(self.create_resources(device, queue, key, mask));
        }

        let vertices = hero_mask_quad_vertices(spec.rect, target_size);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        let uniform = HeroRevealUniform {
            progress: progress.clamp(0.0, 1.0),
            feather: HERO_MASK_FEATHER,
            padding: [0.0, 0.0],
            color: WELCOME_HANDWRITING_COLOR,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        self.prepared = true;
        true
    }

    fn create_resources(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: HeroMaskKey,
        mask: HeroMaskImage,
    ) -> HeroMaskResources {
        let glyph_texture = create_hero_mask_texture(
            device,
            queue,
            "jcode-desktop-hero-alpha-texture",
            mask.width,
            mask.height,
            &mask.glyph_rgba,
        );
        let reveal_texture = create_hero_mask_texture(
            device,
            queue,
            "jcode-desktop-hero-reveal-texture",
            mask.width,
            mask.height,
            &mask.reveal_rgba,
        );
        let glyph_view = glyph_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let reveal_view = reveal_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("jcode-desktop-hero-mask-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&glyph_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&reveal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        HeroMaskResources {
            key,
            bind_group,
            _glyph_texture: glyph_texture,
            _glyph_view: glyph_view,
            _reveal_texture: reveal_texture,
            _reveal_view: reveal_view,
        }
    }

    fn render_prepared<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if !self.prepared {
            return;
        }
        let Some(resources) = self.resources.as_ref() else {
            return;
        };
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.draw(0..6, 0..1);
    }
}

fn create_hero_mask_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HERO_MASK_TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    texture
}

fn hero_mask_quad_vertices(rect: Rect, target_size: PhysicalSize<u32>) -> [HeroMaskVertex; 6] {
    let target_width = target_size.width.max(1) as f32;
    let target_height = target_size.height.max(1) as f32;
    let left = rect.x / target_width * 2.0 - 1.0;
    let right = (rect.x + rect.width) / target_width * 2.0 - 1.0;
    let top = 1.0 - rect.y / target_height * 2.0;
    let bottom = 1.0 - (rect.y + rect.height) / target_height * 2.0;
    [
        HeroMaskVertex {
            position: [left, top],
            uv: [0.0, 0.0],
        },
        HeroMaskVertex {
            position: [left, bottom],
            uv: [0.0, 1.0],
        },
        HeroMaskVertex {
            position: [right, bottom],
            uv: [1.0, 1.0],
        },
        HeroMaskVertex {
            position: [left, top],
            uv: [0.0, 0.0],
        },
        HeroMaskVertex {
            position: [right, bottom],
            uv: [1.0, 1.0],
        },
        HeroMaskVertex {
            position: [right, top],
            uv: [1.0, 0.0],
        },
    ]
}

#[derive(Clone, Copy, Debug)]
struct HeroMaskPixelBounds {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl HeroMaskPixelBounds {
    fn width(self) -> f32 {
        self.max_x.saturating_sub(self.min_x).max(1) as f32
    }

    fn height(self) -> f32 {
        self.max_y.saturating_sub(self.min_y).max(1) as f32
    }
}

fn build_hero_mask_image(
    phrase: &str,
    width: u32,
    height: u32,
    requested_font_size: f32,
) -> Option<HeroMaskImage> {
    let font = hero_handwriting_font()?;
    let mut font_size = requested_font_size.max(1.0);
    let mut glyphs = layout_hero_glyphs(font, phrase, font_size, point(0.0, 0.0));
    let mut bounds = hero_glyph_bounds(font, &glyphs)?;
    let target_width = width as f32 * 0.92;
    let target_height = height as f32 * 0.78;
    let glyph_width = (bounds.2 - bounds.0).max(1.0);
    let glyph_height = (bounds.3 - bounds.1).max(1.0);
    let fit = (target_width / glyph_width)
        .min(target_height / glyph_height)
        .min(1.0);
    if fit < 0.995 {
        font_size *= fit;
        glyphs = layout_hero_glyphs(font, phrase, font_size, point(0.0, 0.0));
        bounds = hero_glyph_bounds(font, &glyphs)?;
    }

    let glyph_width = (bounds.2 - bounds.0).max(1.0);
    let glyph_height = (bounds.3 - bounds.1).max(1.0);
    let origin = point(
        (width as f32 - glyph_width) * 0.5 - bounds.0,
        (height as f32 - glyph_height) * 0.48 - bounds.1,
    );
    let glyphs = layout_hero_glyphs(font, phrase, font_size, origin);
    let mut glyph_rgba = vec![0_u8; (width * height * 4) as usize];
    draw_hero_glyphs(font, &glyphs, width, height, &mut glyph_rgba);

    let alpha_bounds = hero_alpha_bounds(&glyph_rgba, width, height)?;
    let reveal_rgba = build_hero_reveal_texture(phrase, width, height, &glyph_rgba, alpha_bounds)?;
    Some(HeroMaskImage {
        width,
        height,
        glyph_rgba,
        reveal_rgba,
    })
}

fn hero_handwriting_font() -> Option<&'static FontArc> {
    static HERO_FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    HERO_FONT
        .get_or_init(|| {
            FontArc::try_from_slice(include_bytes!("../assets/fonts/HomemadeApple-Regular.ttf"))
                .ok()
        })
        .as_ref()
}

fn layout_hero_glyphs(
    font: &FontArc,
    phrase: &str,
    font_size: f32,
    origin: ab_glyph::Point,
) -> Vec<AbGlyph> {
    let scale = PxScale::from(font_size);
    let scaled = font.as_scaled(scale);
    let mut caret_x = origin.x;
    let mut previous = None;
    let mut glyphs = Vec::new();
    for ch in phrase.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(previous) = previous {
            caret_x += scaled.kern(previous, id);
        }
        glyphs.push(id.with_scale_and_position(scale, point(caret_x, origin.y)));
        caret_x += scaled.h_advance(id);
        previous = Some(id);
    }
    glyphs
}

fn hero_glyph_bounds(font: &FontArc, glyphs: &[AbGlyph]) -> Option<(f32, f32, f32, f32)> {
    let mut bounds = None::<(f32, f32, f32, f32)>;
    for glyph in glyphs.iter().cloned() {
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let px = outlined.px_bounds();
        bounds = Some(match bounds {
            Some((min_x, min_y, max_x, max_y)) => (
                min_x.min(px.min.x),
                min_y.min(px.min.y),
                max_x.max(px.max.x),
                max_y.max(px.max.y),
            ),
            None => (px.min.x, px.min.y, px.max.x, px.max.y),
        });
    }
    bounds
}

fn draw_hero_glyphs(
    font: &FontArc,
    glyphs: &[AbGlyph],
    width: u32,
    height: u32,
    glyph_rgba: &mut [u8],
) {
    for glyph in glyphs.iter().cloned() {
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outlined.px_bounds();
        let min_x = bounds.min.x as i32;
        let min_y = bounds.min.y as i32;
        outlined.draw(|x, y, coverage| {
            let px = min_x + x as i32;
            let py = min_y + y as i32;
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                return;
            }
            let alpha = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
            let index = ((py as u32 * width + px as u32) * 4) as usize;
            if alpha > glyph_rgba[index] {
                glyph_rgba[index] = alpha;
                glyph_rgba[index + 1] = alpha;
                glyph_rgba[index + 2] = alpha;
                glyph_rgba[index + 3] = 255;
            }
        });
    }
}

fn hero_alpha_bounds(glyph_rgba: &[u8], width: u32, height: u32) -> Option<HeroMaskPixelBounds> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    for y in 0..height {
        for x in 0..width {
            let alpha = glyph_rgba[((y * width + x) * 4) as usize];
            if alpha <= 2 {
                continue;
            }
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x + 1);
            max_y = max_y.max(y + 1);
        }
    }
    (min_x < max_x && min_y < max_y).then_some(HeroMaskPixelBounds {
        min_x,
        min_y,
        max_x,
        max_y,
    })
}

fn build_hero_reveal_texture(
    phrase: &str,
    width: u32,
    height: u32,
    glyph_rgba: &[u8],
    alpha_bounds: HeroMaskPixelBounds,
) -> Option<Vec<u8>> {
    let segments = welcome_hero_normalized_stroke_segments(phrase);
    if segments.is_empty() {
        return None;
    }

    let mut values = vec![1.0_f32; (width * height) as usize];
    let mut min_value = f32::INFINITY;
    let mut max_value = 0.0_f32;
    let brush_delay_px = (alpha_bounds.height() * 0.10).max(5.0);

    for y in 0..height {
        for x in 0..width {
            let pixel_index = (y * width + x) as usize;
            let alpha = glyph_rgba[pixel_index * 4];
            if alpha <= 2 {
                continue;
            }
            let (path_progress, distance) = nearest_hero_stroke_progress(
                x as f32 + 0.5,
                y as f32 + 0.5,
                alpha_bounds,
                &segments,
            );
            let width_delay = (distance / brush_delay_px).min(1.0) * 0.045;
            let value = (path_progress + width_delay).clamp(0.0, 1.0);
            values[pixel_index] = value;
            min_value = min_value.min(value);
            max_value = max_value.max(value);
        }
    }

    if !min_value.is_finite() || max_value <= min_value {
        return None;
    }

    let mut reveal_rgba = vec![255_u8; (width * height * 4) as usize];
    let scale = 0.970 / (max_value - min_value).max(0.001);
    for y in 0..height {
        for x in 0..width {
            let pixel_index = (y * width + x) as usize;
            let alpha = glyph_rgba[pixel_index * 4];
            if alpha <= 2 {
                continue;
            }
            let normalized = 0.006 + (values[pixel_index] - min_value) * scale;
            let value = normalized.clamp(0.0, 0.985);
            let encoded = (value * 255.0).round() as u8;
            let rgba_index = pixel_index * 4;
            reveal_rgba[rgba_index] = encoded;
            reveal_rgba[rgba_index + 1] = encoded;
            reveal_rgba[rgba_index + 2] = encoded;
            reveal_rgba[rgba_index + 3] = 255;
        }
    }
    Some(reveal_rgba)
}

fn nearest_hero_stroke_progress(
    x: f32,
    y: f32,
    alpha_bounds: HeroMaskPixelBounds,
    segments: &[WelcomeHeroStrokeSegment],
) -> (f32, f32) {
    let bounds_width = alpha_bounds.width();
    let bounds_height = alpha_bounds.height();
    let origin_x = alpha_bounds.min_x as f32;
    let origin_y = alpha_bounds.min_y as f32;
    let mut best_distance_sq = f32::INFINITY;
    let mut best_progress = 0.0;

    for segment in segments {
        let ax = origin_x + segment.start[0] * bounds_width;
        let ay = origin_y + segment.start[1] * bounds_height;
        let bx = origin_x + segment.end[0] * bounds_width;
        let by = origin_y + segment.end[1] * bounds_height;
        let dx = bx - ax;
        let dy = by - ay;
        let len_sq = dx * dx + dy * dy;
        if len_sq <= 0.001 {
            continue;
        }
        let t = (((x - ax) * dx + (y - ay) * dy) / len_sq).clamp(0.0, 1.0);
        let closest_x = ax + dx * t;
        let closest_y = ay + dy * t;
        let distance_sq = (x - closest_x).powi(2) + (y - closest_y).powi(2);
        if distance_sq < best_distance_sq {
            best_distance_sq = distance_sq;
            best_progress =
                segment.start_progress + (segment.end_progress - segment.start_progress) * t;
        }
    }

    (best_progress, best_distance_sq.sqrt())
}

fn build_vertices(
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
    focus_pulse: f32,
    space_hold_progress: Option<f32>,
) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(workspace_vertex_capacity_hint(workspace));
    build_vertices_into(
        WorkspaceVertexBuildParams {
            workspace,
            size,
            render_layout,
            focus_pulse,
            space_hold_progress,
            surface_frames: None,
            exiting_surfaces: &HashMap::new(),
            workspace_panel_cache: None,
            status_color: workspace_status_bar_target_color(workspace),
            status_text_frame: None,
        },
        &mut vertices,
    );
    vertices
}

fn reserve_workspace_vertex_capacity(vertices: &mut Vec<Vertex>, workspace: &Workspace) {
    let hint = workspace_vertex_capacity_hint(workspace);
    if vertices.capacity() < hint {
        vertices.reserve(hint - vertices.capacity());
    }
}

fn workspace_vertex_capacity_hint(workspace: &Workspace) -> usize {
    WORKSPACE_BASE_VERTEX_CAPACITY_HINT
        + workspace
            .surfaces
            .len()
            .saturating_mul(WORKSPACE_SURFACE_VERTEX_CAPACITY_HINT)
}

struct WorkspaceSingleSessionTextPane<'a> {
    app: &'a SingleSessionApp,
    rect: Rect,
    size: PhysicalSize<u32>,
    rendered_body_lines: &'a [SingleSessionStyledLine],
    buffers: &'a [Buffer],
}

struct CachedWorkspaceSingleSessionTextPane {
    identity_key: u64,
    text_key: SingleSessionTextKey,
    app: SingleSessionApp,
    rect: Rect,
    size: PhysicalSize<u32>,
    rendered_body_lines: Vec<SingleSessionStyledLine>,
    buffers: Vec<Buffer>,
    child_vertices: Vec<Vertex>,
}

struct WorkspaceSurfaceTransitionFrames {
    frames: Vec<SurfaceVisualFrame>,
    animating: bool,
}

impl WorkspaceSurfaceTransitionFrames {
    fn new(frames: Vec<SurfaceVisualFrame>, animating: bool) -> Self {
        Self { frames, animating }
    }

    fn frame_for_surface(&self, surface_id: u64) -> Option<SurfaceVisualFrame> {
        self.frames
            .iter()
            .copied()
            .find(|frame| frame.id == surface_id)
    }

    fn exiting_frames(&self) -> impl Iterator<Item = SurfaceVisualFrame> + '_ {
        self.frames.iter().copied().filter(|frame| frame.exiting)
    }
}

fn update_workspace_surface_exit_cache(
    cache: &mut HashMap<u64, workspace::Surface>,
    workspace: &Workspace,
    surface_frames: &WorkspaceSurfaceTransitionFrames,
) {
    for surface in &workspace.surfaces {
        cache.insert(surface.id, surface.clone());
    }

    cache.retain(|surface_id, _| {
        workspace
            .surfaces
            .iter()
            .any(|surface| surface.id == *surface_id)
            || surface_frames.frame_for_surface(*surface_id).is_some()
    });
}

fn workspace_transitioned_surface_rect(
    frames: Option<&WorkspaceSurfaceTransitionFrames>,
    surface_id: u64,
    fallback: Rect,
) -> Rect {
    frames
        .and_then(|frames| frames.frame_for_surface(surface_id))
        .map(|frame| rect_from_animated_rect(frame.visual_rect()))
        .unwrap_or(fallback)
}

fn workspace_transitioned_surface_opacity(
    frames: Option<&WorkspaceSurfaceTransitionFrames>,
    surface_id: u64,
) -> f32 {
    frames
        .and_then(|frames| frames.frame_for_surface(surface_id))
        .map(|frame| frame.opacity)
        .unwrap_or(1.0)
}

fn animated_rect_from_rect(rect: Rect) -> AnimatedRect {
    AnimatedRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn rect_from_animated_rect(rect: AnimatedRect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn workspace_surface_transition_targets(
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
) -> Vec<SurfaceVisualTarget> {
    let mut targets = Vec::new();
    if workspace.zoomed {
        if let Some(surface) = workspace.focused_surface() {
            targets.push(SurfaceVisualTarget {
                id: surface.id,
                rect: animated_rect_from_rect(Rect {
                    x: OUTER_PADDING,
                    y: STATUS_BAR_HEIGHT + OUTER_PADDING * 2.0,
                    width: (size.width as f32 - OUTER_PADDING * 2.0).max(1.0),
                    height: (size.height as f32 - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0),
                }),
            });
        }
        return targets;
    }

    for_each_visible_workspace_surface(
        workspace,
        size,
        render_layout,
        0.0,
        |surface, rect, _, _| {
            targets.push(SurfaceVisualTarget {
                id: surface.id,
                rect: animated_rect_from_rect(rect),
            });
        },
    );
    targets
}

fn workspace_panel_size(rect: Rect) -> PhysicalSize<u32> {
    PhysicalSize::new(
        rect.width.round().max(1.0) as u32,
        rect.height.round().max(1.0) as u32,
    )
}

fn workspace_single_session_app_for_surface(
    workspace: &Workspace,
    surface: &workspace::Surface,
) -> Option<SingleSessionApp> {
    let card = surface.session_card()?;
    let session_id = card.session_id.clone();
    let mut app = SingleSessionApp::new(Some(card));
    app.live_session_id = Some(session_id);

    if workspace.mode == InputMode::Insert && workspace.is_focused(surface.id) {
        app.draft = workspace.draft.clone();
        app.draft_cursor = workspace.draft_cursor.min(app.draft.len());
        app.pending_images = workspace.pending_images.clone();
    }

    Some(app)
}

fn push_workspace_single_session_panel(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    rect: Rect,
    parent_size: PhysicalSize<u32>,
    focus_pulse: f32,
    opacity: f32,
) {
    let panel_size = workspace_panel_size(rect);
    let rendered_body_lines = single_session_rendered_body_lines_for_tick(app, panel_size, 0);
    let child_vertices = build_single_session_vertices_with_cached_body(
        app,
        panel_size,
        focus_pulse,
        0,
        0.0,
        1.0,
        &rendered_body_lines,
    );
    append_child_vertices_to_parent_with_opacity(
        vertices,
        &child_vertices,
        panel_size,
        rect,
        parent_size,
        opacity,
    );
}

fn append_child_vertices_to_parent_with_opacity(
    vertices: &mut Vec<Vertex>,
    child_vertices: &[Vertex],
    child_size: PhysicalSize<u32>,
    rect: Rect,
    parent_size: PhysicalSize<u32>,
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    let child_width = child_size.width.max(1) as f32;
    let child_height = child_size.height.max(1) as f32;
    vertices.extend(child_vertices.iter().map(|vertex| {
        let child_x = (vertex.position[0] + 1.0) * 0.5 * child_width;
        let child_y = (1.0 - vertex.position[1]) * 0.5 * child_height;
        let mut color = vertex.color;
        color[3] *= opacity;
        Vertex {
            position: pixel_to_ndc([rect.x + child_x, rect.y + child_y], parent_size),
            color,
        }
    }));
}

fn for_each_visible_workspace_surface(
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
    focus_pulse: f32,
    mut visit: impl FnMut(&workspace::Surface, Rect, bool, f32),
) {
    let width = size.width as f32;
    let height = size.height as f32;
    let workspace_height = (height - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0);
    let workspace_top = STATUS_BAR_HEIGHT + OUTER_PADDING * 2.0;
    let lane_pitch = workspace_height + GAP;
    let column_width = render_layout.column_width;
    let scroll_offset = render_layout.scroll_offset;
    let vertical_scroll_offset = render_layout.vertical_scroll_offset;
    let viewport_left = OUTER_PADDING - GAP;
    let viewport_right = width - OUTER_PADDING + GAP;

    for surface in &workspace.surfaces {
        let column = surface.column as f32;
        let y = workspace_top + surface.lane as f32 * lane_pitch - vertical_scroll_offset;
        if y + workspace_height < workspace_top || y > workspace_top + workspace_height {
            continue;
        }
        let rect = Rect {
            x: OUTER_PADDING + column * (column_width + GAP) - scroll_offset,
            y,
            width: column_width,
            height: workspace_height,
        };
        if rect.x + rect.width < viewport_left || rect.x > viewport_right {
            continue;
        }
        let focused = workspace.is_focused(surface.id);
        let surface_pulse = if focused { focus_pulse } else { 0.0 };
        visit(surface, rect, focused, surface_pulse);
    }
}

fn workspace_visible_surface_count(
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
) -> usize {
    let mut count = 0;
    for_each_visible_workspace_surface(workspace, size, render_layout, 0.0, |_, _, _, _| {
        count += 1;
    });
    count
}

fn build_workspace_single_session_text_panes<'a>(
    cache: &'a mut HashMap<u64, CachedWorkspaceSingleSessionTextPane>,
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
    surface_frames: Option<&WorkspaceSurfaceTransitionFrames>,
    font_system: &mut FontSystem,
) -> Vec<WorkspaceSingleSessionTextPane<'a>> {
    let mut visible_surface_ids = Vec::new();
    if workspace.zoomed {
        if let Some(surface) = workspace.focused_surface()
            && surface.kind == workspace::SurfaceKind::Session
            && let Some(app) = workspace_single_session_app_for_surface(workspace, surface)
        {
            let target_rect = Rect {
                x: OUTER_PADDING,
                y: STATUS_BAR_HEIGHT + OUTER_PADDING * 2.0,
                width: (size.width as f32 - OUTER_PADDING * 2.0).max(1.0),
                height: (size.height as f32 - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0),
            };
            let rect = workspace_transitioned_surface_rect(surface_frames, surface.id, target_rect);
            let panel_size = workspace_panel_size(rect);
            visible_surface_ids.push(surface.id);
            refresh_workspace_text_pane_cache_entry(
                cache,
                surface.id,
                workspace_surface_text_pane_identity_key(workspace, surface, panel_size),
                app,
                rect,
                panel_size,
                font_system,
            );
        }
        retain_workspace_text_pane_cache_for_workspace(cache, workspace);
        return workspace_text_panes_from_cache(cache, &visible_surface_ids);
    }

    for_each_visible_workspace_surface(
        workspace,
        size,
        render_layout,
        0.0,
        |surface, target_rect, _, _| {
            if surface.kind != workspace::SurfaceKind::Session {
                return;
            }
            let Some(app) = workspace_single_session_app_for_surface(workspace, surface) else {
                return;
            };
            let rect = workspace_transitioned_surface_rect(surface_frames, surface.id, target_rect);
            let panel_size = workspace_panel_size(rect);
            visible_surface_ids.push(surface.id);
            refresh_workspace_text_pane_cache_entry(
                cache,
                surface.id,
                workspace_surface_text_pane_identity_key(workspace, surface, panel_size),
                app,
                rect,
                panel_size,
                font_system,
            );
        },
    );

    retain_workspace_text_pane_cache_for_workspace(cache, workspace);
    workspace_text_panes_from_cache(cache, &visible_surface_ids)
}

fn retain_workspace_text_pane_cache_for_workspace(
    cache: &mut HashMap<u64, CachedWorkspaceSingleSessionTextPane>,
    workspace: &Workspace,
) {
    let session_surface_ids = workspace
        .surfaces
        .iter()
        .filter(|surface| surface.kind == workspace::SurfaceKind::Session)
        .map(|surface| surface.id)
        .collect::<HashSet<_>>();
    cache.retain(|surface_id, _| session_surface_ids.contains(surface_id));
}

fn workspace_surface_text_pane_identity_key(
    workspace: &Workspace,
    surface: &workspace::Surface,
    panel_size: PhysicalSize<u32>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    panel_size.width.hash(&mut hasher);
    panel_size.height.hash(&mut hasher);
    surface.id.hash(&mut hasher);
    workspace_surface_kind_key(surface.kind).hash(&mut hasher);
    surface.title.hash(&mut hasher);
    surface.body_lines.hash(&mut hasher);
    surface.detail_lines.hash(&mut hasher);
    surface.session_id.hash(&mut hasher);
    surface.transcript_messages.len().hash(&mut hasher);
    for message in &surface.transcript_messages {
        message.role.hash(&mut hasher);
        message.content.hash(&mut hasher);
    }
    if workspace.mode == InputMode::Insert && workspace.is_focused(surface.id) {
        workspace.draft.hash(&mut hasher);
        workspace.draft_cursor.hash(&mut hasher);
        workspace.pending_images.len().hash(&mut hasher);
    }
    hasher.finish()
}

fn workspace_surface_kind_key(kind: workspace::SurfaceKind) -> u8 {
    match kind {
        workspace::SurfaceKind::Session => 0,
        workspace::SurfaceKind::Scratch => 1,
        workspace::SurfaceKind::WorkspacePlaceholder => 2,
        workspace::SurfaceKind::HotkeyHelp => 3,
        workspace::SurfaceKind::Loading => 4,
        workspace::SurfaceKind::Empty => 5,
    }
}

fn refresh_workspace_text_pane_cache_entry(
    cache: &mut HashMap<u64, CachedWorkspaceSingleSessionTextPane>,
    surface_id: u64,
    identity_key: u64,
    app: SingleSessionApp,
    rect: Rect,
    panel_size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) {
    if let Some(entry) = cache.get_mut(&surface_id)
        && entry.identity_key == identity_key
    {
        entry.app = app;
        entry.rect = rect;
        entry.size = panel_size;
        return;
    }

    let rendered_body_lines = single_session_rendered_body_lines_for_tick(&app, panel_size, 0);
    let key = single_session_text_key_for_tick_with_rendered_body(
        &app,
        panel_size,
        0,
        0.0,
        &rendered_body_lines,
    );

    if let Some(entry) = cache.get_mut(&surface_id)
        && entry.text_key == key
    {
        entry.app = app;
        entry.rect = rect;
        entry.size = panel_size;
        return;
    }

    let child_vertices = build_single_session_vertices_with_cached_body(
        &app,
        panel_size,
        0.0,
        0,
        0.0,
        1.0,
        &rendered_body_lines,
    );
    let buffers = single_session_text_buffers_from_key(&key, panel_size, font_system);
    cache.insert(
        surface_id,
        CachedWorkspaceSingleSessionTextPane {
            identity_key,
            text_key: key,
            app,
            rect,
            size: panel_size,
            rendered_body_lines,
            buffers,
            child_vertices,
        },
    );
}

fn workspace_text_panes_from_cache<'a>(
    cache: &'a HashMap<u64, CachedWorkspaceSingleSessionTextPane>,
    surface_ids: &[u64],
) -> Vec<WorkspaceSingleSessionTextPane<'a>> {
    surface_ids
        .iter()
        .filter_map(|surface_id| cache.get(surface_id))
        .map(|entry| WorkspaceSingleSessionTextPane {
            app: &entry.app,
            rect: entry.rect,
            size: entry.size,
            rendered_body_lines: &entry.rendered_body_lines,
            buffers: &entry.buffers,
        })
        .collect()
}

fn workspace_single_session_text_areas<'a>(
    panes: &'a [WorkspaceSingleSessionTextPane<'a>],
) -> Vec<TextArea<'a>> {
    let mut areas = Vec::new();
    for pane in panes {
        let viewport = single_session_body_viewport_from_lines(
            pane.app,
            pane.size,
            0.0,
            pane.rendered_body_lines,
        );
        let pane_areas = single_session_text_areas_for_app_with_cached_body_viewport_and_reveal(
            pane.app,
            pane.buffers,
            pane.size,
            0.0,
            viewport,
            1.0,
        );
        areas.extend(pane_areas.into_iter().filter_map(|area| {
            let area = offset_workspace_text_area(area, pane.rect);
            (area.bounds.right > area.bounds.left && area.bounds.bottom > area.bounds.top)
                .then_some(area)
        }));
    }
    areas
}

fn offset_workspace_text_area<'a>(area: TextArea<'a>, rect: Rect) -> TextArea<'a> {
    let clip_left = rect.x.floor() as i32;
    let clip_top = rect.y.floor() as i32;
    let clip_right = (rect.x + rect.width).ceil() as i32;
    let clip_bottom = (rect.y + rect.height).ceil() as i32;
    TextArea {
        buffer: area.buffer,
        left: area.left + rect.x,
        top: area.top + rect.y,
        scale: area.scale,
        bounds: TextBounds {
            left: offset_text_bound(area.bounds.left, rect.x).max(clip_left),
            top: offset_text_bound(area.bounds.top, rect.y).max(clip_top),
            right: offset_text_bound(area.bounds.right, rect.x).min(clip_right),
            bottom: offset_text_bound(area.bounds.bottom, rect.y).min(clip_bottom),
        },
        default_color: area.default_color,
    }
}

fn offset_text_bound(value: i32, offset: f32) -> i32 {
    (value as f32 + offset)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

struct WorkspaceVertexBuildParams<'a> {
    workspace: &'a Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
    focus_pulse: f32,
    space_hold_progress: Option<f32>,
    surface_frames: Option<&'a WorkspaceSurfaceTransitionFrames>,
    exiting_surfaces: &'a HashMap<u64, workspace::Surface>,
    workspace_panel_cache: Option<&'a HashMap<u64, CachedWorkspaceSingleSessionTextPane>>,
    status_color: [f32; 4],
    status_text_frame: Option<&'a StatusTextTransitionFrame>,
}

fn build_vertices_into(params: WorkspaceVertexBuildParams<'_>, vertices: &mut Vec<Vertex>) {
    let WorkspaceVertexBuildParams {
        workspace,
        size,
        render_layout,
        focus_pulse,
        space_hold_progress,
        surface_frames,
        exiting_surfaces,
        workspace_panel_cache,
        status_color,
        status_text_frame,
    } = params;
    vertices.clear();
    let width = size.width as f32;
    let height = size.height as f32;

    push_gradient_rect(
        vertices,
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        },
        BACKGROUND_TOP_LEFT,
        BACKGROUND_BOTTOM_LEFT,
        BACKGROUND_BOTTOM_RIGHT,
        BACKGROUND_TOP_RIGHT,
        size,
    );

    let status_rect = Rect {
        x: OUTER_PADDING,
        y: OUTER_PADDING,
        width: (width - OUTER_PADDING * 2.0).max(1.0),
        height: STATUS_BAR_HEIGHT,
    };
    push_rounded_rect(vertices, status_rect, STATUS_RADIUS, status_color, size);

    let active_workspace = workspace.current_workspace();
    push_workspace_number(vertices, active_workspace, status_rect, size);
    push_status_preview(
        vertices,
        workspace,
        active_workspace,
        render_layout,
        surface_frames,
        exiting_surfaces,
        focus_pulse,
        status_rect,
        size,
    );
    push_status_text(vertices, workspace, status_rect, size, status_text_frame);

    if workspace.zoomed {
        if let Some(surface) = workspace.focused_surface() {
            let target_rect = Rect {
                x: OUTER_PADDING,
                y: STATUS_BAR_HEIGHT + OUTER_PADDING * 2.0,
                width: (width - OUTER_PADDING * 2.0).max(1.0),
                height: (height - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0),
            };
            let rect = workspace_transitioned_surface_rect(surface_frames, surface.id, target_rect);
            let opacity = workspace_transitioned_surface_opacity(surface_frames, surface.id);
            let start_index = vertices.len();
            if surface.kind == workspace::SurfaceKind::Session
                && let Some(app) = workspace_single_session_app_for_surface(workspace, surface)
            {
                push_workspace_single_session_panel(
                    vertices,
                    &app,
                    rect,
                    size,
                    focus_pulse,
                    opacity,
                );
            } else {
                push_surface(vertices, rect, surface.color_index, true, focus_pulse, size);
                let draft = focused_panel_draft(workspace, surface.id);
                push_panel_contents(
                    vertices,
                    surface,
                    rect,
                    size,
                    true,
                    workspace.detail_scroll,
                    draft.as_deref(),
                );
                multiply_vertex_alpha(&mut vertices[start_index..], opacity);
            }
        }
        if let Some(progress) = space_hold_progress {
            push_space_hold_progress(vertices, progress, size);
        }
        push_workspace_exiting_surfaces(vertices, surface_frames, exiting_surfaces, size);
        return;
    }

    for_each_visible_workspace_surface(
        workspace,
        size,
        render_layout,
        focus_pulse,
        |surface, target_rect, focused, surface_pulse| {
            let rect = workspace_transitioned_surface_rect(surface_frames, surface.id, target_rect);
            let opacity = workspace_transitioned_surface_opacity(surface_frames, surface.id);
            let start_index = vertices.len();
            if surface.kind == workspace::SurfaceKind::Session {
                if surface_pulse <= 0.001
                    && let Some(entry) =
                        workspace_panel_cache.and_then(|cache| cache.get(&surface.id))
                {
                    append_child_vertices_to_parent_with_opacity(
                        vertices,
                        &entry.child_vertices,
                        entry.size,
                        rect,
                        size,
                        opacity,
                    );
                    return;
                }
                let Some(app) = workspace_single_session_app_for_surface(workspace, surface) else {
                    return;
                };
                push_workspace_single_session_panel(
                    vertices,
                    &app,
                    rect,
                    size,
                    surface_pulse,
                    opacity,
                );
                return;
            }
            push_surface(
                vertices,
                rect,
                surface.color_index,
                focused,
                surface_pulse,
                size,
            );
            let draft = focused_panel_draft(workspace, surface.id);
            push_panel_contents(vertices, surface, rect, size, false, 0, draft.as_deref());
            multiply_vertex_alpha(&mut vertices[start_index..], opacity);
        },
    );

    push_workspace_exiting_surfaces(vertices, surface_frames, exiting_surfaces, size);

    if let Some(progress) = space_hold_progress {
        push_space_hold_progress(vertices, progress, size);
    }
}

fn push_workspace_exiting_surfaces(
    vertices: &mut Vec<Vertex>,
    surface_frames: Option<&WorkspaceSurfaceTransitionFrames>,
    exiting_surfaces: &HashMap<u64, workspace::Surface>,
    size: PhysicalSize<u32>,
) {
    let Some(surface_frames) = surface_frames else {
        return;
    };

    for frame in surface_frames.exiting_frames() {
        let Some(surface) = exiting_surfaces.get(&frame.id) else {
            continue;
        };
        let rect = rect_from_animated_rect(frame.visual_rect());
        let start_index = vertices.len();
        push_surface(vertices, rect, surface.color_index, false, 0.0, size);
        push_panel_contents(vertices, surface, rect, size, false, 0, None);
        multiply_vertex_alpha(&mut vertices[start_index..], frame.opacity);
    }
}

fn push_space_hold_progress(vertices: &mut Vec<Vertex>, progress: f32, size: PhysicalSize<u32>) {
    let width = size.width as f32;
    let bar_width = (width * SPACE_HOLD_PROGRESS_WIDTH_FRACTION).clamp(120.0, 460.0);
    let rect = Rect {
        x: (width - bar_width) * 0.5,
        y: OUTER_PADDING + STATUS_BAR_HEIGHT + 4.0,
        width: bar_width,
        height: SPACE_HOLD_PROGRESS_HEIGHT,
    };
    push_rounded_rect(
        vertices,
        rect,
        SPACE_HOLD_PROGRESS_HEIGHT * 0.5,
        SPACE_HOLD_PROGRESS_TRACK_COLOR,
        size,
    );
    let fill = Rect {
        width: (rect.width * progress.clamp(0.0, 1.0)).max(SPACE_HOLD_PROGRESS_HEIGHT),
        ..rect
    };
    push_rounded_rect(
        vertices,
        fill,
        SPACE_HOLD_PROGRESS_HEIGHT * 0.5,
        SPACE_HOLD_PROGRESS_FILL_COLOR,
        size,
    );
}

fn multiply_vertex_alpha(vertices: &mut [Vertex], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity >= 0.999 {
        return;
    }
    for vertex in vertices {
        vertex.color[3] *= opacity;
    }
}

fn workspace_render_layout(
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    monitor_size: Option<PhysicalSize<u32>>,
) -> WorkspaceRenderLayout {
    let workspace_width = (size.width as f32 - OUTER_PADDING * 2.0).max(1.0);
    let workspace_height = (size.height as f32 - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0);
    let lane_pitch = workspace_height + GAP;
    let active_workspace = workspace.current_workspace();
    let visible = visible_column_layout(
        workspace,
        size.width,
        monitor_size.map(|size| size.width),
        active_workspace,
    );
    let visible_columns_f = visible.visible_columns as f32;
    let total_gap_width = GAP * (visible_columns_f - 1.0).max(0.0);
    let column_width = ((workspace_width - total_gap_width) / visible_columns_f).max(1.0);
    let scroll_offset = visible.first_visible_column as f32 * (column_width + GAP);
    let vertical_scroll_offset = active_workspace as f32 * lane_pitch;

    WorkspaceRenderLayout {
        visible,
        column_width,
        scroll_offset,
        vertical_scroll_offset,
    }
}

fn visible_column_layout(
    workspace: &Workspace,
    window_width: u32,
    monitor_width: Option<u32>,
    active_workspace: i32,
) -> VisibleColumnLayout {
    let visible_columns = inferred_visible_column_count(
        window_width,
        monitor_width,
        workspace.preferred_panel_screen_fraction(),
    );
    let focused_column = workspace
        .focused_surface()
        .map(|surface| surface.column)
        .unwrap_or_default();
    let (min_column, max_column) = workspace
        .surfaces
        .iter()
        .filter(|surface| surface.lane == active_workspace)
        .map(|surface| surface.column)
        .fold((focused_column, focused_column), |(min, max), column| {
            (min.min(column), max.max(column))
        });
    let visible_columns_i = visible_columns as i32;
    let max_first_column = (max_column - visible_columns_i + 1).max(min_column);
    let preferred_first_column = focused_column - visible_columns_i / 2;
    let first_visible_column = preferred_first_column.clamp(min_column, max_first_column);

    VisibleColumnLayout {
        visible_columns,
        first_visible_column,
    }
}

fn inferred_visible_column_count(
    window_width: u32,
    monitor_width: Option<u32>,
    preferred_panel_screen_fraction: f32,
) -> u32 {
    let Some(monitor_width) = monitor_width.filter(|width| *width > 0) else {
        return 1;
    };

    let preferred_panel_screen_fraction = preferred_panel_screen_fraction.clamp(0.25, 1.0);
    let target_panel_width = monitor_width as f32 * preferred_panel_screen_fraction;
    ((window_width as f32 / target_panel_width + PANEL_FIT_TOLERANCE).floor() as u32).clamp(1, 4)
}

fn push_status_text(
    vertices: &mut Vec<Vertex>,
    workspace: &Workspace,
    status_rect: Rect,
    size: PhysicalSize<u32>,
    transition_frame: Option<&StatusTextTransitionFrame>,
) {
    let settled;
    let frame = if let Some(frame) = transition_frame {
        frame
    } else {
        settled = StatusTextTransitionFrame::settled(workspace_status_text(workspace));
        &settled
    };
    if let Some(previous) = frame.previous.as_ref() {
        push_status_text_visual(vertices, previous, status_rect, size);
    }
    push_status_text_visual(vertices, &frame.current, status_rect, size);
}

fn push_status_text_visual(
    vertices: &mut Vec<Vertex>,
    visual: &StatusTextVisualFrame,
    status_rect: Rect,
    size: PhysicalSize<u32>,
) {
    if visual.opacity <= 0.001 {
        return;
    }
    let text_width = bitmap_text_width(&visual.text, BITMAP_TEXT_PIXEL);
    let x = status_rect.x + status_rect.width - STATUS_TEXT_RIGHT_PADDING - text_width;
    let y = status_rect.y
        + (status_rect.height - bitmap_text_height(BITMAP_TEXT_PIXEL)) / 2.0
        + visual.y_offset_pixels;
    if x > status_rect.x {
        let mut color = STATUS_TEXT_COLOR;
        color[3] *= visual.opacity.clamp(0.0, 1.0);
        push_bitmap_text(
            vertices,
            &visual.text,
            x,
            y,
            BITMAP_TEXT_PIXEL,
            color,
            size,
            text_width,
        );
    }
}

fn workspace_status_text(workspace: &Workspace) -> String {
    let mode = match workspace.mode {
        InputMode::Navigation => "NAV",
        InputMode::Insert => "INS",
    };
    let panel_percent = (workspace.preferred_panel_screen_fraction() * 100.0).round() as u32;
    format!("{mode} P{panel_percent} {}", desktop_build_hash_label())
}

fn workspace_status_bar_target_color(workspace: &Workspace) -> [f32; 4] {
    match workspace.mode {
        InputMode::Navigation => NAV_STATUS_COLOR,
        InputMode::Insert => INSERT_STATUS_COLOR,
    }
}

fn desktop_build_hash_label() -> &'static str {
    option_env!("JCODE_DESKTOP_GIT_HASH").unwrap_or("unknown")
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
