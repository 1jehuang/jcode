mod animation;
mod desktop_prefs;
mod render_helpers;
mod session_data;
mod session_launch;
mod single_session;
mod single_session_render;
mod workspace;

use animation::{AnimatedViewport, FocusPulse, VisibleColumnLayout, WorkspaceRenderLayout};
use anyhow::{Context, Result};
use base64::Engine;
use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Color as TextColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
};
use render_helpers::*;
use single_session::{
    SINGLE_SESSION_FONT_FAMILY, SelectionPoint, SingleSessionApp, SingleSessionLineStyle,
    SingleSessionStyledLine, single_session_surface, single_session_typography,
};
use single_session_render::*;
use wgpu::util::DeviceExt;
use wgpu::{CompositeAlphaMode, PresentMode, SurfaceError, TextureUsages};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowBuilder};
use workspace::{InputMode, KeyInput, KeyOutcome, PanelSizePreset, Workspace};

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEFAULT_WINDOW_WIDTH: f64 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f64 = 800.0;
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
const SINGLE_SESSION_STATUS_GAP: f32 = 30.0;
const SINGLE_SESSION_CARET_WIDTH: f32 = 2.0;
const SINGLE_SESSION_CARET_COLOR: [f32; 4] = [0.130, 0.150, 0.190, 0.92];
const SESSION_SPAWN_REFRESH_DELAY: Duration = Duration::from_millis(350);
const BACKGROUND_POLL_INTERVAL: Duration = Duration::from_millis(33);
const HEADLESS_CHAT_SMOKE_TIMEOUT: Duration = Duration::from_secs(90);

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
const UNFOCUSED_BORDER_COLOR: [f32; 4] = [0.170, 0.190, 0.230, 0.68];
const NAV_STATUS_COLOR: [f32; 4] = [0.184, 0.204, 0.251, 1.0];
const INSERT_STATUS_COLOR: [f32; 4] = [0.310, 0.435, 0.376, 1.0];
const STATUS_PREVIEW_ACTIVE_GROUP_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.16];
const STATUS_PREVIEW_EMPTY_FOCUSED_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.50];
const STATUS_PREVIEW_VIEWPORT_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.78];
const WORKSPACE_NUMBER_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.90];
const STATUS_TEXT_COLOR: [f32; 4] = [0.953, 0.965, 0.984, 0.88];
const PANEL_TITLE_COLOR: [f32; 4] = [0.090, 0.105, 0.135, 0.86];
const PANEL_BODY_COLOR: [f32; 4] = [0.070, 0.082, 0.110, 0.88];
const ASSISTANT_TEXT_COLOR: [f32; 4] = [0.030, 0.165, 0.190, 0.92];
const USER_TEXT_COLOR: [f32; 4] = [0.115, 0.145, 0.430, 0.94];
const USER_CONTINUATION_TEXT_COLOR: [f32; 4] = [0.135, 0.155, 0.360, 0.86];
const TOOL_TEXT_COLOR: [f32; 4] = [0.530, 0.330, 0.070, 0.90];
const META_TEXT_COLOR: [f32; 4] = [0.255, 0.300, 0.380, 0.76];
const CODE_TEXT_COLOR: [f32; 4] = [0.235, 0.260, 0.310, 0.92];
const STATUS_TEXT_ACCENT_COLOR: [f32; 4] = [0.235, 0.375, 0.310, 0.90];
const ERROR_TEXT_COLOR: [f32; 4] = [0.675, 0.150, 0.150, 0.92];
const OVERLAY_TEXT_COLOR: [f32; 4] = [0.185, 0.215, 0.285, 0.84];
const OVERLAY_SELECTION_TEXT_COLOR: [f32; 4] = [0.080, 0.120, 0.210, 0.96];
const PANEL_SECTION_COLOR: [f32; 4] = [0.085, 0.100, 0.130, 0.78];
const SELECTION_HIGHLIGHT_COLOR: [f32; 4] = [0.220, 0.420, 0.700, 0.22];
const CODE_BLOCK_BACKGROUND_COLOR: [f32; 4] = [0.075, 0.095, 0.135, 0.075];
const TOOL_CARD_BACKGROUND_COLOR: [f32; 4] = [0.900, 0.640, 0.220, 0.100];
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

