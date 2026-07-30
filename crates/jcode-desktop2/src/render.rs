//! GPU state: wgpu surface + Vello renderer.
//!
//! The wgpu instance, adapter, and device are set up here directly rather
//! than through `vello::util::RenderContext`. The reason is memory:
//! `RenderContext` requests its device with wgpu's default
//! `MemoryHints::Performance`, which tells the allocator to carve GPU memory
//! in 128MiB device / 64MiB host blocks. For a 2D UI that draws a few dozen
//! buffers a frame, those pools sat ~200MiB of mapped host memory in every
//! window's RSS doing nothing. `MemoryHints::MemoryUsage` (8MiB/4MiB blocks)
//! serves the same workload in a fraction of that, and lets the app run many
//! windows without each one costing a quarter gigabyte.

use anyhow::{Result, anyhow};
use std::sync::Arc;
use vello::wgpu::{self, util::TextureBlitter};
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};
use winit::window::Window;

pub struct RenderState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Vello renders into this intermediate texture (its compute shaders
    /// cannot bind most surface formats directly), then blits to the surface.
    target_view: wgpu::TextureView,
    blitter: TextureBlitter,
    renderer: Renderer,
    window: Arc<Window>,
}

impl RenderState {
    /// Create an instance limited to `backends`, and find an adapter that can
    /// present to the window. Split out so the caller can retry with more
    /// backends. wgpu handles are internally refcounted, so the instance
    /// itself does not need to outlive this function.
    async fn pick_adapter(
        window: &Arc<Window>,
        backends: wgpu::Backends,
    ) -> Result<(wgpu::Surface<'static>, wgpu::Adapter)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends,
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| anyhow!("create surface: {error}"))?;
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, Some(&surface))
            .await
            .map_err(|error| anyhow!("no compatible GPU adapter: {error}"))?;
        Ok((surface, adapter))
    }

    pub async fn new(window: Arc<Window>) -> Result<Self> {
        // Try primary backends (Vulkan here) first, and only fall back to the
        // full set (which adds GL) when no primary adapter exists. Merely
        // enabling the GL backend makes wgpu initialise EGL during adapter
        // enumeration, which drags Mesa's GL stack and libLLVM (~90MiB of
        // resident file pages) into a process that then renders with Vulkan
        // and never touches them again.
        let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        let (surface, adapter) = match Self::pick_adapter(&window, backends).await {
            Ok(picked) => picked,
            Err(_) if backends == wgpu::Backends::PRIMARY => {
                Self::pick_adapter(&window, wgpu::Backends::all()).await?
            }
            Err(error) => return Err(error),
        };
        // The features Vello can use when present; mirrors what
        // `vello::util::RenderContext` requests.
        let optional = wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("desktop2 device"),
                required_features: adapter.features() & optional,
                required_limits: wgpu::Limits::default(),
                // The whole point of this module owning device creation: see
                // the module docs. Small allocator blocks, not 128MiB pools.
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await
            .map_err(|error| anyhow!("request GPU device: {error}"))?;

        let size = window.inner_size();
        let format = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .ok_or_else(|| anyhow!("no supported surface format"))?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let target_view = create_target(&device, config.width, config.height);

        // Only Area AA is ever rendered (see `render` below), so compile only
        // its pipeline permutations. The default compiles MSAA8 and MSAA16
        // variants too, which costs startup time and driver-compiled shader
        // memory that would never be used.
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                antialiasing_support: vello::AaSupport::area_only(),
                ..RendererOptions::default()
            },
        )
        .map_err(|error| anyhow!("create renderer: {error}"))?;
        let blitter = TextureBlitter::new(&device, format);
        Ok(Self {
            device,
            queue,
            surface,
            config,
            target_view,
            blitter,
            renderer,
            window,
        })
    }

    /// Window scale factor (HiDPI). Scene layout is in logical units.
    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.target_view = create_target(&self.device, self.config.width, self.config.height);
        self.window.request_redraw();
    }

    /// Draw `scene` and put it on screen, timing the GPU and present spans
    /// into `meter`.
    ///
    /// The meter is threaded through as a parameter rather than owned here
    /// because a frame's spans only mean something together: the build span is
    /// measured by the caller around `build_scene`, and a GPU number without
    /// the build number beside it cannot say which half is slow.
    pub fn render(
        &mut self,
        scene: &Scene,
        meter: &mut crate::frame_meter::FrameMeter,
    ) -> Result<()> {
        meter.start();
        let (width, height) = self.size();
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &self.target_view,
                &RenderParams {
                    base_color: vello::peniko::Color::BLACK,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|error| anyhow!("vello render: {error}"))?;
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            // Skip this frame; the next resize/redraw will recover.
            _ => return Ok(()),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("desktop2 blit"),
            });
        self.blitter.copy(
            &self.device,
            &mut encoder,
            &self.target_view,
            &surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        self.queue.submit([encoder.finish()]);
        meter.end_gpu();
        // Timed separately from the work above because this is the call that
        // blocks on the compositor: vsync back-pressure shows up here and
        // nowhere else, and it is not a cost any code change to painting can
        // reduce.
        meter.start();
        surface_texture.present();
        meter.end_present();
        Ok(())
    }
}

/// The intermediate texture Vello's compute pipeline renders into.
fn create_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("desktop2 render target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

impl RenderState {
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Retitle the window, so a compositor's window list can tell two jcode
    /// windows on different checkouts apart.
    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    /// Set the mouse pointer shape.
    pub fn set_cursor_icon(&self, icon: winit::window::CursorIcon) {
        self.window.set_cursor(icon);
    }
}
