//! jcode-desktop2: greenfield desktop app.
//!
//! Milestone 3+4 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md: winit window,
//! Vello vector rendering, Parley text layout, and a live harness API
//! connection (via jcode-harness-api-bridge) with a minimal chat loop.

mod harness;
mod render;
mod text;

use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use vello::Scene;
use vello::kurbo::{Affine, Circle, RoundedRect};
use vello::peniko::Color;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
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
    model: Model,
    harness: Option<(Receiver<harness::HarnessUpdate>, Sender<String>)>,
}

/// UI model: what the frame is built from.
struct Model {
    status: String,
    session_id: Option<String>,
    transcript: String,
    input: String,
    busy: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            status: "starting...".into(),
            session_id: None,
            transcript: String::new(),
            input: String::new(),
            busy: false,
        }
    }
}

impl App {
    fn drain_harness_updates(&mut self) {
        let Some((updates, _)) = self.harness.as_ref() else {
            return;
        };
        while let Ok(update) = updates.try_recv() {
            match update {
                harness::HarnessUpdate::Status(status) => self.model.status = status,
                harness::HarnessUpdate::Attached { session_id } => {
                    self.model.status = format!("attached: {session_id}");
                    self.model.session_id = Some(session_id);
                }
                harness::HarnessUpdate::Text(text) => self.model.transcript.push_str(&text),
                harness::HarnessUpdate::TurnDone => {
                    self.model.busy = false;
                    self.model.transcript.push('\n');
                }
            }
        }
    }

    fn submit_input(&mut self) {
        if self.model.input.trim().is_empty() || self.model.session_id.is_none() {
            return;
        }
        let content = std::mem::take(&mut self.model.input);
        self.model
            .transcript
            .push_str(&format!("\n> {content}\n\n"));
        self.model.busy = true;
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(content);
        }
    }
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
        let redraw_window = Arc::clone(&window);
        self.harness = Some(harness::spawn(move || redraw_window.request_redraw()));
        let state = pollster::block_on(render::RenderState::new(window)).expect("init gpu");
        self.state = Some(state);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.state.is_none() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(state) = self.state.as_mut() {
                    state.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        text,
                        ..
                    },
                ..
            } => {
                match logical_key {
                    Key::Named(NamedKey::Enter) => self.submit_input(),
                    Key::Named(NamedKey::Backspace) => {
                        self.model.input.pop();
                    }
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    _ => {
                        if let Some(text) = text {
                            for ch in text.chars().filter(|c| !c.is_control()) {
                                self.model.input.push(ch);
                            }
                        }
                    }
                }
                if let Some(state) = self.state.as_ref() {
                    state.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.drain_harness_updates();
                let mut scene = Scene::new();
                if let Some(state) = self.state.as_mut() {
                    build_scene(&mut scene, &mut self.text, &self.model, state.size());
                    if let Err(error) = state.render(&scene) {
                        eprintln!("render error: {error:#}");
                    }
                }
            }
            _ => {}
        }
    }
}

fn build_scene(scene: &mut Scene, text: &mut text::TextSystem, model: &Model, size: (u32, u32)) {
    let (width, height) = (size.0 as f64, size.1 as f64);
    let fill = |scene: &mut Scene, color: Color, shape: &RoundedRect| {
        scene.fill(vello::peniko::Fill::NonZero, Affine::IDENTITY, color, None, shape);
    };

    // Background.
    fill(
        scene,
        Color::from_rgb8(0x14, 0x16, 0x1b),
        &RoundedRect::new(0.0, 0.0, width, height, 0.0),
    );

    // Status bar.
    let status_color = if model.session_id.is_some() {
        Color::from_rgb8(0x6f, 0xc2, 0x8f)
    } else {
        Color::from_rgb8(0xd7, 0xa6, 0x5f)
    };
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        status_color,
        None,
        &Circle::new((28.0, 30.0), 6.0),
    );
    text.draw_paragraph(
        scene,
        &model.status,
        (44.0, 20.0),
        (width - 88.0) as f32,
        13.0,
        Color::from_rgb8(0x9a, 0xa0, 0xac),
    );

    // Transcript card.
    let input_top = height - 92.0;
    fill(
        scene,
        Color::from_rgb8(0x1a, 0x1d, 0x24),
        &RoundedRect::new(20.0, 52.0, width - 20.0, input_top - 12.0, 12.0),
    );
    let transcript = if model.transcript.is_empty() {
        "Type a message and press Enter."
    } else {
        &model.transcript
    };
    // Show the tail of the transcript (no scrolling yet).
    let tail: String = transcript
        .lines()
        .rev()
        .take(((input_top - 90.0) / 22.0) as usize)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    text.draw_paragraph(
        scene,
        &tail,
        (36.0, 68.0),
        (width - 72.0) as f32,
        14.0,
        Color::from_rgb8(0xe8, 0xea, 0xf0),
    );

    // Input card.
    fill(
        scene,
        Color::from_rgb8(0x22, 0x26, 0x2f),
        &RoundedRect::new(20.0, input_top, width - 20.0, height - 20.0, 12.0),
    );
    let prompt = if model.busy {
        "(working...)".to_string()
    } else {
        format!("{}_", model.input)
    };
    text.draw_paragraph(
        scene,
        &prompt,
        (36.0, input_top + 16.0),
        (width - 72.0) as f32,
        15.0,
        Color::from_rgb8(0xcf, 0xd4, 0xdd),
    );
}
