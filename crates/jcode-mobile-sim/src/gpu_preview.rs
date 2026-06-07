use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use jcode_mobile_core::{UiRect, VisualPrimitive, VisualScene};
use serde::{Deserialize, Serialize};
use wgpu::util::DeviceExt;
use wgpu::{CompositeAlphaMode, PresentMode, SurfaceError, TextureUsages};
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowBuilder};

const PREVIEW_SCALE: f64 = 1.0;
const TEXT_PIXEL: f32 = 2.0;
const ROUNDED_CORNER_SEGMENTS: usize = 6;

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

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Pod, Zeroable)]
pub struct PreviewVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
}

impl PreviewVertex {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PreviewVertex>() as wgpu::BufferAddress,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewMesh {
    pub backend: String,
    pub scene_schema_version: u32,
    pub viewport: UiRect,
    pub vertex_count: usize,
    pub vertices: Vec<PreviewVertex>,
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

pub fn build_preview_mesh(scene: &VisualScene) -> PreviewMesh {
    let size = PhysicalSize::new(scene.viewport.width as u32, scene.viewport.height as u32);
    let mut vertices = Vec::new();
    let mut layers: Vec<_> = scene.layers.iter().collect();
    layers.sort_by_key(|layer| layer.z_index);

    for layer in layers {
        for primitive in &layer.primitives {
            match primitive {
                VisualPrimitive::Rect(rect) => {
                    let fill = parse_color(&rect.fill).unwrap_or([1.0, 0.0, 1.0, 1.0]);
                    let bounds = to_rect(rect.bounds);
                    push_rounded_rect(&mut vertices, bounds, rect.corner_radius as f32, fill, size);
                    if let Some(stroke) = &rect.stroke
                        && let Some(stroke_color) = parse_color(stroke)
                    {
                        push_stroked_rect(
                            &mut vertices,
                            bounds,
                            rect.stroke_width.max(1) as f32,
                            stroke_color,
                            size,
                        );
                    }
                }
                VisualPrimitive::Text(text) => {
                    let fill = parse_color(&text.fill).unwrap_or([1.0, 1.0, 1.0, 1.0]);
                    let y = text.y as f32 - bitmap_text_height(TEXT_PIXEL);
                    push_bitmap_text(
                        &mut vertices,
                        &normalize_preview_text(&text.text),
                        [text.x as f32, y],
                        TEXT_PIXEL,
                        fill,
                        size,
                        text.max_width as f32,
                    );
                }
            }
        }
    }

    PreviewMesh {
        backend: "wgpu-triangle-list-v1".to_string(),
        scene_schema_version: scene.schema_version,
        viewport: scene.viewport,
        vertex_count: vertices.len(),
        vertices,
    }
}

pub fn run_preview(scene: VisualScene) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create mobile preview event loop")?;
    let window: &'static Window = Box::leak(Box::new(
        WindowBuilder::new()
            .with_title("Jcode Mobile Rust Scene Preview")
            .with_inner_size(LogicalSize::new(
                scene.viewport.width as f64 * PREVIEW_SCALE,
                scene.viewport.height as f64 * PREVIEW_SCALE,
            ))
            .build(&event_loop)
            .context("failed to create mobile preview window")?,
    ));
    let mut canvas = pollster::block_on(PreviewCanvas::new(window, scene))?;

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
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed
                        && matches!(event.logical_key, Key::Named(NamedKey::Escape)) =>
                {
                    target.exit();
                }
                WindowEvent::RedrawRequested => match canvas.render() {
                    Ok(()) => {}
                    Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                        canvas.resize(window.inner_size());
                        window.request_redraw();
                    }
                    Err(SurfaceError::OutOfMemory) => target.exit(),
                    Err(SurfaceError::Timeout) => window.request_redraw(),
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

struct PreviewCanvas<'window> {
    surface: wgpu::Surface<'window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    scene: VisualScene,
    size: PhysicalSize<u32>,
    needs_initial_frame: bool,
}

