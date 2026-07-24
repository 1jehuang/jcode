//! jcode-desktop2: greenfield desktop app.
//!
//! Milestone 3+4 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md: winit window,
//! Vello vector rendering, Parley text layout, and a live harness API
//! connection (via jcode-harness-api-bridge) with a minimal chat loop.

mod capture;
mod harness;
mod render;
mod states;
mod text;
mod theme;

use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use vello::Scene;
use vello::kurbo::Affine;
use vello::peniko::Color;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--capture") {
        return run_capture(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--e2e") {
        return run_e2e(
            args.get(1)
                .map(String::as_str)
                .unwrap_or("Reply with exactly the word: pong"),
        );
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// `--e2e [message]`: headless validation of the app's own harness wiring.
/// Uses the same worker (`harness::spawn`) and model updates as the windowed
/// app: connect, attach, send one message, stream the reply, exit 0 on
/// `TurnDone`. Also renders the final model offscreen to prove the full
/// model -> scene path.
fn run_e2e(message: &str) -> Result<()> {
    let (updates, outgoing) = harness::spawn(|| {});
    let mut model = Model::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut sent = false;
    while std::time::Instant::now() < deadline {
        let Ok(update) = updates.recv_timeout(std::time::Duration::from_secs(1)) else {
            continue;
        };
        match update {
            harness::HarnessUpdate::Status(status) => {
                println!("[e2e] status: {status}");
                if status.starts_with("disconnected") || status.starts_with("error") {
                    anyhow::bail!("harness failure: {status}");
                }
                model.status = status;
            }
            harness::HarnessUpdate::Attached { session_id } => {
                println!("[e2e] attached: {session_id}");
                model.status = format!("attached: {session_id}");
                model.session_id = Some(session_id);
                model.transcript.push_str(&format!("\n> {message}\n\n"));
                outgoing.send(message.to_string())?;
                sent = true;
            }
            harness::HarnessUpdate::Text(text) => {
                print!("{text}");
                model.transcript.push_str(&text);
            }
            harness::HarnessUpdate::TurnDone if sent => {
                println!("\n[e2e] turn done");
                let out = std::env::temp_dir().join("jcode-desktop2-e2e.png");
                let mut text_system = text::TextSystem::default();
                let mut scene = Scene::new();
                build_scene(&mut scene, &mut text_system, &model, (1100, 720));
                capture::capture_scene_to_png(&scene, 1100, 720, &out)?;
                println!("[e2e] final frame -> {}", out.display());
                println!("[e2e] OK");
                return Ok(());
            }
            harness::HarnessUpdate::TurnDone => {}
        }
    }
    anyhow::bail!("e2e timed out")
}

/// `--capture <node|all> [out.png|out_dir]`: render state-space nodes
/// offscreen to PNG for visual verification without a window or compositor.
fn run_capture(args: &[String]) -> Result<()> {
    const WIDTH: u32 = 1100;
    const HEIGHT: u32 = 720;
    let node = args.first().map(String::as_str).unwrap_or("all");
    let mut text = text::TextSystem::default();
    let mut render_node = |name: &str, model: &Model, path: &std::path::Path| -> Result<()> {
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut text, model, (WIDTH, HEIGHT));
        capture::capture_scene_to_png(&scene, WIDTH, HEIGHT, path)?;
        println!("captured {name} -> {}", path.display());
        Ok(())
    };
    if node == "all" {
        let dir = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("captures"));
        std::fs::create_dir_all(&dir)?;
        for name in states::names() {
            let model = states::by_name(name).expect("listed node");
            render_node(name, &model, &dir.join(format!("{name}.png")))?;
        }
        return Ok(());
    }
    let Some(model) = states::by_name(node) else {
        anyhow::bail!(
            "unknown node '{node}'; available: {}",
            states::names().join(", ")
        );
    };
    let out = std::path::PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| format!("{node}.png")),
    );
    render_node(node, &model, &out)
}

#[derive(Default)]
struct App {
    state: Option<render::RenderState>,
    text: text::TextSystem,
    model: Model,
    harness: Option<(Receiver<harness::HarnessUpdate>, Sender<String>)>,
}

/// UI model: what the frame is built from.
pub struct Model {
    pub theme: theme::Theme,
    pub status: String,
    pub session_id: Option<String>,
    pub transcript: String,
    pub input: String,
    pub busy: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            theme: theme::Theme::from_env(),
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
    use text::ParagraphStyle;
    let theme = &model.theme;
    let (width, height) = (size.0 as f64, size.1 as f64);
    let fill = |scene: &mut Scene, color: Color, shape: &vello::kurbo::Rect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            color,
            None,
            shape,
        );
    };
    let hairline = |scene: &mut Scene, y: f64, x0: f64, x1: f64| {
        fill(
            scene,
            theme.rule,
            &vello::kurbo::Rect::new(x0, y, x1, y + 1.0),
        );
    };

    // Paper.
    fill(
        scene,
        theme.background,
        &vello::kurbo::Rect::new(0.0, 0.0, width, height),
    );

    let margin = 48.0;
    let right = width - margin;

    // Masthead: product name (lowercase, bold) and status as a caption.
    text.draw_paragraph_styled(
        scene,
        "jcode",
        (margin, 30.0),
        200.0,
        ParagraphStyle {
            font_size: 17.0,
            bold: true,
            color: theme.text,
            ..Default::default()
        },
    );
    text.draw_paragraph_styled(
        scene,
        &model.status,
        (margin + 110.0, 35.0),
        (right - margin - 110.0) as f32,
        ParagraphStyle {
            font_size: 11.0,
            color: if model.session_id.is_some() {
                theme.muted
            } else {
                theme.faint
            },
            letter_spacing_em: 0.12,
            ..Default::default()
        },
    );
    hairline(scene, 64.0, margin, right);

    // Transcript: ink on paper, measure-limited like body copy.
    let input_rule_y = height - 88.0;
    let transcript = if model.transcript.is_empty() {
        "type a message and press enter."
    } else {
        &model.transcript
    };
    let transcript_color = if model.transcript.is_empty() {
        theme.faint
    } else {
        theme.text
    };
    let line_height_px = 14.0 * 1.65;
    let visible_lines = ((input_rule_y - 96.0) / line_height_px) as usize;
    let tail: String = transcript
        .lines()
        .rev()
        .take(visible_lines.max(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    text.draw_paragraph_styled(
        scene,
        &tail,
        (margin, 84.0),
        (right - margin).min(760.0) as f32,
        ParagraphStyle {
            font_size: 14.0,
            color: transcript_color,
            ..Default::default()
        },
    );

    // Input: a single hairline above the prompt, like a form rule on paper.
    hairline(scene, input_rule_y, margin, right);
    let (prompt, prompt_color) = if model.busy {
        ("working...".to_string(), theme.muted)
    } else {
        (format!("> {}_", model.input), theme.text)
    };
    text.draw_paragraph_styled(
        scene,
        &prompt,
        (margin, input_rule_y + 18.0),
        (right - margin) as f32,
        ParagraphStyle {
            font_size: 14.0,
            color: prompt_color,
            ..Default::default()
        },
    );
}
