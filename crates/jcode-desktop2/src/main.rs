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
                build_scene(&mut scene, &mut text_system, &model, (1100, 720), 1.0);
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
    // Capture at HiDPI so reviewed frames match what the window shows.
    const SCALE: f64 = 2.0;
    const WIDTH: u32 = 2200;
    const HEIGHT: u32 = 1440;
    let node = args.first().map(String::as_str).unwrap_or("all");
    let mut text = text::TextSystem::default();
    let mut render_node = |name: &str, model: &Model, path: &std::path::Path| -> Result<()> {
        let mut scene = Scene::new();
        build_scene(&mut scene, &mut text, model, (WIDTH, HEIGHT), SCALE);
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
                    let scale = state.scale_factor();
                    build_scene(&mut scene, &mut self.text, &self.model, state.size(), scale);
                    if let Err(error) = state.render(&scene) {
                        eprintln!("render error: {error:#}");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Build the frame. `size` is the surface size in physical pixels and
/// `scale` is the window scale factor; all layout below is in logical units
/// so the design reads identically on 1x and HiDPI displays.
fn build_scene(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    size: (u32, u32),
    scale: f64,
) {
    use text::ParagraphStyle;
    use vello::kurbo::{Rect, RoundedRect};
    let theme = &model.theme;
    let (width, height) = (size.0 as f64 / scale, size.1 as f64 / scale);

    let fill = |scene: &mut Scene, color: Color, shape: &Rect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            color,
            None,
            shape,
        );
    };
    let fill_round = |scene: &mut Scene, color: Color, shape: &RoundedRect| {
        scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::scale(scale),
            color,
            None,
            shape,
        );
    };
    // Hairlines stay one physical pixel regardless of scale.
    let hairline = |scene: &mut Scene, y: f64, x0: f64, x1: f64| {
        fill(scene, theme.rule, &Rect::new(x0, y, x1, y + 1.0 / scale));
    };

    // Paper.
    fill(scene, theme.background, &Rect::new(0.0, 0.0, width, height));

    // Layout: a centered measure column with breathing room, like body copy
    // on a page. Margins shrink gracefully on narrow windows.
    const MEASURE: f64 = 720.0;
    let gutter = (width * 0.06).clamp(20.0, 64.0);
    let column = (width - gutter * 2.0).min(MEASURE);
    let left = ((width - column) / 2.0).max(gutter);
    let right = left + column;

    // Masthead: product name, then status as a right-aligned caption.
    let top = (height * 0.05).clamp(24.0, 44.0);
    text.draw_paragraph_scaled(
        scene,
        "jcode",
        (left, top),
        column as f32,
        ParagraphStyle {
            font_size: 15.0,
            bold: true,
            color: theme.text,
            letter_spacing_em: 0.02,
            ..Default::default()
        },
        scale,
    );
    // Status is a caption beside the wordmark. Elide rather than wrap, so the
    // masthead stays one line and never crosses its own rule.
    let status_x = left + 72.0;
    let status_style = ParagraphStyle {
        font_size: 10.5,
        color: if model.session_id.is_some() {
            theme.muted
        } else {
            theme.faint
        },
        letter_spacing_em: 0.1,
        ..Default::default()
    };
    let status_width = (right - status_x).max(80.0);
    let status_chars = (status_width / (f64::from(status_style.font_size) * 0.72)) as usize;
    let status = elide(&model.status, status_chars.max(12));
    text.draw_paragraph_scaled(
        scene,
        &status,
        (status_x, top + 4.0),
        status_width as f32,
        status_style,
        scale,
    );
    let masthead_rule = top + 28.0;
    hairline(scene, masthead_rule, left, right);

    // Composer: a quiet well pinned to the bottom, sized to the design grid.
    let composer_height = 44.0;
    let composer_bottom = height - (height * 0.05).clamp(20.0, 40.0);
    let composer_top = composer_bottom - composer_height;
    fill_round(
        scene,
        theme.wash,
        &RoundedRect::new(left, composer_top, right, composer_bottom, 6.0),
    );

    // Transcript: ink on paper, bottom-aligned against the composer so new
    // lines rise from the rule rather than dangling from the masthead.
    let body_size = 13.5f32;
    let line_height = f64::from(body_size) * 1.65;
    let body_top = masthead_rule + 22.0;
    let body_bottom = composer_top - 20.0;
    let visible_lines = (((body_bottom - body_top) / line_height) as usize).max(1);
    let placeholder = model.transcript.trim().is_empty();
    let transcript = if placeholder {
        "type a message and press enter"
    } else {
        model.transcript.trim_start_matches('\n')
    };
    let body_style = ParagraphStyle {
        font_size: body_size,
        color: if placeholder { theme.faint } else { theme.text },
        ..Default::default()
    };
    // Bottom-align the tail against the composer, measuring the *wrapped*
    // height so long replies never bleed into the input well.
    let available = body_bottom - body_top;
    let lines: Vec<&str> = transcript.lines().collect();
    let mut first_line = lines.len().saturating_sub(visible_lines);
    let mut tail = lines[first_line..].join("\n");
    let mut tail_height = text.measure_paragraph(&tail, column as f32, body_style, scale);
    while tail_height > available && first_line < lines.len().saturating_sub(1) {
        first_line += 1;
        tail = lines[first_line..].join("\n");
        tail_height = text.measure_paragraph(&tail, column as f32, body_style, scale);
    }
    let origin_y = if placeholder {
        body_top
    } else {
        (body_bottom - tail_height).max(body_top)
    };
    text.draw_paragraph_scaled(scene, &tail, (left, origin_y), column as f32, body_style, scale);

    // Prompt line inside the well.
    let (prompt, prompt_color) = if model.busy {
        ("working...".to_string(), theme.muted)
    } else if model.input.is_empty() {
        (">".to_string(), theme.faint)
    } else {
        (format!("> {}_", model.input), theme.text)
    };
    text.draw_paragraph_scaled(
        scene,
        &prompt,
        (left + 14.0, composer_top + 13.0),
        (column - 28.0) as f32,
        ParagraphStyle {
            font_size: body_size,
            color: prompt_color,
            ..Default::default()
        },
        scale,
    );
}

/// Middle-elide `text` to at most `max_chars` characters, keeping the head and
/// tail (the informative ends of paths, ids, and error strings).
fn elide(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let keep = max_chars - 3;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push_str("...");
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::elide;

    #[test]
    fn elide_keeps_short_text() {
        assert_eq!(elide("attached", 20), "attached");
    }

    #[test]
    fn elide_respects_budget_and_keeps_ends() {
        let out = elide("disconnected: no such file or directory (os error 2)", 24);
        assert_eq!(out.chars().count(), 24);
        assert!(out.starts_with("disconn"));
        assert!(out.ends_with("2)"));
    }

    #[test]
    fn elide_handles_tiny_budget() {
        assert_eq!(elide("abcdef", 2), "...");
    }
}
