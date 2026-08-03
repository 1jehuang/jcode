//! Offscreen capture: render a scene to a PNG without any window or
//! compositor. Used by `--capture` for self-contained visual verification
//! (agents and CI can inspect the app's real output without screenshots).

use anyhow::{Result, anyhow};
use vello::wgpu::{
    self, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};

/// Render `scene` at `width` x `height` and write a PNG to `path`.
pub fn capture_scene_to_png(
    scene: &Scene,
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> Result<()> {
    let pixels = capture_scene_to_rgba(scene, width, height)?;
    write_png(path, width, height, &pixels)
}

/// Render `scene` offscreen and return tight RGBA8 pixels. Pixel-level tests
/// use this to assert visual invariants (regions stay clear, contrast holds)
/// against the app's real rendered output.
pub fn capture_scene_to_rgba(scene: &Scene, width: u32, height: u32) -> Result<Vec<u8>> {
    let mut context = vello::util::RenderContext::new();
    let device_id = pollster::block_on(context.device(None))
        .ok_or_else(|| anyhow!("no compatible GPU device"))?;
    let device_handle = &context.devices[device_id];
    let device = &device_handle.device;
    let queue = &device_handle.queue;

    // Only Area AA is rendered below, so only its pipelines are compiled.
    let mut renderer = Renderer::new(
        device,
        RendererOptions {
            antialiasing_support: vello::AaSupport::area_only(),
            ..RendererOptions::default()
        },
    )
    .map_err(|error| anyhow!("create renderer: {error}"))?;

    let target = device.create_texture(&TextureDescriptor {
        label: Some("capture target"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer
        .render_to_texture(
            device,
            queue,
            scene,
            &view,
            &RenderParams {
                base_color: vello::peniko::Color::BLACK,
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        )
        .map_err(|error| anyhow!("vello render: {error}"))?;

    // Read the texture back. Rows must be 256-byte aligned for the copy.
    let bytes_per_row = (width * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("capture readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("capture copy"),
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &buffer,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(|error| anyhow!("device poll: {error:?}"))?;
    rx.recv()??;

    // Strip row padding into a tight RGBA buffer.
    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        pixels.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
    }
    drop(mapped);
    buffer.unmap();

    Ok(pixels)
}

/// Write tight RGBA8 pixels out as a PNG. Encoding itself lives in `png`, so
/// the clipboard path can produce the same bytes without a file.
fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    std::fs::write(path, crate::png::encode_rgba(width, height, rgba))?;
    Ok(())
}
