//! jcode-desktop2: greenfield desktop app skeleton.
//!
//! Milestone 3 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md. Renders a Vello
//! scene (vector graphics) plus a Parley-laid-out paragraph in a winit
//! window. Everything else (transcript, harness wiring, workspaces) builds
//! on this loop.

mod render;
mod text;

use anyhow::Result;
use std::sync::Arc;
use vello::kurbo::{Affine, Circle, RoundedRect};
use vello::peniko::Color;
use vello::Scene;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

fn main() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[derive(Default)]
struct App {
    state: Option<render::RenderState>,
    text: text::TextSystem,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("jcode desktop2")
                        .with_inner_size(winit::dpi::LogicalSize::new(1100.0, 720.0)),
                )
                .expect("create window"),
        );
        let state = pollster::block_on(render::RenderState::new(window)).expect("init gpu");
        self.state = Some(state);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                let mut scene = Scene::new();
                build_hello_scene(&mut scene, &mut self.text, state.size());
                if let Err(error) = state.render(&scene) {
                    eprintln!("render error: {error:#}");
                }
            }
            _ => {}
        }
    }
}

/// Placeholder scene proving vectors + text both work.
fn build_hello_scene(scene: &mut Scene, text: &mut text::TextSystem, size: (u32, u32)) {
    let (width, height) = (size.0 as f64, size.1 as f64);

    // Background.
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(0x14, 0x16, 0x1b),
        None,
        &RoundedRect::new(0.0, 0.0, width, height, 0.0),
    );

    // A card, the way transcript bubbles will be drawn.
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(0x1f, 0x23, 0x2b),
        None,
        &RoundedRect::new(40.0, 40.0, width - 40.0, 200.0, 14.0),
    );

    // Accent dot, the way status indicators will be drawn.
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(0x6f, 0xc2, 0x8f),
        None,
        &Circle::new((70.0, 70.0), 8.0),
    );

    // Parley-laid-out text.
    text.draw_paragraph(
        scene,
        "jcode desktop2: Vello vectors + Parley text.\n\
         This paragraph is shaped, wrapped, and rendered on the GPU.",
        (95.0, 58.0),
        (width - 160.0) as f32,
        16.0,
        Color::from_rgb8(0xe8, 0xea, 0xf0),
    );
}
