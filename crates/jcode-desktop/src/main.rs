mod session_data;
mod workspace;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use wgpu::{CompositeAlphaMode, PresentMode, SurfaceError, TextureUsages};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Fullscreen, Window, WindowBuilder};
use workspace::{InputMode, KeyInput, KeyOutcome, PanelSizePreset, Workspace};

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

    let session_cards = match session_data::load_recent_session_cards() {
        Ok(cards) => cards,
        Err(error) => {
            eprintln!("jcode-desktop: failed to load session metadata: {error:#}");
            Vec::new()
        }
    };
    let mut workspace = Workspace::from_session_cards(session_cards);
    window.set_title(&workspace.status_title());
    let mut canvas = Canvas::new(window).await?;
    let mut modifiers = ModifiersState::empty();

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
                    match workspace.handle_key(to_key_input(&event.logical_key, modifiers)) {
                        KeyOutcome::Exit => target.exit(),
                        KeyOutcome::Redraw => {
                            window.set_title(&workspace.status_title());
                            window.request_redraw();
                        }
                        KeyOutcome::None => {}
                    }
                }
                WindowEvent::RedrawRequested => match canvas.render(
                    &workspace,
                    window.current_monitor().map(|monitor| monitor.size()),
                ) {
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

fn to_key_input(key: &Key, modifiers: ModifiersState) -> KeyInput {
    match key {
        Key::Named(NamedKey::Escape) => KeyInput::Escape,
        Key::Named(NamedKey::Enter) => KeyInput::Enter,
        Key::Named(NamedKey::Backspace) => KeyInput::Backspace,
        Key::Character(text) if modifiers.control_key() && text == ";" => KeyInput::SpawnPanel,
        Key::Character(text) if modifiers.control_key() && (text == "?" || text == "/") => {
            KeyInput::HotkeyHelp
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
        workspace: &Workspace,
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
        let target_layout = workspace_render_layout(workspace, self.size, monitor_size);
        let render_layout = self.viewport_animation.frame(target_layout, now);
        let focus_pulse = self.focus_pulse.frame(workspace.focused_id, now);
        let animation_active =
            self.viewport_animation.is_animating() || self.focus_pulse.is_animating();
        let vertices = build_vertices(workspace, self.size, render_layout, focus_pulse);
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
            push_panel_contents(&mut vertices, surface, rect, size);
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
        push_panel_contents(&mut vertices, surface, rect, size);
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

fn push_panel_title(vertices: &mut Vec<Vertex>, title: &str, rect: Rect, size: PhysicalSize<u32>) {
    let text = normalize_bitmap_text(title);
    let max_width = (rect.width - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
    push_bitmap_text(
        vertices,
        &text,
        rect.x + PANEL_TITLE_LEFT_PADDING,
        rect.y + PANEL_TITLE_TOP_PADDING,
        BITMAP_TEXT_PIXEL,
        PANEL_TITLE_COLOR,
        size,
        max_width,
    );
}

fn push_panel_contents(
    vertices: &mut Vec<Vertex>,
    surface: &workspace::Surface,
    rect: Rect,
    size: PhysicalSize<u32>,
) {
    push_panel_title(vertices, surface.title.as_str(), rect, size);

    let max_width = (rect.width - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
    let mut y = rect.y + PANEL_BODY_TOP_PADDING;
    let line_height = bitmap_text_height(BITMAP_TEXT_PIXEL) + PANEL_BODY_LINE_GAP;
    let max_y = rect.y + rect.height - PANEL_TITLE_TOP_PADDING;
    for line in &surface.body_lines {
        if y + bitmap_text_height(BITMAP_TEXT_PIXEL) > max_y {
            break;
        }
        let text = normalize_bitmap_text(line);
        push_bitmap_text(
            vertices,
            &text,
            rect.x + PANEL_TITLE_LEFT_PADDING,
            y,
            BITMAP_TEXT_PIXEL,
            PANEL_BODY_COLOR,
            size,
            max_width,
        );
        y += line_height;
    }
}

fn normalize_bitmap_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        let mapped = match ch {
            'a'..='z' => ch.to_ascii_uppercase(),
            'A'..='Z' | '0'..='9' => ch,
            '-' | '/' => ch,
            _ => ' ',
        };
        if mapped == ' ' {
            if !last_was_space {
                normalized.push(mapped);
            }
            last_was_space = true;
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }
    normalized.trim().to_string()
}

fn push_bitmap_text(
    vertices: &mut Vec<Vertex>,
    text: &str,
    x: f32,
    y: f32,
    pixel: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
    max_width: f32,
) {
    let advance = bitmap_char_advance(pixel);
    let mut cursor_x = x;
    for ch in text.chars() {
        if cursor_x + 5.0 * pixel > x + max_width {
            break;
        }
        if let Some(rows) = bitmap_glyph(ch) {
            for (row_index, row) in rows.iter().enumerate() {
                for column in 0..5 {
                    let mask = 1 << (4 - column);
                    if row & mask != 0 {
                        push_rect(
                            vertices,
                            Rect {
                                x: cursor_x + column as f32 * pixel,
                                y: y + row_index as f32 * pixel,
                                width: pixel,
                                height: pixel,
                            },
                            color,
                            size,
                        );
                    }
                }
            }
        }
        cursor_x += advance;
    }
}

fn bitmap_text_width(text: &str, pixel: f32) -> f32 {
    let count = text.chars().count();
    if count == 0 {
        0.0
    } else {
        count as f32 * 5.0 * pixel + count.saturating_sub(1) as f32 * pixel
    }
}

fn bitmap_text_height(pixel: f32) -> f32 {
    7.0 * pixel
}

fn bitmap_char_advance(pixel: f32) -> f32 {
    6.0 * pixel
}

fn bitmap_glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        ' ' => [0; 7],
        _ => return None,
    })
}

fn push_workspace_number(
    vertices: &mut Vec<Vertex>,
    active_workspace: i32,
    status_rect: Rect,
    size: PhysicalSize<u32>,
) {
    let label = active_workspace.to_string();
    let digit_count = label.chars().count() as f32;
    let total_width = digit_count * WORKSPACE_NUMBER_DIGIT_WIDTH
        + (digit_count - 1.0).max(0.0) * WORKSPACE_NUMBER_DIGIT_GAP;
    let mut x = status_rect.x + WORKSPACE_NUMBER_LEFT_PADDING;
    let y = status_rect.y + (status_rect.height - WORKSPACE_NUMBER_DIGIT_HEIGHT) / 2.0;
    if x + total_width > status_rect.x + status_rect.width {
        return;
    }

    for ch in label.chars() {
        match ch {
            '-' => push_workspace_minus(vertices, x, y, size),
            digit if digit.is_ascii_digit() => {
                let digit = digit.to_digit(10).unwrap_or_default() as usize;
                push_workspace_digit(vertices, digit, x, y, size);
            }
            _ => {}
        }
        x += WORKSPACE_NUMBER_DIGIT_WIDTH + WORKSPACE_NUMBER_DIGIT_GAP;
    }
}

fn push_workspace_minus(vertices: &mut Vec<Vertex>, x: f32, y: f32, size: PhysicalSize<u32>) {
    let thickness = WORKSPACE_NUMBER_STROKE;
    push_rounded_rect(
        vertices,
        Rect {
            x,
            y: y + WORKSPACE_NUMBER_DIGIT_HEIGHT / 2.0 - thickness / 2.0,
            width: WORKSPACE_NUMBER_DIGIT_WIDTH,
            height: thickness,
        },
        thickness / 2.0,
        WORKSPACE_NUMBER_COLOR,
        size,
    );
}

fn push_workspace_digit(
    vertices: &mut Vec<Vertex>,
    digit: usize,
    x: f32,
    y: f32,
    size: PhysicalSize<u32>,
) {
    const DIGIT_SEGMENTS: [[bool; 7]; 10] = [
        [true, true, true, true, true, true, false],
        [false, true, true, false, false, false, false],
        [true, true, false, true, true, false, true],
        [true, true, true, true, false, false, true],
        [false, true, true, false, false, true, true],
        [true, false, true, true, false, true, true],
        [true, false, true, true, true, true, true],
        [true, true, true, false, false, false, false],
        [true, true, true, true, true, true, true],
        [true, true, true, true, false, true, true],
    ];
    let segments = DIGIT_SEGMENTS[digit % DIGIT_SEGMENTS.len()];
    for rect in workspace_digit_segment_rects(x, y)
        .into_iter()
        .zip(segments)
        .filter_map(|(rect, enabled)| enabled.then_some(rect))
    {
        push_rounded_rect(
            vertices,
            rect,
            WORKSPACE_NUMBER_STROKE / 2.0,
            WORKSPACE_NUMBER_COLOR,
            size,
        );
    }
}

fn workspace_digit_segment_rects(x: f32, y: f32) -> [Rect; 7] {
    let w = WORKSPACE_NUMBER_DIGIT_WIDTH;
    let h = WORKSPACE_NUMBER_DIGIT_HEIGHT;
    let t = WORKSPACE_NUMBER_STROKE;
    let vertical_height = (h - t * 3.0) / 2.0;
    [
        Rect {
            x,
            y,
            width: w,
            height: t,
        },
        Rect {
            x: x + w - t,
            y,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x: x + w - t,
            y: y + h / 2.0,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x,
            y: y + h - t,
            width: w,
            height: t,
        },
        Rect {
            x,
            y: y + h / 2.0,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x,
            y,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x,
            y: y + h / 2.0 - t / 2.0,
            width: w,
            height: t,
        },
    ]
}

fn push_status_preview(
    vertices: &mut Vec<Vertex>,
    workspace: &Workspace,
    active_workspace: i32,
    visible_layout: VisibleColumnLayout,
    status_rect: Rect,
    size: PhysicalSize<u32>,
) {
    let first_lane = active_workspace - STATUS_PREVIEW_LANE_RADIUS;
    let last_lane = active_workspace + STATUS_PREVIEW_LANE_RADIUS;
    let lanes: Vec<StatusPreviewLane> = (first_lane..=last_lane)
        .map(|lane| status_preview_lane(workspace, lane, active_workspace, visible_layout))
        .filter(|lane| !lane.is_empty || lane.is_active)
        .collect();

    if lanes.is_empty() {
        return;
    }

    let full_width = lanes.iter().map(StatusPreviewLane::width).sum::<f32>()
        + STATUS_PREVIEW_GROUP_GAP * lanes.len().saturating_sub(1) as f32;
    let preview_area = inset_rect(
        status_rect,
        STATUS_PREVIEW_SIDE_RESERVE.min(status_rect.width / 4.0),
    );
    let max_width = STATUS_PREVIEW_MAX_WIDTH.min((preview_area.width - 24.0).max(1.0));
    if max_width < 24.0 {
        return;
    }
    let scale = (max_width / full_width).min(1.0);
    let panel_width = (STATUS_PREVIEW_PANEL_WIDTH * scale).max(2.0);
    let panel_gap = (STATUS_PREVIEW_PANEL_GAP * scale).max(1.0);
    let group_gap = (STATUS_PREVIEW_GROUP_GAP * scale).max(4.0);
    let scaled_width = lanes
        .iter()
        .map(|lane| lane.scaled_width(panel_width, panel_gap))
        .sum::<f32>()
        + group_gap * lanes.len().saturating_sub(1) as f32;
    let strip_height = STATUS_PREVIEW_HEIGHT.min((status_rect.height - 8.0).max(1.0));
    let strip_y = status_rect.y + (status_rect.height - strip_height) / 2.0;
    let mut cursor_x = preview_area.x + (preview_area.width - scaled_width) / 2.0;

    for lane in lanes {
        let lane_width = lane.scaled_width(panel_width, panel_gap);
        let lane_rect = Rect {
            x: cursor_x - 3.0,
            y: strip_y - 3.0,
            width: lane_width + 6.0,
            height: strip_height + 6.0,
        };

        if lane.is_active {
            push_rounded_rect(
                vertices,
                lane_rect,
                5.0,
                STATUS_PREVIEW_ACTIVE_GROUP_COLOR,
                size,
            );
        }

        if lane.is_empty {
            push_rounded_rect(
                vertices,
                Rect {
                    x: cursor_x + lane_width / 2.0 - 2.0,
                    y: strip_y + strip_height / 2.0 - 2.0,
                    width: 4.0,
                    height: 4.0,
                },
                2.0,
                STATUS_PREVIEW_EMPTY_FOCUSED_COLOR,
                size,
            );
            cursor_x += lane_width + group_gap;
            continue;
        }

        for surface in workspace
            .surfaces
            .iter()
            .filter(|surface| surface.lane == lane.lane)
        {
            let column_offset = (surface.column - lane.min_column) as f32;
            let surface_x = cursor_x + column_offset * (panel_width + panel_gap);
            let focused = workspace.is_focused(surface.id);
            let color = status_preview_surface_color(surface.color_index, focused, lane.is_active);
            let tick_width = if focused {
                panel_width
            } else {
                panel_width * 0.56
            };
            let tick_x = surface_x + (panel_width - tick_width) / 2.0;
            push_rounded_rect(
                vertices,
                Rect {
                    x: tick_x,
                    y: strip_y,
                    width: tick_width.max(2.0),
                    height: strip_height,
                },
                2.0,
                color,
                size,
            );
        }

        if lane.is_active {
            let viewport_x = cursor_x
                + (visible_layout.first_visible_column - lane.min_column) as f32
                    * (panel_width + panel_gap);
            let viewport_width = visible_layout.visible_columns as f32 * panel_width
                + visible_layout.visible_columns.saturating_sub(1) as f32 * panel_gap;
            push_stroked_rect(
                vertices,
                Rect {
                    x: viewport_x - 1.5,
                    y: strip_y - 2.0,
                    width: (viewport_width + 3.0).min(cursor_x + lane_width - viewport_x + 1.5),
                    height: strip_height + 4.0,
                },
                1.0,
                STATUS_PREVIEW_VIEWPORT_COLOR,
                size,
            );
        }

        cursor_x += lane_width + group_gap;
    }
}

fn status_preview_surface_color(color_index: usize, focused: bool, active_lane: bool) -> [f32; 4] {
    let accent = STATUS_PREVIEW_ACCENTS[color_index % STATUS_PREVIEW_ACCENTS.len()];
    let alpha = if focused {
        0.94
    } else if active_lane {
        0.72
    } else {
        0.34
    };
    [accent[0], accent[1], accent[2], alpha]
}

#[derive(Clone, Copy)]
struct StatusPreviewLane {
    lane: i32,
    min_column: i32,
    max_column: i32,
    is_active: bool,
    is_empty: bool,
}

impl StatusPreviewLane {
    fn column_count(&self) -> i32 {
        (self.max_column - self.min_column + 1).max(1)
    }

    fn width(&self) -> f32 {
        self.scaled_width(STATUS_PREVIEW_PANEL_WIDTH, STATUS_PREVIEW_PANEL_GAP)
    }

    fn scaled_width(&self, panel_width: f32, panel_gap: f32) -> f32 {
        let column_count = self.column_count() as f32;
        column_count * panel_width + (column_count - 1.0).max(0.0) * panel_gap
    }
}

fn status_preview_lane(
    workspace: &Workspace,
    lane: i32,
    active_workspace: i32,
    visible_layout: VisibleColumnLayout,
) -> StatusPreviewLane {
    let is_active = lane == active_workspace;
    let viewport_first_column = visible_layout.first_visible_column;
    let viewport_last_column =
        viewport_first_column + visible_layout.visible_columns.saturating_sub(1) as i32;
    let mut min_column = if is_active {
        viewport_first_column
    } else {
        i32::MAX
    };
    let mut max_column = if is_active {
        viewport_last_column
    } else {
        i32::MIN
    };
    let mut is_empty = true;

    for surface in workspace
        .surfaces
        .iter()
        .filter(|surface| surface.lane == lane)
    {
        min_column = min_column.min(surface.column);
        max_column = max_column.max(surface.column);
        is_empty = false;
    }

    if is_empty && !is_active {
        min_column = 0;
        max_column = 0;
    }

    StatusPreviewLane {
        lane,
        min_column,
        max_column,
        is_active,
        is_empty,
    }
}

fn push_surface(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    _color_index: usize,
    focused: bool,
    focus_pulse: f32,
    size: PhysicalSize<u32>,
) {
    let border = if focused {
        FOCUS_RING_COLOR
    } else {
        UNFOCUSED_BORDER_COLOR
    };

    let stroke_width = if focused {
        FOCUSED_BORDER_WIDTH
    } else {
        UNFOCUSED_BORDER_WIDTH
    } + focus_pulse * 2.5;
    push_panel_outline(vertices, rect, stroke_width, border, size);

    if focus_pulse > 0.0 {
        let pulse_rect = inset_rect(rect, -3.0 * focus_pulse);
        push_panel_outline(
            vertices,
            pulse_rect,
            1.0,
            with_alpha(FOCUS_RING_COLOR, 0.32 * focus_pulse),
            size,
        );
    }
}

fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha.clamp(0.0, 1.0);
    color
}

fn push_panel_outline(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    stroke_width: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let stroke_width = stroke_width
        .max(1.0)
        .min(rect.width / 2.0)
        .min(rect.height / 2.0);
    let outer_radius = PANEL_RADIUS.min(rect.width / 2.0).min(rect.height / 2.0);
    let inner = inset_rect(rect, stroke_width);
    let inner_radius = (outer_radius - stroke_width).max(0.0);
    let outer_points = rounded_rect_points(rect, outer_radius);
    let inner_points = rounded_rect_points(inner, inner_radius);

    for index in 0..outer_points.len() {
        let next_index = (index + 1) % outer_points.len();
        push_pixel_triangle(
            vertices,
            outer_points[index],
            outer_points[next_index],
            inner_points[next_index],
            color,
            size,
        );
        push_pixel_triangle(
            vertices,
            outer_points[index],
            inner_points[next_index],
            inner_points[index],
            color,
            size,
        );
    }
}

fn rounded_rect_points(rect: Rect, radius: f32) -> Vec<[f32; 2]> {
    let radius = radius.max(0.0).min(rect.width / 2.0).min(rect.height / 2.0);
    let mut points = Vec::with_capacity((ROUNDED_CORNER_SEGMENTS + 1) * 4);
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + radius,
        radius,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    );
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + rect.height - radius,
        radius,
        0.0,
        std::f32::consts::FRAC_PI_2,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + rect.height - radius,
        radius,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + radius,
        radius,
        std::f32::consts::PI,
        std::f32::consts::PI * 1.5,
    );
    points
}

