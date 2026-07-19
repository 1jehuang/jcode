mod animation;
mod desktop_app_driver;
mod desktop_benchmark;
mod desktop_branding;
pub(crate) use desktop_branding::*;
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
#[cfg(test)]
#[path = "state_space_tests.rs"]
mod state_space_tests;
mod workspace;

mod workspace_vertices;
pub(crate) use workspace_vertices::*;
mod hero_mask;
pub(crate) use hero_mask::*;
mod canvas;
pub(crate) use canvas::*;
mod desktop_profiling;
pub(crate) use desktop_profiling::*;
mod desktop_benchmarks_run;
pub(crate) use desktop_benchmarks_run::*;
mod desktop_benchmarks_transcript;
pub(crate) use desktop_benchmarks_transcript::*;
mod desktop_benchmarks_scroll;
pub(crate) use desktop_benchmarks_scroll::*;
mod desktop_reload;
pub(crate) use desktop_reload::*;
mod desktop_events_glue;
pub(crate) use desktop_events_glue::*;
mod desktop_scroll;
pub(crate) use desktop_scroll::*;
mod desktop_clipboard;
pub(crate) use desktop_clipboard::*;
mod desktop_jobs;
pub(crate) use desktop_jobs::*;
mod desktop_capture;
pub(crate) use desktop_capture::*;
mod desktop_tasks;
mod streaming_text_style;
pub(crate) use desktop_tasks::*;
mod desktop_worker_process;
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
pub(crate) use desktop_worker_process::*;
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
pub(crate) use streaming_text_style::*;
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
const DESKTOP_RELOAD_HANDOFF_PLACEMENT_ENV: &str = "JCODE_DESKTOP_RELOAD_PLACEMENT_FILE";
const DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL: Duration = Duration::from_millis(25);
const DESKTOP_RELOAD_HANDOFF_TIMEOUT: Duration = Duration::from_secs(8);
const DESKTOP_RELOAD_STARTUP_RELEASE_TIMEOUT: Duration = Duration::from_secs(3);
const DESKTOP_RELOAD_MAX_RESTORED_DIMENSION: u32 = 32_768;
const OUTER_PADDING: f32 = 12.0;
const GAP: f32 = 10.0;
const STATUS_BAR_HEIGHT: f32 = 30.0;
const FOCUSED_BORDER_WIDTH: f32 = 2.0;
const UNFOCUSED_BORDER_WIDTH: f32 = 1.5;
const PANEL_RADIUS: f32 = 12.0;
const STATUS_RADIUS: f32 = 9.0;
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
/// Minimum spacing between animation-driven redraws.
///
/// Without this, the desktop render loop re-requests a redraw immediately after
/// every animated frame (welcome-hero reveal, focus pulse, spinners, smooth
/// scroll, etc.). Because the surface uses non-blocking `Mailbox` presentation,
/// `present()` returns instantly, so the unthrottled loop renders at hundreds of
/// fps and pins the main thread near 100% CPU, starving input handling and the
/// compositor (the root cause of desktop lag/jank). ~16ms paces continuous
/// animations to about 60fps, matching typical display refresh.
const DESKTOP_ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(16);
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
const SINGLE_SESSION_SCROLL_ANIMATION_DURATION: Duration = Duration::from_millis(90);
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

fn desktop_automatic_redraw_allowed(
    now: Instant,
    surface_renderable: bool,
    window_occluded: bool,
    surface_timeout_redraw_at: Option<Instant>,
) -> bool {
    surface_renderable
        && !window_occluded
        && surface_timeout_redraw_at.is_none_or(|redraw_at| now >= redraw_at)
}

fn desktop_surface_timeout_wake(
    surface_renderable: bool,
    window_occluded: bool,
    surface_timeout_redraw_at: Option<Instant>,
) -> Option<Instant> {
    (surface_renderable && !window_occluded)
        .then_some(surface_timeout_redraw_at)
        .flatten()
}

#[derive(Clone, Debug, Default)]
struct AutomaticRedrawState {
    pending: bool,
    request_outstanding: bool,
}