impl<'window> PreviewCanvas<'window> {
    async fn new(window: &'window Window, scene: VisualScene) -> Result<Self> {
        let size = non_zero_size(window.inner_size());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window)
            .context("failed to create mobile preview wgpu surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to find compatible mobile preview GPU adapter")?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("jcode-mobile-preview-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .context("failed to create mobile preview wgpu device")?;
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
            label: Some("jcode-mobile-preview-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("jcode-mobile-preview-pipeline-layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("jcode-mobile-preview-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[PreviewVertex::layout()],
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
            scene,
            size,
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

    fn render(&mut self) -> std::result::Result<(), SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("jcode-mobile-preview-render"),
            });
        let vertices = build_preview_vertices_for_size(&self.scene, self.size);
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("jcode-mobile-preview-vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("jcode-mobile-preview-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.043,
                            g: 0.063,
                            b: 0.125,
                            a: 1.0,
                        }),
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
        Ok(())
    }
}

fn build_preview_vertices_for_size(
    scene: &VisualScene,
    size: PhysicalSize<u32>,
) -> Vec<PreviewVertex> {
    let base = build_preview_mesh(scene).vertices;
    if size.width == scene.viewport.width as u32 && size.height == scene.viewport.height as u32 {
        return base;
    }

    let sx = size.width as f32 / scene.viewport.width.max(1) as f32;
    let sy = size.height as f32 / scene.viewport.height.max(1) as f32;
    let s = sx.min(sy);
    let used_width = scene.viewport.width as f32 * s;
    let used_height = scene.viewport.height as f32 * s;
    let offset_x = (size.width as f32 - used_width) / 2.0;
    let offset_y = (size.height as f32 - used_height) / 2.0;

    // Convert normalized scene vertices back to logical pixels, then normalize for the window.
    base.into_iter()
        .map(|vertex| {
            let x = (vertex.position[0] + 1.0) * 0.5 * scene.viewport.width as f32;
            let y = (1.0 - vertex.position[1]) * 0.5 * scene.viewport.height as f32;
            let x = offset_x + x * s;
            let y = offset_y + y * s;
            PreviewVertex {
                position: pixel_to_ndc(x, y, size),
                color: vertex.color,
            }
        })
        .collect()
}

fn non_zero_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

fn to_rect(rect: UiRect) -> Rect {
    Rect {
        x: rect.x as f32,
        y: rect.y as f32,
        width: rect.width as f32,
        height: rect.height as f32,
    }
}

