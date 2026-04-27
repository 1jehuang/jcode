mod desktop_prefs;
mod render_helpers;
mod session_data;
mod session_launch;
mod workspace;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use render_helpers::*;
use wgpu::util::DeviceExt;
use wgpu::{CompositeAlphaMode, PresentMode, SurfaceError, TextureUsages};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowBuilder};
use workspace::{InputMode, KeyInput, KeyOutcome, PanelSizePreset, Workspace};

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
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
const VIEWPORT_ANIMATION_DURATION: Duration = Duration::from_millis(150);
const FOCUS_PULSE_DURATION: Duration = Duration::from_millis(180);
const VIEWPORT_ANIMATION_EPSILON: f32 = 0.5;
const SESSION_SPAWN_REFRESH_DELAY: Duration = Duration::from_millis(350);

const SINGLE_SESSION_FONT_FAMILY: &str = "JetBrainsMono Nerd Font";
const SINGLE_SESSION_FONT_WEIGHT: &str = "Light";
const SINGLE_SESSION_FONT_FALLBACKS: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrains Mono",
    "monospace",
];
const SINGLE_SESSION_TITLE_FONT_SIZE: f32 = 18.0;
const SINGLE_SESSION_BODY_FONT_SIZE: f32 = 15.0;
const SINGLE_SESSION_META_FONT_SIZE: f32 = 12.0;
const SINGLE_SESSION_CODE_FONT_SIZE: f32 = 14.0;
const SINGLE_SESSION_BODY_LINE_HEIGHT: f32 = 1.45;
const SINGLE_SESSION_CODE_LINE_HEIGHT: f32 = 1.35;
const SINGLE_SESSION_META_LINE_HEIGHT: f32 = 1.25;

#[derive(Clone, Copy, Debug)]
struct SingleSessionTypography {
    family: &'static str,
    weight: &'static str,
    fallbacks: &'static [&'static str],
    title_size: f32,
    body_size: f32,
    meta_size: f32,
    code_size: f32,
    body_line_height: f32,
    code_line_height: f32,
    meta_line_height: f32,
}

