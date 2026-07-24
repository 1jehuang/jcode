//! GPU state: wgpu surface + Vello renderer.

use anyhow::{Result, anyhow};
use std::sync::Arc;
use vello::util::{RenderContext, RenderSurface};
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};
use winit::window::Window;

pub struct RenderState {
    context: RenderContext,
    surface: RenderSurface<'static>,
    renderer: Renderer,
    window: Arc<Window>,
}

impl RenderState {
    pub async fn new(window: Arc<Window>) -> Result<Self> {
        let mut context = RenderContext::new();
        let size = window.inner_size();
        let surface = context
            .create_surface(
                window.clone(),
                size.width.max(1),
                size.height.max(1),
                vello::wgpu::PresentMode::AutoVsync,
            )
            .await
            .map_err(|error| anyhow!("create surface: {error}"))?;
        let device_handle = &context.devices[surface.dev_id];
        let renderer = Renderer::new(&device_handle.device, RendererOptions::default())
            .map_err(|error| anyhow!("create renderer: {error}"))?;
        Ok(Self {
            context,
            surface,
            renderer,
            window,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.surface.config.width, self.surface.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.context
            .resize_surface(&mut self.surface, width.max(1), height.max(1));
        self.window.request_redraw();
    }

    pub fn render(&mut self, scene: &Scene) -> Result<()> {
        let device_handle = &self.context.devices[self.surface.dev_id];
        let (width, height) = self.size();
        self.renderer
            .render_to_texture(
                &device_handle.device,
                &device_handle.queue,
                scene,
                &self.surface.target_view,
                &RenderParams {
                    base_color: vello::peniko::Color::BLACK,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|error| anyhow!("vello render: {error}"))?;
        let surface_texture = match self.surface.surface.get_current_texture() {
            vello::wgpu::CurrentSurfaceTexture::Success(texture)
            | vello::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            // Skip this frame; the next resize/redraw will recover.
            _ => return Ok(()),
        };
        let mut encoder =
            device_handle
                .device
                .create_command_encoder(&vello::wgpu::CommandEncoderDescriptor {
                    label: Some("desktop2 blit"),
                });
        self.surface.blitter.copy(
            &device_handle.device,
            &mut encoder,
            &self.surface.target_view,
            &surface_texture
                .texture
                .create_view(&vello::wgpu::TextureViewDescriptor::default()),
        );
        device_handle.queue.submit([encoder.finish()]);
        surface_texture.present();
        Ok(())
    }
}

impl RenderState {
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }
}