fn main() -> Result<()> {
    pollster::block_on(run())
}

async fn run() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(message) = headless_chat_smoke_message(&args) {
        return run_headless_chat_smoke(message);
    }
    let fullscreen = args.iter().any(|arg| arg == "--fullscreen");
    let desktop_mode = desktop_mode_from_args(args.iter().map(String::as_str));
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    let mut window_builder = WindowBuilder::new()
        .with_title("Jcode Desktop")
        .with_inner_size(LogicalSize::new(
            DEFAULT_WINDOW_WIDTH,
            DEFAULT_WINDOW_HEIGHT,
        ));

    if fullscreen {
        window_builder = window_builder.with_fullscreen(Some(Fullscreen::Borderless(None)));
    }

    let window: &'static Window = Box::leak(Box::new(
        window_builder
            .build(&event_loop)
            .context("failed to create desktop window")?,
    ));

    let mut app = if desktop_mode == DesktopMode::WorkspacePrototype {
        let session_cards = load_session_cards_for_desktop();
        let mut workspace = Workspace::from_session_cards(session_cards);
        if let Some(preferences) = load_desktop_preferences() {
            workspace.apply_preferences(preferences);
        }
        DesktopApp::Workspace(workspace)
    } else {
        fresh_single_session_app()
    };
    window.set_title(&app.status_title());
    let mut canvas = Canvas::new(window).await?;
    let mut modifiers = ModifiersState::empty();
    let mut cursor_position = winit::dpi::PhysicalPosition::new(0.0, 0.0);
    let mut selecting_body = false;
    let mut hot_reloader = DesktopHotReloader::new();
    let (session_event_tx, session_event_rx) = mpsc::channel();

    event_loop.run(move |event, target| {
        if app.has_background_work() {
            target.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + BACKGROUND_POLL_INTERVAL,
            ));
        } else {
            target.set_control_flow(ControlFlow::Wait);
        }

        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => {
                    canvas.resize(size);
                    window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    canvas.resize(window.inner_size());
                    window.request_redraw();
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if let Some(lines) = mouse_scroll_lines(delta) {
                        app.scroll_single_session_body(lines);
                        window.request_redraw();
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    cursor_position = position;
                    if selecting_body
                        && app.update_single_session_selection_at(
                            cursor_position.x as f32,
                            cursor_position.y as f32,
                            window.inner_size(),
                        )
                    {
                        window.request_redraw();
                    }
                }
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => match state {
                    ElementState::Pressed => {
                        selecting_body = app.begin_single_session_selection_at(
                            cursor_position.x as f32,
                            cursor_position.y as f32,
                            window.inner_size(),
                        );
                        if selecting_body {
                            window.request_redraw();
                        }
                    }
                    ElementState::Released => {
                        if selecting_body {
                            app.update_single_session_selection_at(
                                cursor_position.x as f32,
                                cursor_position.y as f32,
                                window.inner_size(),
                            );
                            selecting_body = false;
                            let selected = app.selected_single_session_text(window.inner_size());
                            if let Some(text) = selected {
                                copy_text_to_clipboard(&text, &mut app);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                    }
                },
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    let key_input = to_key_input(&event.logical_key, modifiers);
                    if key_input == KeyInput::RefreshSessions && app.is_workspace() {
                        if let DesktopApp::Workspace(workspace) = &mut app {
                            workspace.replace_session_cards(load_session_cards_for_desktop());
                            save_desktop_preferences(workspace);
                        }
                        window.set_title(&app.status_title());
                        window.request_redraw();
                        return;
                    }

                    match app.handle_key(key_input) {
                        KeyOutcome::Exit => target.exit(),
                        KeyOutcome::Redraw => {
                            if let DesktopApp::Workspace(workspace) = &app {
                                save_desktop_preferences(workspace);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::OpenSession { session_id, title } => {
                            if let DesktopApp::Workspace(workspace) = &app {
                                save_desktop_preferences(workspace);
                            }
                            if let Err(error) =
                                session_launch::launch_validated_resume_session(&session_id, &title)
                            {
                                eprintln!(
                                    "jcode-desktop: failed to open session {session_id}: {error:#}"
                                );
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
                                eprintln!("jcode-desktop: failed to spawn session: {error:#}");
                            } else {
                                std::thread::sleep(SESSION_SPAWN_REFRESH_DELAY);
                                app.refresh_sessions();
                                if let DesktopApp::Workspace(workspace) = &app {
                                    save_desktop_preferences(workspace);
                                }
                                window.set_title(&app.status_title());
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
                                        std::thread::sleep(SESSION_SPAWN_REFRESH_DELAY);
                                        app.refresh_sessions();
                                        if let DesktopApp::Workspace(workspace) = &app {
                                            save_desktop_preferences(workspace);
                                        }
                                        window.set_title(&app.status_title());
                                        window.request_redraw();
                                    }
                                    Err(error) => eprintln!(
                                        "jcode-desktop: failed to send image draft to {session_id}: {error:#}"
                                    ),
                                }
                            } else if let Err(error) = session_launch::send_message_to_session(
                                &session_id,
                                &title,
                                &message,
                            ) {
                                eprintln!(
                                    "jcode-desktop: failed to send draft to {session_id}: {error:#}"
                                );
                            } else {
                                std::thread::sleep(SESSION_SPAWN_REFRESH_DELAY);
                                app.refresh_sessions();
                                if let DesktopApp::Workspace(workspace) = &app {
                                    save_desktop_preferences(workspace);
                                }
                                window.set_title(&app.status_title());
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
                            copy_text_to_clipboard(&text, &mut app);
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
                                app.apply_session_event(
                                    session_launch::DesktopSessionEvent::Status(
                                        "switching model".to_string(),
                                    ),
                                );
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
                            app.apply_single_session_switcher_cards(load_session_cards_for_desktop());
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
                                app.apply_session_event(
                                    session_launch::DesktopSessionEvent::Status(
                                        "switching model".to_string(),
                                    ),
                                );
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
                            match clipboard_image_png_base64() {
                                Ok((media_type, base64_data)) => {
                                    app.attach_clipboard_image(media_type, base64_data);
                                }
                                Err(error) => apply_single_session_error(&mut app, error),
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::PasteText => {
                            if let Err(error) = paste_clipboard_into_app(&mut app) {
                                apply_single_session_error(&mut app, error);
                            }
                            window.set_title(&app.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::None => {}
                    }
                }
                WindowEvent::RedrawRequested => match canvas
                    .render(&app, window.current_monitor().map(|monitor| monitor.size()))
                {
                    Ok(animation_active) => {
                        if animation_active {
                            window.request_redraw();
                        }
                    }
                    Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                        canvas.resize(window.inner_size());
                        window.request_redraw();
                    }
                    Err(SurfaceError::OutOfMemory) => target.exit(),
                    Err(SurfaceError::Timeout) => {
                        window.request_redraw();
                    }
                },
                _ => {}
            },
            Event::AboutToWait => {
                if apply_pending_session_events(&mut app, &session_event_rx) {
                    if let Some(session_id) = app.single_session_live_id() {
                        attach_single_session_by_id(&mut app, &session_id);
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
                    window.request_redraw();
                }

                if let Some(relaunch) = hot_reloader.poll() {
                    if let Err(error) = relaunch.spawn() {
                        eprintln!("jcode-desktop: failed to hot reload desktop: {error:#}");
                    } else {
                        target.exit();
                        return;
                    }
                }

                if canvas.needs_initial_frame {
                    canvas.needs_initial_frame = false;
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
            eprintln!("jcode-desktop: failed to load session metadata: {error:#}");
            Vec::new()
        }
    }
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
            session_launch::DesktopSessionEvent::ToolStarted { name } => {
                last_status = Some(format!("using tool {name}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_started", "name": name})
                );
            }
            session_launch::DesktopSessionEvent::ToolFinished {
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

fn load_desktop_preferences() -> Option<workspace::DesktopPreferences> {
    match desktop_prefs::load_preferences() {
        Ok(preferences) => preferences,
        Err(error) => {
            eprintln!("jcode-desktop: failed to load desktop preferences: {error:#}");
            None
        }
    }
}

fn save_desktop_preferences(workspace: &Workspace) {
    if let Err(error) = desktop_prefs::save_preferences(&workspace.preferences()) {
        eprintln!("jcode-desktop: failed to save desktop preferences: {error:#}");
    }
}

fn load_primary_session_card() -> Option<workspace::SessionCard> {
    load_session_cards_for_desktop().into_iter().next()
}

fn fresh_single_session_app() -> DesktopApp {
    DesktopApp::SingleSession(SingleSessionApp::new(None))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DesktopMode {
    SingleSession,
    WorkspacePrototype,
}

fn desktop_mode_from_args<'a>(args: impl IntoIterator<Item = &'a str>) -> DesktopMode {
    if args.into_iter().any(|arg| arg == "--workspace") {
        DesktopMode::WorkspacePrototype
    } else {
        DesktopMode::SingleSession
    }
}

fn attach_single_session_by_id(app: &mut DesktopApp, session_id: &str) {
    let Some(card) = load_session_cards_for_desktop()
        .into_iter()
        .find(|card| card.session_id == session_id)
    else {
        return;
    };

    if let DesktopApp::SingleSession(single_session) = app {
        single_session.replace_session(Some(card));
    }
}

struct DesktopHotReloader {
    relaunch: Option<DesktopRelaunch>,
    initial_modified: Option<std::time::SystemTime>,
    last_checked: Instant,
}

impl DesktopHotReloader {
    const CHECK_INTERVAL: Duration = Duration::from_millis(750);

    fn new() -> Self {
        let relaunch = DesktopRelaunch::from_current_process();
        let initial_modified = relaunch
            .as_ref()
            .and_then(|relaunch| binary_modified_time(&relaunch.binary));
        Self {
            relaunch,
            initial_modified,
            last_checked: Instant::now(),
        }
    }

    fn poll(&mut self) -> Option<DesktopRelaunch> {
        if self.last_checked.elapsed() < Self::CHECK_INTERVAL {
            return None;
        }
        self.last_checked = Instant::now();

        let relaunch = self.relaunch.as_ref()?;
        let initial_modified = self.initial_modified?;
        let current_modified = binary_modified_time(&relaunch.binary)?;
        if current_modified > initial_modified {
            self.initial_modified = Some(current_modified);
            return Some(relaunch.clone());
        }
        None
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

    fn spawn(&self) -> Result<()> {
        Command::new(&self.binary)
            .args(&self.args)
            .spawn()
            .with_context(|| format!("failed to spawn {}", self.binary.display()))?;
        Ok(())
    }
}

fn binary_modified_time(path: &Path) -> Option<std::time::SystemTime> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return None,
    };
    match metadata.modified() {
        Ok(modified) => Some(modified),
        Err(_) => None,
    }
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
    fn is_single_session(&self) -> bool {
        matches!(self, Self::SingleSession(_))
    }

    fn is_workspace(&self) -> bool {
        matches!(self, Self::Workspace(_))
    }

    fn has_background_work(&self) -> bool {
        matches!(self, Self::SingleSession(app) if app.has_background_work())
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

    fn refresh_sessions(&mut self) {
        match self {
            Self::SingleSession(app) => app.replace_session(load_primary_session_card()),
            Self::Workspace(workspace) => {
                workspace.replace_session_cards(load_session_cards_for_desktop())
            }
        }
    }

    fn apply_session_event(&mut self, event: session_launch::DesktopSessionEvent) {
        if let Self::SingleSession(app) = self {
            app.apply_session_event(event);
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

    fn selected_single_session_text(&mut self, size: PhysicalSize<u32>) -> Option<String> {
        if let Self::SingleSession(app) = self {
            let lines = single_session_visible_body(app, size);
            let selected = app.selected_text_from_lines(&lines);
            app.clear_selection();
            return selected;
        }
        None
    }

    fn scroll_single_session_body(&mut self, lines: i32) {
        if let Self::SingleSession(app) = self {
            app.scroll_body_lines(lines);
        }
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

fn to_key_input(key: &Key, modifiers: ModifiersState) -> KeyInput {
    match key {
        Key::Named(NamedKey::Escape) => KeyInput::Escape,
        Key::Named(NamedKey::Enter) if modifiers.control_key() => KeyInput::QueueDraft,
        Key::Named(NamedKey::Enter) if modifiers.shift_key() => KeyInput::Enter,
        Key::Named(NamedKey::Enter) => KeyInput::SubmitDraft,
        Key::Named(NamedKey::Backspace) if modifiers.control_key() => KeyInput::DeletePreviousWord,
        Key::Named(NamedKey::Backspace) => KeyInput::Backspace,
        Key::Named(NamedKey::Delete) => KeyInput::DeleteNextChar,
        Key::Named(NamedKey::PageUp) => KeyInput::ScrollBodyPages(1),
        Key::Named(NamedKey::PageDown) => KeyInput::ScrollBodyPages(-1),
        Key::Named(NamedKey::ArrowUp) if modifiers.alt_key() => KeyInput::JumpPrompt(-1),
        Key::Named(NamedKey::ArrowDown) if modifiers.alt_key() => KeyInput::JumpPrompt(1),
        Key::Named(NamedKey::ArrowUp) => KeyInput::ModelPickerMove(-1),
        Key::Named(NamedKey::ArrowDown) => KeyInput::ModelPickerMove(1),
        Key::Named(NamedKey::ArrowLeft) => KeyInput::MoveCursorLeft,
        Key::Named(NamedKey::ArrowRight) => KeyInput::MoveCursorRight,
        Key::Named(NamedKey::Home) => KeyInput::MoveToLineStart,
        Key::Named(NamedKey::End) => KeyInput::MoveToLineEnd,
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("a") => {
            KeyInput::MoveToLineStart
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("e") => {
            KeyInput::MoveToLineEnd
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("u") => {
            KeyInput::DeleteToLineStart
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("k") => {
            KeyInput::DeleteToLineEnd
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("z") => {
            KeyInput::UndoInput
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("c") =>
        {
            KeyInput::CopyLatestResponse
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("c") => {
            KeyInput::CancelGeneration
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
        Key::Character(text) if modifiers.control_key() && text == ";" => KeyInput::SpawnPanel,
        Key::Character(text) if modifiers.control_key() && (text == "?" || text == "/") => {
            KeyInput::HotkeyHelp
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
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("v") => {
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

fn apply_pending_session_events(
    app: &mut DesktopApp,
    session_event_rx: &mpsc::Receiver<session_launch::DesktopSessionEvent>,
) -> bool {
    let mut changed = false;
    while let Ok(event) = session_event_rx.try_recv() {
        app.apply_session_event(event);
        changed = true;
    }
    changed
}

fn apply_single_session_error(app: &mut DesktopApp, error: anyhow::Error) {
    app.apply_session_event(session_launch::DesktopSessionEvent::Error(format!(
        "{error:#}"
    )));
}

fn copy_text_to_clipboard(text: &str, app: &mut DesktopApp) {
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.to_string())) {
        Ok(()) => app.apply_session_event(session_launch::DesktopSessionEvent::Status(
            "copied latest response".to_string(),
        )),
        Err(error) => app.apply_session_event(session_launch::DesktopSessionEvent::Error(format!(
            "failed to copy latest response: {error}"
        ))),
    }
}

fn paste_clipboard_into_app(app: &mut DesktopApp) -> Result<()> {
    match clipboard_text() {
        Ok(text) => {
            app.paste_text(&text);
            Ok(())
        }
        Err(text_error) if app.accepts_clipboard_image_paste() => {
            match clipboard_image_png_base64() {
                Ok((media_type, base64_data)) => {
                    app.attach_clipboard_image(media_type, base64_data);
                    Ok(())
                }
                Err(image_error) => Err(anyhow::anyhow!(
                    "clipboard contains neither pasteable text nor image: text: {text_error}; image: {image_error}"
                )),
            }
        }
        Err(error) => Err(error),
    }
}

fn clipboard_image_png_base64() -> Result<(String, String)> {
    let mut clipboard = arboard::Clipboard::new().context("failed to access clipboard")?;
    let image = clipboard
        .get_image()
        .context("clipboard does not contain an image")?;
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

fn clipboard_text() -> Result<String> {
    arboard::Clipboard::new()
        .context("failed to access clipboard")?
        .get_text()
        .context("clipboard does not contain text")
}

fn mouse_scroll_lines(delta: MouseScrollDelta) -> Option<i32> {
    let lines = match delta {
        MouseScrollDelta::LineDelta(_, y) => (y * 3.0).round() as i32,
        MouseScrollDelta::PixelDelta(position) => (position.y / 40.0).round() as i32,
    };
    (lines != 0).then_some(lines)
}

struct Canvas<'window> {
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    font_system: FontSystem,
    swash_cache: SwashCache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    size: PhysicalSize<u32>,
    viewport_animation: AnimatedViewport,
    focus_pulse: FocusPulse,
    needs_initial_frame: bool,
    single_session_text_key: Option<SingleSessionTextKey>,
    single_session_text_buffers: Vec<Buffer>,
}

impl<'window> Canvas<'window> {
    async fn new(window: &'window Window) -> Result<Self> {
        let size = non_zero_size(window.inner_size());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window)
            .context("failed to create wgpu surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to find a compatible GPU adapter")?;
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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("jcode-desktop-primitive-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("jcode-desktop-primitive-pipeline-layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let mut text_atlas = TextAtlas::new(&device, &queue, format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &device,
            wgpu::MultisampleState::default(),
            None,
        );

        Ok(Self {
            surface,
            device,
            queue,
            config,
            render_pipeline,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            text_atlas,
            text_renderer,
            size,
            viewport_animation: AnimatedViewport::default(),
            focus_pulse: FocusPulse::default(),
            needs_initial_frame: true,
            single_session_text_key: None,
            single_session_text_buffers: Vec::new(),
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let size = non_zero_size(size);
        if self.size == size {
            return;
        }

        self.size = size;
        self.single_session_text_key = None;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    fn refresh_cached_single_session_text_buffers(&mut self, app: &SingleSessionApp) {
        let key = single_session_text_key(app, self.size);
        if self.single_session_text_key.as_ref() != Some(&key) {
            self.single_session_text_buffers =
                single_session_text_buffers_from_key(&key, self.size, &mut self.font_system);
            self.single_session_text_key = Some(key);
        }
    }

    fn render(
        &mut self,
        app: &DesktopApp,
        monitor_size: Option<PhysicalSize<u32>>,
    ) -> std::result::Result<bool, SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-desktop-render-workspace"),
            });
        let now = Instant::now();
        let (mut vertices, animation_active) = match app {
            DesktopApp::SingleSession(single_session) => {
                let focus_pulse = self.focus_pulse.frame(1, now);
                let animation_active = self.focus_pulse.is_animating();
                (
                    build_single_session_vertices(single_session, self.size, focus_pulse),
                    animation_active,
                )
            }
            DesktopApp::Workspace(workspace) => {
                let target_layout = workspace_render_layout(workspace, self.size, monitor_size);
                let render_layout = self.viewport_animation.frame(target_layout, now);
                let focus_pulse = self.focus_pulse.frame(workspace.focused_id, now);
                let animation_active =
                    self.viewport_animation.is_animating() || self.focus_pulse.is_animating();
                (
                    build_vertices(workspace, self.size, render_layout, focus_pulse),
                    animation_active,
                )
            }
        };
        if let DesktopApp::SingleSession(single_session) = app {
            self.refresh_cached_single_session_text_buffers(single_session);
        } else {
            self.single_session_text_key = None;
            self.single_session_text_buffers.clear();
        }
        let text_buffers = &self.single_session_text_buffers;
        if let DesktopApp::SingleSession(single_session) = app {
            push_single_session_caret(
                &mut vertices,
                single_session,
                self.size,
                text_buffers.get(2),
            );
        }
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("jcode-desktop-workspace-vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let text_areas = single_session_text_areas(&text_buffers, self.size);
        if !text_areas.is_empty() {
            if let Err(error) = self.text_renderer.prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.text_atlas,
                Resolution {
                    width: self.config.width,
                    height: self.config.height,
                },
                text_areas,
                &mut self.swash_cache,
            ) {
                eprintln!("jcode-desktop: failed to prepare text: {error:?}");
            }
        }

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
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.draw(0..vertices.len() as u32, 0..1);
            if !text_buffers.is_empty()
                && let Err(error) = self
                    .text_renderer
                    .render(&self.text_atlas, &mut render_pass)
            {
                eprintln!("jcode-desktop: failed to render text: {error:?}");
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(animation_active)
    }
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

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn build_vertices(
    workspace: &Workspace,
    size: PhysicalSize<u32>,
    render_layout: WorkspaceRenderLayout,
    focus_pulse: f32,
) -> Vec<Vertex> {
    let width = size.width as f32;
    let height = size.height as f32;
    let mut vertices = Vec::new();

    push_gradient_rect(
        &mut vertices,
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

    let status_color = match workspace.mode {
        InputMode::Navigation => NAV_STATUS_COLOR,
        InputMode::Insert => INSERT_STATUS_COLOR,
    };
    let status_rect = Rect {
        x: OUTER_PADDING,
        y: OUTER_PADDING,
        width: (width - OUTER_PADDING * 2.0).max(1.0),
        height: STATUS_BAR_HEIGHT,
    };
    push_rounded_rect(
        &mut vertices,
        status_rect,
        STATUS_RADIUS,
        status_color,
        size,
    );

    let active_workspace = workspace.current_workspace();
    let visible_layout = render_layout.visible;
    push_workspace_number(&mut vertices, active_workspace, status_rect, size);
    push_status_preview(
        &mut vertices,
        workspace,
        active_workspace,
        visible_layout,
        status_rect,
        size,
    );
    push_status_text(&mut vertices, workspace, status_rect, size);

    if workspace.zoomed {
        if let Some(surface) = workspace.focused_surface() {
            let rect = Rect {
                x: OUTER_PADDING,
                y: STATUS_BAR_HEIGHT + OUTER_PADDING * 2.0,
                width: (width - OUTER_PADDING * 2.0).max(1.0),
                height: (height - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0),
            };
            push_surface(
                &mut vertices,
                rect,
                surface.color_index,
                true,
                focus_pulse,
                size,
            );
            let draft = focused_panel_draft(workspace, surface.id);
            push_panel_contents(
                &mut vertices,
                surface,
                rect,
                size,
                true,
                workspace.detail_scroll,
                draft.as_deref(),
            );
        }
        return vertices;
    }

    let workspace_height = (height - STATUS_BAR_HEIGHT - OUTER_PADDING * 3.0).max(1.0);
    let workspace_top = STATUS_BAR_HEIGHT + OUTER_PADDING * 2.0;
    let lane_pitch = workspace_height + GAP;
    let column_width = render_layout.column_width;
    let scroll_offset = render_layout.scroll_offset;
    let vertical_scroll_offset = render_layout.vertical_scroll_offset;

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
        let focused = workspace.is_focused(surface.id);
        let surface_pulse = if focused { focus_pulse } else { 0.0 };
        push_surface(
            &mut vertices,
            rect,
            surface.color_index,
            focused,
            surface_pulse,
            size,
        );
        let draft = focused_panel_draft(workspace, surface.id);
        push_panel_contents(
            &mut vertices,
            surface,
            rect,
            size,
            false,
            0,
            draft.as_deref(),
        );
    }

    vertices
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
) {
    let mode = match workspace.mode {
        InputMode::Navigation => "NAV",
        InputMode::Insert => "INS",
    };
    let panel_percent = (workspace.preferred_panel_screen_fraction() * 100.0).round() as u32;
    let text = format!("{mode} P{panel_percent}");
    let text_width = bitmap_text_width(&text, BITMAP_TEXT_PIXEL);
    let x = status_rect.x + status_rect.width - STATUS_TEXT_RIGHT_PADDING - text_width;
    let y = status_rect.y + (status_rect.height - bitmap_text_height(BITMAP_TEXT_PIXEL)) / 2.0;
    if x > status_rect.x {
        push_bitmap_text(
            vertices,
            &text,
            x,
            y,
            BITMAP_TEXT_PIXEL,
            STATUS_TEXT_COLOR,
            size,
            text_width,
        );
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