const fn single_session_typography() -> SingleSessionTypography {
    SingleSessionTypography {
        family: SINGLE_SESSION_FONT_FAMILY,
        weight: SINGLE_SESSION_FONT_WEIGHT,
        fallbacks: SINGLE_SESSION_FONT_FALLBACKS,
        title_size: SINGLE_SESSION_TITLE_FONT_SIZE,
        body_size: SINGLE_SESSION_BODY_FONT_SIZE,
        meta_size: SINGLE_SESSION_META_FONT_SIZE,
        code_size: SINGLE_SESSION_CODE_FONT_SIZE,
        body_line_height: SINGLE_SESSION_BODY_LINE_HEIGHT,
        code_line_height: SINGLE_SESSION_CODE_LINE_HEIGHT,
        meta_line_height: SINGLE_SESSION_META_LINE_HEIGHT,
    }
}

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
const PANEL_TITLE_COLOR: [f32; 4] = [0.150, 0.170, 0.210, 0.68];
const PANEL_BODY_COLOR: [f32; 4] = [0.150, 0.170, 0.210, 0.48];
const PANEL_SECTION_COLOR: [f32; 4] = [0.150, 0.170, 0.210, 0.62];
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
    let fullscreen = std::env::args().any(|arg| arg == "--fullscreen");
    let workspace_mode = std::env::args().any(|arg| arg == "--workspace");
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

    let mut app = if workspace_mode {
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
    let mut hot_reloader = DesktopHotReloader::new();

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Wait);

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
                        } => {
                            if let Err(error) = session_launch::send_message_to_session(
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
        let binary = resolve_invoked_binary(&argv0).or_else(|| std::env::current_exe().ok())?;
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
    path.metadata().ok()?.modified().ok()
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

impl DesktopApp {
    fn is_workspace(&self) -> bool {
        matches!(self, Self::Workspace(_))
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
}

#[derive(Clone, Debug)]
struct SingleSessionApp {
    session: Option<workspace::SessionCard>,
    draft: String,
    detail_scroll: usize,
}

impl SingleSessionApp {
    fn new(session: Option<workspace::SessionCard>) -> Self {
        Self {
            session,
            draft: String::new(),
            detail_scroll: 0,
        }
    }

    fn replace_session(&mut self, session: Option<workspace::SessionCard>) {
        self.session = session;
        self.detail_scroll = 0;
    }

    fn reset_fresh_session(&mut self) {
        self.session = None;
        self.draft.clear();
        self.detail_scroll = 0;
    }

    fn status_title(&self) -> String {
        let title = self
            .session
            .as_ref()
            .map(|session| session.title.as_str())
            .unwrap_or("fresh session");
        format!(
            "Jcode Desktop · single session · {title} · Ctrl+Enter send · Enter newline · Ctrl+; spawn · Ctrl+R refresh · Esc quit · --workspace for Niri layout"
        )
    }

    fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::SpawnPanel => KeyOutcome::SpawnSession,
            KeyInput::RefreshSessions => KeyOutcome::Redraw,
            KeyInput::SubmitDraft => self.submit_draft(),
            KeyInput::Escape => KeyOutcome::Exit,
            KeyInput::Enter => {
                self.draft.push('\n');
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                self.draft.pop();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.draft.push_str(&text);
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() {
            return KeyOutcome::None;
        }
        let Some(session) = &self.session else {
            return KeyOutcome::None;
        };
        let session_id = session.session_id.clone();
        let title = session.title.clone();
        self.draft.clear();
        KeyOutcome::SendDraft {
            session_id,
            title,
            message,
        }
    }
}

fn to_key_input(key: &Key, modifiers: ModifiersState) -> KeyInput {
    match key {
        Key::Named(NamedKey::Escape) => KeyInput::Escape,
        Key::Named(NamedKey::Enter) if modifiers.control_key() => KeyInput::SubmitDraft,
        Key::Named(NamedKey::Enter) => KeyInput::Enter,
        Key::Named(NamedKey::Backspace) => KeyInput::Backspace,
        Key::Character(text) if modifiers.control_key() && text == ";" => KeyInput::SpawnPanel,
        Key::Character(text) if modifiers.control_key() && (text == "?" || text == "/") => {
            KeyInput::HotkeyHelp
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("r") => {
            KeyInput::RefreshSessions
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
        Key::Character(text) => KeyInput::Character(text.to_string()),
        _ => KeyInput::Other,
    }
}

struct Canvas<'window> {
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    size: PhysicalSize<u32>,
    viewport_animation: AnimatedViewport,
    focus_pulse: FocusPulse,
    needs_initial_frame: bool,
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

        Ok(Self {
            surface,
            device,
            queue,
            config,
            render_pipeline,
            size,
            viewport_animation: AnimatedViewport::default(),
            focus_pulse: FocusPulse::default(),
            needs_initial_frame: true,
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let size = non_zero_size(size);
        if self.size == size {
            return;
        }

        self.size = size;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
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
        let (vertices, animation_active) = match app {
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
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("jcode-desktop-workspace-vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

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

#[derive(Clone, Copy)]
struct VisibleColumnLayout {
    visible_columns: u32,
    first_visible_column: i32,
}

#[derive(Clone, Copy)]
struct WorkspaceRenderLayout {
    visible: VisibleColumnLayout,
    column_width: f32,
    scroll_offset: f32,
    vertical_scroll_offset: f32,
}

#[derive(Default)]
struct AnimatedViewport {
    initialized: bool,
    start_column_width: f32,
    start_scroll_offset: f32,
    start_vertical_scroll_offset: f32,
    current_column_width: f32,
    current_scroll_offset: f32,
    current_vertical_scroll_offset: f32,
    target_column_width: f32,
    target_scroll_offset: f32,
    target_vertical_scroll_offset: f32,
    started_at: Option<Instant>,
}

impl AnimatedViewport {
    fn frame(&mut self, target: WorkspaceRenderLayout, now: Instant) -> WorkspaceRenderLayout {
        if !self.initialized {
            self.initialized = true;
            self.current_column_width = target.column_width;
            self.current_scroll_offset = target.scroll_offset;
            self.current_vertical_scroll_offset = target.vertical_scroll_offset;
            self.target_column_width = target.column_width;
            self.target_scroll_offset = target.scroll_offset;
            self.target_vertical_scroll_offset = target.vertical_scroll_offset;
            return target;
        }

        if has_layout_target_changed(self.target_column_width, target.column_width)
            || has_layout_target_changed(self.target_scroll_offset, target.scroll_offset)
            || has_layout_target_changed(
                self.target_vertical_scroll_offset,
                target.vertical_scroll_offset,
            )
        {
            self.start_column_width = self.current_column_width;
            self.start_scroll_offset = self.current_scroll_offset;
            self.start_vertical_scroll_offset = self.current_vertical_scroll_offset;
            self.target_column_width = target.column_width;
            self.target_scroll_offset = target.scroll_offset;
            self.target_vertical_scroll_offset = target.vertical_scroll_offset;
            self.started_at = Some(now);
        }

        if let Some(started_at) = self.started_at {
            let progress =
                (now - started_at).as_secs_f32() / VIEWPORT_ANIMATION_DURATION.as_secs_f32();
            let progress = progress.clamp(0.0, 1.0);
            let eased = ease_out_cubic(progress);
            self.current_column_width =
                lerp(self.start_column_width, self.target_column_width, eased);
            self.current_scroll_offset =
                lerp(self.start_scroll_offset, self.target_scroll_offset, eased);
            self.current_vertical_scroll_offset = lerp(
                self.start_vertical_scroll_offset,
                self.target_vertical_scroll_offset,
                eased,
            );

            if progress >= 1.0 {
                self.current_column_width = self.target_column_width;
                self.current_scroll_offset = self.target_scroll_offset;
                self.current_vertical_scroll_offset = self.target_vertical_scroll_offset;
                self.started_at = None;
            }
        }

        WorkspaceRenderLayout {
            visible: target.visible,
            column_width: self.current_column_width,
            scroll_offset: self.current_scroll_offset,
            vertical_scroll_offset: self.current_vertical_scroll_offset,
        }
    }

    fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }
}

#[derive(Default)]
struct FocusPulse {
    last_focused_id: Option<u64>,
    started_at: Option<Instant>,
}

impl FocusPulse {
    fn frame(&mut self, focused_id: u64, now: Instant) -> f32 {
        match self.last_focused_id {
            None => {
                self.last_focused_id = Some(focused_id);
                return 0.0;
            }
            Some(last_focused_id) if last_focused_id != focused_id => {
                self.last_focused_id = Some(focused_id);
                self.started_at = Some(now);
            }
            Some(_) => {}
        }

        let Some(started_at) = self.started_at else {
            return 0.0;
        };
        let progress =
            ((now - started_at).as_secs_f32() / FOCUS_PULSE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
        if progress >= 1.0 {
            self.started_at = None;
            return 0.0;
        }

        1.0 - ease_out_cubic(progress)
    }

    fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }
}

fn has_layout_target_changed(previous: f32, next: f32) -> bool {
    (previous - next).abs() > VIEWPORT_ANIMATION_EPSILON
}

fn ease_out_cubic(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(3)
}

fn lerp(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

fn build_single_session_vertices(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
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

    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: width.max(1.0),
        height: height.max(1.0),
    };
    let typography = single_session_typography();
    let _ = (
        typography.family,
        typography.weight,
        typography.fallbacks,
        typography.title_size,
        typography.body_size,
        typography.meta_size,
        typography.code_size,
        typography.body_line_height,
        typography.code_line_height,
        typography.meta_line_height,
    );
    let surface = single_session_surface(app.session.as_ref());
    push_surface(
        &mut vertices,
        rect,
        surface.color_index,
        true,
        focus_pulse,
        size,
    );
    let draft = if !app.draft.trim().is_empty() {
        Some(app.draft.trim())
    } else {
        None
    };
    push_panel_contents(
        &mut vertices,
        &surface,
        rect,
        size,
        true,
        app.detail_scroll,
        draft,
    );

    vertices
}

fn single_session_surface(session: Option<&workspace::SessionCard>) -> workspace::Surface {
    let lines = single_session_lines(session);
    workspace::Surface {
        id: 1,
        title: session
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "new jcode session".to_string()),
        body_lines: lines.clone(),
        detail_lines: lines,
        session_id: session.map(|session| session.session_id.clone()),
        lane: 0,
        column: 0,
        color_index: 0,
    }
}

fn single_session_lines(session: Option<&workspace::SessionCard>) -> Vec<String> {
    let Some(session) = session else {
        return vec![
            "single session mode".to_string(),
            "fresh desktop-native session draft".to_string(),
            "type here without nav or insert modes".to_string(),
            "ctrl+enter will send once desktop-native execution is connected".to_string(),
            "ctrl+; clears this draft and starts another fresh desktop session".to_string(),
            "run with --workspace for the niri layout wrapper".to_string(),
        ];
    };

    let mut lines = vec![
        "single session mode".to_string(),
        session.subtitle.clone(),
        session.detail.clone(),
    ];
    if !session.preview_lines.is_empty() {
        lines.push("recent transcript".to_string());
        lines.extend(session.preview_lines.clone());
    }
    if !session.detail_lines.is_empty() {
        lines.push("expanded transcript".to_string());
        lines.extend(session.detail_lines.clone());
    }
    lines
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
mod tests {
    use super::*;

    #[test]
    fn quarter_size_preset_follows_quarter_screen_width_steps() {
        let monitor_width = Some(2000);

        assert_eq!(inferred_visible_column_count(500, monitor_width, 0.25), 1);
        assert_eq!(inferred_visible_column_count(1000, monitor_width, 0.25), 2);
        assert_eq!(inferred_visible_column_count(1500, monitor_width, 0.25), 3);
        assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.25), 4);
    }

    #[test]
    fn preferred_panel_size_limits_visible_column_count() {
        let monitor_width = Some(2000);

        assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.25), 4);
        assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.50), 2);
        assert_eq!(inferred_visible_column_count(2000, monitor_width, 0.75), 1);
        assert_eq!(inferred_visible_column_count(2000, monitor_width, 1.00), 1);

        assert_eq!(inferred_visible_column_count(500, monitor_width, 0.25), 1);
        assert_eq!(inferred_visible_column_count(500, monitor_width, 1.00), 1);
    }

    #[test]
    fn visible_column_count_tolerates_window_manager_gaps() {
        let monitor_width = Some(2000);

        assert_eq!(inferred_visible_column_count(1940, monitor_width, 0.25), 4);
        assert_eq!(inferred_visible_column_count(970, monitor_width, 0.25), 2);
        assert_eq!(inferred_visible_column_count(1940, monitor_width, 0.50), 2);
    }

    #[test]
    fn visible_column_count_is_clamped_and_safe_without_monitor() {
        assert_eq!(inferred_visible_column_count(1, Some(2000), 0.25), 1);
        assert_eq!(inferred_visible_column_count(3000, Some(2000), 0.25), 4);
        assert_eq!(inferred_visible_column_count(1000, Some(0), 0.25), 1);
        assert_eq!(inferred_visible_column_count(1000, None, 0.25), 1);
    }

    #[test]
    fn viewport_animation_interpolates_to_new_layout_target() {
        let mut animation = AnimatedViewport::default();
        let now = Instant::now();
        let visible = VisibleColumnLayout {
            visible_columns: 2,
            first_visible_column: 0,
        };
        let start = WorkspaceRenderLayout {
            visible,
            column_width: 200.0,
            scroll_offset: 0.0,
            vertical_scroll_offset: 0.0,
        };
        let target = WorkspaceRenderLayout {
            visible: VisibleColumnLayout {
                visible_columns: 2,
                first_visible_column: 2,
            },
            column_width: 300.0,
            scroll_offset: 600.0,
            vertical_scroll_offset: 800.0,
        };

        let first_frame = animation.frame(start, now);
        assert_eq!(first_frame.column_width, 200.0);
        assert_eq!(first_frame.scroll_offset, 0.0);
        assert_eq!(first_frame.vertical_scroll_offset, 0.0);
        assert!(!animation.is_animating());

        let transition_start = animation.frame(target, now);
        assert_eq!(transition_start.column_width, 200.0);
        assert_eq!(transition_start.scroll_offset, 0.0);
        assert_eq!(transition_start.vertical_scroll_offset, 0.0);
        assert!(animation.is_animating());

        let middle = animation.frame(target, now + VIEWPORT_ANIMATION_DURATION / 2);
        assert!(middle.column_width > 200.0);
        assert!(middle.column_width < 300.0);
        assert!(middle.scroll_offset > 0.0);
        assert!(middle.scroll_offset < 600.0);
        assert!(middle.vertical_scroll_offset > 0.0);
        assert!(middle.vertical_scroll_offset < 800.0);

        let final_frame = animation.frame(target, now + VIEWPORT_ANIMATION_DURATION);
        assert_eq!(final_frame.column_width, 300.0);
        assert_eq!(final_frame.scroll_offset, 600.0);
        assert_eq!(final_frame.vertical_scroll_offset, 800.0);
        assert!(!animation.is_animating());
    }

    #[test]
    fn focus_pulse_runs_when_focused_surface_changes() {
        let mut pulse = FocusPulse::default();
        let now = Instant::now();

        assert_eq!(pulse.frame(1, now), 0.0);
        assert!(!pulse.is_animating());

        let start = pulse.frame(2, now);
        assert!(start > 0.0);
        assert!(pulse.is_animating());

        let middle = pulse.frame(2, now + FOCUS_PULSE_DURATION / 2);
        assert!(middle > 0.0);
        assert!(middle < start);

        let end = pulse.frame(2, now + FOCUS_PULSE_DURATION);
        assert_eq!(end, 0.0);
        assert!(!pulse.is_animating());
    }

    #[test]
    fn bitmap_text_normalization_sanitizes_panel_titles() {
        assert_eq!(
            normalize_bitmap_text("fox · coordinator"),
            "FOX COORDINATOR"
        );
        assert_eq!(normalize_bitmap_text("agent-12"), "AGENT-12");
        assert_eq!(bitmap_text_width("NAV", 2.0), 34.0);
    }

    #[test]
    fn bitmap_text_wrapping_breaks_on_words() {
        assert_eq!(
            wrap_bitmap_text("ONE TWO THREE", 1.0, bitmap_char_advance(1.0) * 7.0),
            vec!["ONE TWO", "THREE"]
        );
    }

    #[test]
    fn bitmap_text_wrapping_splits_long_words() {
        assert_eq!(
            wrap_bitmap_text("ABCDEFGHI", 1.0, bitmap_char_advance(1.0) * 4.0),
            vec!["ABCD", "EFGH", "I"]
        );
    }

    #[test]
    fn single_session_typography_targets_jetbrains_mono_light_nerd() {
        assert_eq!(SINGLE_SESSION_FONT_FAMILY, "JetBrainsMono Nerd Font");
        assert_eq!(SINGLE_SESSION_FONT_WEIGHT, "Light");
        assert!(SINGLE_SESSION_FONT_FALLBACKS.contains(&"monospace"));
        assert!(SINGLE_SESSION_TITLE_FONT_SIZE > SINGLE_SESSION_BODY_FONT_SIZE);
        assert!(SINGLE_SESSION_BODY_FONT_SIZE > SINGLE_SESSION_META_FONT_SIZE);
        assert!(SINGLE_SESSION_CODE_FONT_SIZE <= SINGLE_SESSION_BODY_FONT_SIZE);
        assert!(SINGLE_SESSION_BODY_LINE_HEIGHT > SINGLE_SESSION_CODE_LINE_HEIGHT);
        assert!(SINGLE_SESSION_CODE_LINE_HEIGHT > SINGLE_SESSION_META_LINE_HEIGHT);
    }

    #[test]
    fn single_session_without_session_is_native_fresh_draft() {
        let mut app = SingleSessionApp::new(None);

        assert!(app.status_title().contains("single session"));
        assert_eq!(
            app.handle_key(KeyInput::SpawnPanel),
            KeyOutcome::SpawnSession
        );
        assert!(
            single_session_lines(None)
                .iter()
                .any(|line| line.contains("desktop-native"))
        );
    }

    #[test]
    fn default_single_session_app_starts_without_attaching_recent_session() {
        let DesktopApp::SingleSession(mut app) = fresh_single_session_app() else {
            panic!("default desktop app should be single-session mode");
        };

        assert!(app.session.is_none());
        assert_eq!(
            app.handle_key(KeyInput::SpawnPanel),
            KeyOutcome::SpawnSession
        );
    }

    #[test]
    fn single_session_spawn_resets_to_fresh_native_draft() {
        let card = workspace::SessionCard {
            session_id: "session_alpha".to_string(),
            title: "alpha".to_string(),
            subtitle: "active".to_string(),
            detail: "3 msgs".to_string(),
            preview_lines: Vec::new(),
            detail_lines: Vec::new(),
        };
        let mut app = SingleSessionApp::new(Some(card));
        app.handle_key(KeyInput::Character("draft".to_string()));

        app.reset_fresh_session();

        assert!(app.session.is_none());
        assert!(app.draft.is_empty());
        assert_eq!(app.detail_scroll, 0);
        assert!(app.status_title().contains("fresh session"));
    }

    #[test]
    fn single_session_wraps_one_session_card() {
        let card = workspace::SessionCard {
            session_id: "session_alpha".to_string(),
            title: "alpha".to_string(),
            subtitle: "active".to_string(),
            detail: "3 msgs".to_string(),
            preview_lines: vec!["user hello".to_string()],
            detail_lines: vec!["assistant hi".to_string()],
        };
        let mut app = SingleSessionApp::new(Some(card));

        assert_eq!(app.handle_key(KeyInput::Enter), KeyOutcome::Redraw);
        assert_eq!(app.draft, "\n");
        app.handle_key(KeyInput::Character("draft".to_string()));
        assert_eq!(
            app.handle_key(KeyInput::SubmitDraft),
            KeyOutcome::SendDraft {
                session_id: "session_alpha".to_string(),
                title: "alpha".to_string(),
                message: "draft".to_string(),
            }
        );
    }

    #[test]
    fn single_session_surface_is_the_panel_primitive() {
        let card = workspace::SessionCard {
            session_id: "session_alpha".to_string(),
            title: "alpha".to_string(),
            subtitle: "active".to_string(),
            detail: "3 msgs".to_string(),
            preview_lines: Vec::new(),
            detail_lines: Vec::new(),
        };

        let surface = single_session_surface(Some(&card));

        assert_eq!(surface.id, 1);
        assert_eq!(surface.title, "alpha");
        assert_eq!(surface.session_id.as_deref(), Some("session_alpha"));
        assert_eq!((surface.lane, surface.column), (0, 0));
        assert!(
            surface
                .body_lines
                .contains(&"single session mode".to_string())
        );
    }

    #[test]
    fn focused_panel_draft_only_shows_for_focused_insert_panel() {
        let mut workspace = Workspace::from_session_cards(vec![workspace::SessionCard {
            session_id: "a".to_string(),
            title: "alpha".to_string(),
            subtitle: "active".to_string(),
            detail: "1 msg".to_string(),
            preview_lines: Vec::new(),
            detail_lines: Vec::new(),
        }]);
        workspace.handle_key(KeyInput::Character("i".to_string()));
        workspace.handle_key(KeyInput::Character("draft text".to_string()));

        assert_eq!(
            focused_panel_draft(&workspace, workspace.focused_id),
            Some("draft text".to_string())
        );
        assert_eq!(
            focused_panel_draft(&workspace, workspace.focused_id + 1),
            None
        );
    }
}