fn parse_color(input: &str) -> Option<[f32; 4]> {
    let hex = input.strip_prefix('#')?;
    let (r, g, b, a) = match hex.len() {
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            255,
        ),
        8 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

fn push_stroked_rect(
    vertices: &mut Vec<PreviewVertex>,
    rect: Rect,
    stroke_width: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let stroke_width = stroke_width.max(1.0).min(rect.width).min(rect.height);
    push_rect(
        vertices,
        Rect {
            height: stroke_width,
            ..rect
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            y: rect.y + rect.height - stroke_width,
            height: stroke_width,
            ..rect
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            width: stroke_width,
            ..rect
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x + rect.width - stroke_width,
            width: stroke_width,
            ..rect
        },
        color,
        size,
    );
}

fn push_rounded_rect(
    vertices: &mut Vec<PreviewVertex>,
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
    let corners = [
        (
            rect.x + rect.width - radius,
            rect.y + radius,
            -90.0_f32,
            0.0_f32,
        ),
        (
            rect.x + rect.width - radius,
            rect.y + rect.height - radius,
            0.0,
            90.0,
        ),
        (rect.x + radius, rect.y + rect.height - radius, 90.0, 180.0),
        (rect.x + radius, rect.y + radius, 180.0, 270.0),
    ];
    let mut outline = Vec::new();
    for (cx, cy, start, end) in corners {
        for step in 0..=ROUNDED_CORNER_SEGMENTS {
            let t = step as f32 / ROUNDED_CORNER_SEGMENTS as f32;
            let angle = (start + (end - start) * t).to_radians();
            outline.push([cx + radius * angle.cos(), cy + radius * angle.sin()]);
        }
    }
    for idx in 0..outline.len() {
        let a = outline[idx];
        let b = outline[(idx + 1) % outline.len()];
        push_pixel_triangle(vertices, center, a, b, color, size);
    }
}

fn push_rect(
    vertices: &mut Vec<PreviewVertex>,
    rect: Rect,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let left_top = pixel_to_ndc(rect.x, rect.y, size);
    let right_top = pixel_to_ndc(rect.x + rect.width, rect.y, size);
    let right_bottom = pixel_to_ndc(rect.x + rect.width, rect.y + rect.height, size);
    let left_bottom = pixel_to_ndc(rect.x, rect.y + rect.height, size);
    vertices.extend_from_slice(&[
        PreviewVertex {
            position: left_top,
            color,
        },
        PreviewVertex {
            position: left_bottom,
            color,
        },
        PreviewVertex {
            position: right_bottom,
            color,
        },
        PreviewVertex {
            position: left_top,
            color,
        },
        PreviewVertex {
            position: right_bottom,
            color,
        },
        PreviewVertex {
            position: right_top,
            color,
        },
    ]);
}

fn push_pixel_triangle(
    vertices: &mut Vec<PreviewVertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    vertices.extend_from_slice(&[
        PreviewVertex {
            position: pixel_to_ndc(a[0], a[1], size),
            color,
        },
        PreviewVertex {
            position: pixel_to_ndc(b[0], b[1], size),
            color,
        },
        PreviewVertex {
            position: pixel_to_ndc(c[0], c[1], size),
            color,
        },
    ]);
}

fn pixel_to_ndc(x: f32, y: f32, size: PhysicalSize<u32>) -> [f32; 2] {
    let width = size.width.max(1) as f32;
    let height = size.height.max(1) as f32;
    [x / width * 2.0 - 1.0, 1.0 - y / height * 2.0]
}

fn normalize_preview_text(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '&' => '+',
            ':' | '.' | ',' | ';' | '!' | '?' | '_' => ' ',
            ch if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '/' | '+' | '#') => ch,
            _ => ' ',
        })
        .collect::<String>()
        .to_ascii_uppercase()
}

fn push_bitmap_text(
    vertices: &mut Vec<PreviewVertex>,
    text: &str,
    origin: [f32; 2],
    pixel: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
    max_width: f32,
) {
    let [x, y] = origin;
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
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        ' ' => [0; 7],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_mobile_core::{ScenarioName, SimulatorState, SimulatorStore};

    #[test]
    fn preview_mesh_is_deterministic_triangle_list_from_visual_scene() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let scene = store.visual_scene();
        let first = build_preview_mesh(&scene);
        let second = build_preview_mesh(&scene);

        assert_eq!(first, second);
        assert_eq!(first.backend, "wgpu-triangle-list-v1");
        assert_eq!(first.scene_schema_version, scene.schema_version);
        assert_eq!(first.viewport.width, 390);
        assert_eq!(first.viewport.height, 844);
        assert!(first.vertex_count > 500);
        assert_eq!(first.vertex_count, first.vertices.len());
        assert!(first.vertices.iter().all(|vertex| {
            vertex.position[0].is_finite()
                && vertex.position[1].is_finite()
                && vertex.position[0] >= -1.01
                && vertex.position[0] <= 1.01
                && vertex.position[1] >= -1.01
                && vertex.position[1] <= 1.01
        }));
    }

    #[test]
    fn preview_color_parser_handles_scene_hex_colors() {
        assert_eq!(parse_color("#ffffff"), Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(
            parse_color("#00000080"),
            Some([0.0, 0.0, 0.0, 128.0 / 255.0])
        );
        assert_eq!(parse_color("blue"), None);
    }
}