fn inset_rect(rect: Rect, amount: f32) -> Rect {
    Rect {
        x: rect.x + amount,
        y: rect.y + amount,
        width: (rect.width - amount * 2.0).max(1.0),
        height: (rect.height - amount * 2.0).max(1.0),
    }
}

fn push_rect(vertices: &mut Vec<Vertex>, rect: Rect, color: [f32; 4], size: PhysicalSize<u32>) {
    push_gradient_rect(vertices, rect, color, color, color, color, size);
}

fn push_stroked_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    stroke_width: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let stroke_width = stroke_width.max(1.0).min(rect.width).min(rect.height);
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: stroke_width,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y + rect.height - stroke_width,
            width: rect.width,
            height: stroke_width,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: stroke_width,
            height: rect.height,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x + rect.width - stroke_width,
            y: rect.y,
            width: stroke_width,
            height: rect.height,
        },
        color,
        size,
    );
}

fn push_rounded_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    radius: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let radius = radius.max(0.0).min(rect.width / 2.0).min(rect.height / 2.0);
    if radius <= 0.5 {
        push_rect(vertices, rect, color, size);
        return;
    }

    let center = [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0];
    let mut points = Vec::with_capacity((ROUNDED_CORNER_SEGMENTS + 1) * 4);
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + radius,
        radius,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    );
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + rect.height - radius,
        radius,
        0.0,
        std::f32::consts::FRAC_PI_2,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + rect.height - radius,
        radius,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + radius,
        radius,
        std::f32::consts::PI,
        std::f32::consts::PI * 1.5,
    );

    for index in 0..points.len() {
        let next_index = (index + 1) % points.len();
        push_pixel_triangle(
            vertices,
            center,
            points[index],
            points[next_index],
            color,
            size,
        );
    }
}