impl AutomaticRedrawState {
    fn schedule(&mut self, automatic_redraw_allowed: bool) -> bool {
        self.pending = true;
        self.request_pending(automatic_redraw_allowed)
    }

    fn request_pending(&mut self, automatic_redraw_allowed: bool) -> bool {
        if self.pending && automatic_redraw_allowed && !self.request_outstanding {
            self.request_outstanding = true;
            true
        } else {
            false
        }
    }

    fn begin_redraw(&mut self, render_will_start: bool) {
        self.request_outstanding = false;
        if render_will_start {
            self.pending = false;
        }
    }

    fn park(&mut self) {
        self.pending = true;
        self.request_outstanding = false;
    }
}

fn consume_pending_backend_redraw_for_render(
    render_will_start: bool,
    pending_backend_redraw_since: &mut Option<Instant>,
) -> Option<Instant> {
    render_will_start
        .then(|| pending_backend_redraw_since.take())
        .flatten()
}

fn desktop_background_wake(
    now: Instant,
    automatic_redraw_allowed: bool,
    frame_animation_active: bool,
) -> Option<Instant> {
    if automatic_redraw_allowed && frame_animation_active {
        Some(now + BACKGROUND_POLL_INTERVAL)
    } else {
        None
    }
}

/// Compute the next paced animation redraw time.
///
/// Returns `Some(now + DESKTOP_ANIMATION_FRAME_INTERVAL)` while an animation is
/// active and `None` once it settles. Callers schedule this instead of calling
/// `request_redraw()` immediately, which would render as fast as the CPU allows
/// (the surface presents without blocking) and pin the main thread near 100%
/// CPU, starving input handling and the compositor.
fn next_animation_redraw_at(
    now: Instant,
    frame_animation_active: bool,
    scroll_animation_active: bool,
    space_hold_animation_active: bool,
    automatic_redraw_allowed: bool,
) -> Option<Instant> {
    (automatic_redraw_allowed
        && (frame_animation_active || scroll_animation_active || space_hold_animation_active))
        .then(|| now + DESKTOP_ANIMATION_FRAME_INTERVAL)
}

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
            "{} {} {}",
            DESKTOP_PRODUCT_NAME,
            desktop_app_directory_label(),
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
    if let Some(frames) = real_transcript_scroll_benchmark_frames(&args) {
        return run_real_transcript_scroll_benchmark(frames);
    }
    if let Some(frames) = real_transcript_action_benchmark_frames(&args) {
        return run_real_transcript_action_benchmark(frames);
    }
    if let Some(output_dir) = hero_screenshot_capture_dir(&args) {
        return run_hero_screenshot_capture(&output_dir).await;
    }
    if let Some(capture) = gallery_screenshot_capture_request(&args) {
        return run_gallery_screenshot_capture(&capture).await;
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
    let mut window_builder = WindowBuilder::new().with_title(DESKTOP_PRODUCT_NAME);
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
    let mut hot_reloader =
        DesktopHotReloader::new(process_role.reload_strategy(), event_loop_proxy.clone());
    if process_role == DesktopProcessRole::StableHost {
        hot_reloader.start_app_worker_for_current_binary(&app, &window, "stable host startup");
    }
    let preferences_save_tx = spawn_desktop_preferences_saver();
    let mut power_inhibitor = power_inhibit::PowerInhibitor::new();
    let (session_event_tx, session_event_rx) = mpsc::channel();
    spawn_session_event_forwarder(session_event_rx, event_loop_proxy.clone());
    if simulate_stream_requested(&args) && app.is_single_session() {
        // Dev-only: drive the real streaming pipeline with synthetic, bursty
        // TextDelta events so the streaming reveal animation can be observed and
        // recorded live without a backend. Mirrors provider chunk cadence.
        if let DesktopApp::SingleSession(single) = &mut app {
            seed_desktop_stream_simulator_transcript(single);
        }
        window.set_title(&app.status_title());
        spawn_desktop_stream_simulator(session_event_tx.clone());
    }
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
    let mut automatic_redraw = AutomaticRedrawState::default();
    // Scheduled time for the next animation-driven redraw. Continuous animations
    // re-arm this each presented frame so the loop paces itself to roughly the
    // display refresh rate instead of busy-spinning the main thread.
    let mut animation_redraw_at: Option<Instant> = None;
    let mut pending_resize: Option<PhysicalSize<u32>> = None;
    let mut window_occluded = false;
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
        let automatic_redraw_allowed = desktop_automatic_redraw_allowed(
            event_loop_now,
            surface_renderable,
            window_occluded,
            surface_timeout_redraw_at,
        );
        let renderer_ready = renderer.is_gpu_ready();
        let has_background_work = app.has_background_work();
        power_inhibitor.set_active(has_background_work);
        let default_wake = desktop_background_wake(
            event_loop_now,
            automatic_redraw_allowed,
            app.has_frame_animation(),
        );
        let backend_wake = automatic_redraw_allowed
            .then(|| {
                pending_backend_redraw_since
                    .and(last_backend_redraw_request)
                    .map(|last| last + BACKEND_REDRAW_FRAME_INTERVAL)
            })
            .flatten();
        let hot_reload_wake = hot_reloader.next_wake(event_loop_now);
        let animation_wake = automatic_redraw_allowed
            .then_some(animation_redraw_at)
            .flatten();
        let surface_timeout_wake = desktop_surface_timeout_wake(
            surface_renderable,
            window_occluded,
            surface_timeout_redraw_at,
        );
        let wake = [
            default_wake,
            backend_wake,
            hot_reload_wake,
            surface_timeout_wake,
            animation_wake,
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
        let no_paint_active = automatic_redraw_allowed
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
        ) && automatic_redraw.schedule(automatic_redraw_allowed)
        {
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
            if automatic_redraw.schedule(automatic_redraw_allowed) {
                window.request_redraw();
            }
        }
        if worker_drain.reload_requested {
            show_desktop_reload_notice(&mut app);
            window.set_title(&app.status_title());
            if automatic_redraw.schedule(automatic_redraw_allowed) {
                window.request_redraw();
            }
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
                    window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    pending_resize = Some(window.inner_size());
                    window.request_redraw();
                }
                WindowEvent::Focused(focused) => {
                    forward_app_worker_input(
                        &mut hot_reloader,
                        DesktopInputEvent::Window(DesktopWindowEvent::Focused(focused)),
                    );
                }
                WindowEvent::Occluded(occluded) => {
                    window_occluded = occluded;
                    if occluded {
                        animation_redraw_at = None;
                        automatic_redraw.park();
                    } else if automatic_redraw.schedule(desktop_automatic_redraw_allowed(
                        Instant::now(),
                        desktop_surface_size_is_renderable(window.inner_size()),
                        false,
                        surface_timeout_redraw_at,
                    )) {
                        window.request_redraw();
                    }
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
                        KeyOutcome::SpawnSelfDevSession => {
                            if let Err(error) = session_launch::launch_selfdev_session() {
                                desktop_log::error(format_args!(
                                    "jcode-desktop: failed to spawn self-dev session: {error:#}"
                                ));
                            }
                        }
                        KeyOutcome::SpawnHomeSession => {
                            if let Err(error) = session_launch::launch_home_session() {
                                desktop_log::error(format_args!(
                                    "jcode-desktop: failed to spawn home session: {error:#}"
                                ));
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
                                        && let Err(error) = reasoning_effort_queue.request(
                                            effort,
                                            target_session_id,
                                            session_event_tx.clone(),
                                        )
                                    {
                                        apply_single_session_error(&mut app, error);
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
                    if start_pending_transcript_hydration(&mut app, event_loop_proxy.clone()) {
                        window.request_redraw();
                    }
                    log_desktop_slow_interaction(
                        "keyboard_input",
                        keyboard_started.elapsed(),
                        serde_json::json!({ "key": key_debug }),
                    );
                }
                WindowEvent::RedrawRequested => {
                    let window_size = window.inner_size();
                    let redraw_now = Instant::now();
                    let surface_renderable = desktop_surface_size_is_renderable(window_size);
                    let redraw_allowed = desktop_automatic_redraw_allowed(
                        redraw_now,
                        surface_renderable,
                        window_occluded,
                        surface_timeout_redraw_at,
                    );
                    let render_will_start = redraw_allowed && renderer.is_gpu_ready();
                    automatic_redraw.begin_redraw(render_will_start);
                    let pending_backend_redraw_for_frame =
                        consume_pending_backend_redraw_for_render(
                            render_will_start,
                            &mut pending_backend_redraw_since,
                        );
                    let Some(canvas) = renderer.canvas_mut() else {
                        return;
                    };
                    if let Some(size) = pending_resize.take() {
                        canvas.resize(size);
                        // Winit may deliver many resize events between presented
                        // frames. Forward only the newest dimensions so the app
                        // worker does not rebuild and serialize a discarded scene
                        // for every intermediate event.
                        forward_app_worker_input(
                            &mut hot_reloader,
                            DesktopInputEvent::Window(DesktopWindowEvent::Resized {
                                width: size.width,
                                height: size.height,
                                scale_factor: window.scale_factor() as f32,
                            }),
                        );
                    }
                    if !surface_renderable {
                        canvas.suspend_for_zero_size(window_size);
                        animation_redraw_at = None;
                        return;
                    }
                    if window_occluded {
                        animation_redraw_at = None;
                        return;
                    }
                    if let Some(redraw_at) = surface_timeout_redraw_at
                        && redraw_now < redraw_at
                    {
                        target.set_control_flow(ControlFlow::WaitUntil(redraw_at));
                        return;
                    }
                    if let Some(first_pending_backend_redraw) = pending_backend_redraw_for_frame {
                        interaction_latency.mark("backend_events", first_pending_backend_redraw);
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
                        // Pace continuous animations instead of immediately
                        // re-requesting a redraw. An immediate request makes the
                        // event loop render as fast as the CPU allows (the surface
                        // presents without blocking), pinning the main thread near
                        // 100% CPU and starving input/compositor scheduling. The
                        // scheduled wake is serviced in AboutToWait.
                        let now = Instant::now();
                        let space_hold_animation_active = space_hold_started_at.is_some()
                            && !space_hold_consumed
                            && app.is_workspace();
                        animation_redraw_at = next_animation_redraw_at(
                            now,
                            frame.animation_active,
                            scroll_accumulator.is_active(),
                            space_hold_animation_active,
                            desktop_automatic_redraw_allowed(
                                now,
                                true,
                                window_occluded,
                                surface_timeout_redraw_at,
                            ),
                        );
                    }
                    Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                        surface_timeout_backoff.reset();
                        surface_timeout_redraw_at = None;
                        canvas.resize(window.inner_size());
                        if automatic_redraw.schedule(true) {
                            window.request_redraw();
                        }
                    }
                    Err(SurfaceError::OutOfMemory) => target.exit(),
                    Err(SurfaceError::Timeout) => {
                        let now = Instant::now();
                        let (delay, consecutive_timeouts) = surface_timeout_backoff.record_timeout();
                        let redraw_at = now + delay;
                        surface_timeout_redraw_at = Some(redraw_at);
                        animation_redraw_at = None;
                        automatic_redraw.schedule(false);
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
                    if automatic_redraw.schedule(automatic_redraw_allowed) {
                        window.request_redraw();
                    }
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
                            if let Some(placement) = handoff.signal_ready_and_wait_for_release() {
                                if let Some(position) = placement.position {
                                    window.set_outer_position(position);
                                }
                                let _ = window.request_inner_size(placement.inner_size);
                            }
                            window.set_visible(true);
                            startup_trace.mark("reload handoff released");
                        }
                        reload_startup_handoff = None;
                        if automatic_redraw.schedule(automatic_redraw_allowed) {
                            window.request_redraw();
                        }
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
            Event::UserEvent(DesktopUserEvent::AppWorkerActivity) => {}
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
                    if automatic_redraw.schedule(automatic_redraw_allowed) {
                        window.request_redraw();
                    }
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
                    if automatic_redraw.schedule(automatic_redraw_allowed) {
                        window.request_redraw();
                    }
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
                if automatic_redraw.schedule(automatic_redraw_allowed) {
                    window.request_redraw();
                }
            }
            Event::UserEvent(DesktopUserEvent::GitHubIssuesSyncFinished(result)) => {
                github_issue_sync_running = false;
                app.apply_github_issue_sync_result(result);
                window.set_title(&app.status_title());
                interaction_latency.mark("github_issue_sync", Instant::now());
                if automatic_redraw.schedule(automatic_redraw_allowed) {
                    window.request_redraw();
                }
            }
            Event::UserEvent(DesktopUserEvent::TranscriptHydrated {
                session_id,
                result,
                loaded_in,
            }) => {
                if app.apply_hydrated_transcript(&session_id, result) {
                    desktop_log::info(format_args!(
                        "jcode-desktop: hydrated resumed transcript for {session_id} in {}ms",
                        loaded_in.as_millis()
                    ));
                    window.set_title(&app.status_title());
                    interaction_latency.mark("transcript_hydration", Instant::now());
                    if automatic_redraw.schedule(automatic_redraw_allowed) {
                        window.request_redraw();
                    }
                }
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
                    pending_backend_redraw_since.get_or_insert(now);
                    let redraw_due = last_backend_redraw_request.is_none_or(|last| {
                        now.saturating_duration_since(last) >= BACKEND_REDRAW_FRAME_INTERVAL
                    });
                    if redraw_due && automatic_redraw_allowed {
                        last_backend_redraw_request = Some(now);
                        if automatic_redraw.schedule(true) {
                            window.request_redraw();
                        }
                        redraw_requested = true;
                    } else {
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
                let now = Instant::now();
                if let Some(redraw_at) = surface_timeout_redraw_at {
                    if now >= redraw_at && surface_renderable && !window_occluded {
                        surface_timeout_redraw_at = None;
                    }
                }
                let automatic_redraw_allowed = desktop_automatic_redraw_allowed(
                    now,
                    surface_renderable,
                    window_occluded,
                    surface_timeout_redraw_at,
                );
                let animation_tick_due = animation_redraw_at
                    .is_some_and(|redraw_at| now >= redraw_at);
                if animation_tick_due {
                    animation_redraw_at = None;
                }

                let mut paced_redraw_needed = animation_tick_due && app.has_frame_animation();
                if animation_tick_due && automatic_redraw_allowed && app.is_single_session() {
                    let about_to_wait_started = now;
                    let size = window.inner_size();
                    let previous_smooth_scroll = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        size,
                        &mut scroll_metrics_cache,
                    );
                    let frame = scroll_accumulator.frame(now);
                    let scroll_changed = frame.scroll_lines.is_some_and(|lines| {
                        let changed = app.scroll_single_session_body(
                            lines,
                            size,
                            &mut scroll_metrics_cache,
                        );
                        if !changed {
                            scroll_accumulator.stop();
                        }
                        changed
                    });
                    let next_smooth_scroll = app.single_session_smooth_scroll_lines(
                        scroll_accumulator.pending_lines(),
                        size,
                        &mut scroll_metrics_cache,
                    );
                    if scroll_changed
                        || frame.active
                        || (next_smooth_scroll - previous_smooth_scroll).abs()
                            >= SCROLL_FRACTIONAL_EPSILON
                    {
                        interaction_latency.mark("scroll_momentum", about_to_wait_started);
                        paced_redraw_needed = true;
                    }
                } else if scroll_accumulator.is_active() {
                    if !app.is_single_session() {
                        scroll_accumulator.reset();
                        scroll_metrics_cache.clear();
                    }
                }
                let mut workspace_title_changed = false;
                if animation_tick_due
                    && automatic_redraw_allowed
                    && let (DesktopApp::Workspace(workspace), Some(started_at)) =
                        (&mut app, space_hold_started_at)
                    && !space_hold_consumed
                {
                    if now.saturating_duration_since(started_at) >= workspace.space_hold_toggle_duration() {
                        space_hold_consumed = true;
                        if matches!(workspace.handle_key(KeyInput::ToggleInputMode), KeyOutcome::Redraw) {
                            workspace_title_changed = true;
                        }
                    }
                    paced_redraw_needed = true;
                }
                if workspace_title_changed {
                    window.set_title(&app.status_title());
                }
                if animation_tick_due
                    && paced_redraw_needed
                    && automatic_redraw.schedule(automatic_redraw_allowed)
                {
                    window.request_redraw();
                }
                if pending_backend_redraw_since.is_some() {
                    if automatic_redraw_allowed
                        && last_backend_redraw_request.is_none_or(|last| {
                            now.saturating_duration_since(last) >= BACKEND_REDRAW_FRAME_INTERVAL
                        })
                    {
                        last_backend_redraw_request = Some(now);
                        if automatic_redraw.schedule(true) {
                            window.request_redraw();
                        }
                    }
                }
                if hot_reloader.poll(&app, &window) {
                    target.exit();
                    return;
                }

                if let Some(canvas) = renderer.canvas_mut()
                    && automatic_redraw_allowed
                    && canvas.needs_initial_frame
                {
                    canvas.needs_initial_frame = false;
                    if automatic_redraw.schedule(true) {
                        window.request_redraw();
                    }
                }
                if automatic_redraw.request_pending(automatic_redraw_allowed) {
                    window.request_redraw();
                }
                let space_hold_animation_active = space_hold_started_at.is_some()
                    && !space_hold_consumed
                    && app.is_workspace();
                if animation_redraw_at.is_none() {
                    // An animation is active but no paced redraw is scheduled yet
                    // (e.g. it just became active). Schedule one instead of
                    // requesting a redraw on every loop iteration, which would
                    // busy-spin the main thread at 100% CPU.
                    animation_redraw_at = next_animation_redraw_at(
                        now,
                        app.has_frame_animation(),
                        scroll_accumulator.is_active(),
                        space_hold_animation_active,
                        automatic_redraw_allowed,
                    );
                }
            }
            _ => {}
        }
    })?;

    Ok(())
}

enum DesktopUserEvent {
    AppWorkerActivity,
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
    TranscriptHydrated {
        session_id: String,
        result: std::result::Result<Option<Vec<workspace::SessionTranscriptMessage>>, String>,
        loaded_in: Duration,
    },
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
        single_session.request_transcript_hydration(&session_id);
        *self = Self::SingleSession(single_session);
        true
    }

    /// Take the session id queued for off-thread transcript hydration.
    fn take_pending_transcript_hydration(&mut self) -> Option<String> {
        match self {
            Self::SingleSession(app) => app.take_pending_transcript_hydration(),
            Self::Workspace(_) => None,
        }
    }

    /// Apply a transcript that finished loading off the UI thread.
    fn apply_hydrated_transcript(
        &mut self,
        session_id: &str,
        result: std::result::Result<Option<Vec<workspace::SessionTranscriptMessage>>, String>,
    ) -> bool {
        match self {
            Self::SingleSession(app) => app.apply_hydrated_transcript(session_id, result),
            Self::Workspace(_) => false,
        }
    }

    /// Service any queued transcript hydration synchronously. Used by the
    /// app-worker process, which has no event-loop proxy; the disk scan is
    /// bounded so the worst case stays small.
    fn service_pending_transcript_hydration_blocking(&mut self) {
        if let Self::SingleSession(app) = self
            && let Some(session_id) = app.take_pending_transcript_hydration()
        {
            app.hydrate_resumed_session_from_disk(&session_id);
        }
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

fn desktop_build_hash_label() -> &'static str {
    option_env!("JCODE_DESKTOP_GIT_HASH").unwrap_or("unknown")
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