fn append_arc_points(
    points: &mut Vec<[f32; 2]>,
    center_x: f32,
    center_y: f32,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
) {
    for step in 0..=ROUNDED_CORNER_SEGMENTS {
        let t = step as f32 / ROUNDED_CORNER_SEGMENTS as f32;
        let angle = start_angle + (end_angle - start_angle) * t;
        points.push([
            center_x + radius * angle.cos(),
            center_y + radius * angle.sin(),
        ]);
    }
}

fn push_pixel_triangle(
    vertices: &mut Vec<Vertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    vertices.extend_from_slice(&[
        Vertex {
            position: pixel_to_ndc(a, size),
            color,
        },
        Vertex {
            position: pixel_to_ndc(b, size),
            color,
        },
        Vertex {
            position: pixel_to_ndc(c, size),
            color,
        },
    ]);
}

fn pixel_to_ndc(point: [f32; 2], size: PhysicalSize<u32>) -> [f32; 2] {
    let width = size.width.max(1) as f32;
    let height = size.height.max(1) as f32;
    [point[0] / width * 2.0 - 1.0, 1.0 - point[1] / height * 2.0]
}

fn push_gradient_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    top_left_color: [f32; 4],
    bottom_left_color: [f32; 4],
    bottom_right_color: [f32; 4],
    top_right_color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let width = size.width.max(1) as f32;
    let height = size.height.max(1) as f32;
    let left = rect.x / width * 2.0 - 1.0;
    let right = (rect.x + rect.width) / width * 2.0 - 1.0;
    let top = 1.0 - rect.y / height * 2.0;
    let bottom = 1.0 - (rect.y + rect.height) / height * 2.0;

    vertices.extend_from_slice(&[
        Vertex {
            position: [left, top],
            color: top_left_color,
        },
        Vertex {
            position: [left, bottom],
            color: bottom_left_color,
        },
        Vertex {
            position: [right, bottom],
            color: bottom_right_color,
        },
        Vertex {
            position: [left, top],
            color: top_left_color,
        },
        Vertex {
            position: [right, bottom],
            color: bottom_right_color,
        },
        Vertex {
            position: [right, top],
            color: top_right_color,
        },
    ]);
}

fn non_zero_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
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
}
