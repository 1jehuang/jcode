//! jcode-desktop2: greenfield desktop app.
//!
//! Milestone 3+4 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md: winit window,
//! Vello vector rendering, Parley text layout, and a live harness API
//! connection (via jcode-harness-api-bridge) with a minimal chat loop.

mod capture;
mod harness;
mod layout;
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
/// Build the frame. `size` is the surface size in physical pixels and `scale`
/// is the window scale factor; geometry comes from [`layout::Frame`] in logical
/// units, so the design reads identically on 1x and HiDPI displays.
fn build_scene(
    scene: &mut Scene,
    text: &mut text::TextSystem,
    model: &Model,
    size: (u32, u32),
    scale: f64,
) {
    use layout::Frame;
    use text::ParagraphStyle;
    use vello::kurbo::{Rect, RoundedRect};

    let theme = &model.theme;
    let frame = Frame::new(size, scale);
    let scale = frame.scale;
    let column = frame.column() as f32;

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
    let hairline = |scene: &mut Scene, y: f64| {
        fill(
            scene,
            theme.rule,
            &Rect::new(frame.left, y, frame.right, y + frame.hairline()),
        );
    };

    // Paper.
    fill(
        scene,
        theme.background,
        &Rect::new(0.0, 0.0, frame.width, frame.height),
    );

    // Masthead: wordmark, then status as a caption beside it.
    text.draw_paragraph_scaled(
        scene,
        "jcode",
        (frame.left, frame.masthead_top),
        column,
        ParagraphStyle {
            font_size: layout::WORDMARK_SIZE,
            bold: true,
            color: theme.text,
            letter_spacing_em: 0.02,
            ..Default::default()
        },
        scale,
    );
    // Elide rather than wrap, so the masthead stays one line and never
    // crosses its own rule.
    let status_style = ParagraphStyle {
        font_size: layout::CAPTION_SIZE,
        color: if model.session_id.is_some() {
            theme.muted
        } else {
            theme.faint
        },
        letter_spacing_em: 0.1,
        ..Default::default()
    };
    let status_width = frame.status_width();
    let status_chars = (status_width / (f64::from(status_style.font_size) * 0.72)) as usize;
    let status = elide(&model.status, status_chars.max(12));
    text.draw_paragraph_scaled(
        scene,
        &status,
        (frame.status_left(), frame.masthead_top + 4.0),
        status_width as f32,
        status_style,
        scale,
    );
    hairline(scene, frame.masthead_rule);

    // Composer: a quiet well pinned to the bottom.
    fill_round(
        scene,
        theme.wash,
        &RoundedRect::new(
            frame.left,
            frame.composer_top,
            frame.right,
            frame.composer_bottom,
            layout::COMPOSER_RADIUS,
        ),
    );

    // Transcript: ink on paper, bottom-aligned against the composer so new
    // lines rise from the well rather than dangling from the masthead.
    let placeholder = model.transcript.trim().is_empty();
    let transcript = if placeholder {
        "type a message and press enter"
    } else {
        model.transcript.trim_start_matches('\n')
    };
    let body_style = ParagraphStyle {
        font_size: layout::BODY_SIZE,
        color: if placeholder { theme.faint } else { theme.text },
        line_height: layout::BODY_LEADING as f32,
        ..Default::default()
    };
    // Measure the *wrapped* height so long replies never bleed into the well.
    let available = frame.body_bottom - frame.body_top;
    let lines: Vec<&str> = transcript.lines().collect();
    let mut first_line = lines.len().saturating_sub(frame.visible_body_lines());
    let mut tail = lines[first_line..].join("\n");
    let mut tail_height = text.measure_paragraph(&tail, column, body_style, scale);
    while tail_height > available && first_line < lines.len().saturating_sub(1) {
        first_line += 1;
        tail = lines[first_line..].join("\n");
        tail_height = text.measure_paragraph(&tail, column, body_style, scale);
    }
    let origin_y = if placeholder {
        frame.body_top
    } else {
        (frame.body_bottom - tail_height).max(frame.body_top)
    };
    text.draw_paragraph_scaled(
        scene,
        &tail,
        (frame.left, origin_y),
        column,
        body_style,
        scale,
    );

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
        (
            frame.left + layout::COMPOSER_PAD_X,
            frame.composer_top + 13.0,
        ),
        (frame.column() - layout::COMPOSER_PAD_X * 2.0) as f32,
        ParagraphStyle {
            font_size: layout::BODY_SIZE,
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

/// Pixel-level visual tests: render every state-space node offscreen and
/// assert the invariants from `docs/DESKTOP2_VISUAL_CHECKLIST.md` that only
/// the real rendered output can prove (regions stay clear, text is legible,
/// nothing is clipped). Requires a GPU, so these are ignored by default and
/// run with `cargo test -p jcode-desktop2 -- --ignored`.
#[cfg(test)]
mod visual_tests {
    use super::{Model, build_scene, layout::Frame, states, text::TextSystem};
    use vello::Scene;

    const WIDTH: u32 = 1400;
    const HEIGHT: u32 = 900;
    const SCALE: f64 = 1.75;

    struct Rendered {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        frame: Frame,
    }

    impl Rendered {
        fn new(model: &Model) -> Option<Self> {
            Self::at(model, WIDTH, HEIGHT, SCALE)
        }

        /// Render one model at an explicit surface size and scale factor.
        fn at(model: &Model, width: u32, height: u32, scale: f64) -> Option<Self> {
            let mut text = TextSystem::default();
            let mut scene = Scene::new();
            build_scene(&mut scene, &mut text, model, (width, height), scale);
            let pixels = super::capture::capture_scene_to_rgba(&scene, width, height).ok()?;
            Some(Self {
                pixels,
                width,
                height,
                frame: Frame::new((width, height), scale),
            })
        }

        /// Height in physical pixels of the inked rows within a logical rect.
        /// Used to verify text is rasterized at physical size (HiDPI), not
        /// laid out at 1x and left tiny on a scaled display.
        fn ink_rows(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> u32 {
            let s = self.frame.scale;
            let cx = |v: f64| (v * s).round().clamp(0.0, f64::from(self.width - 1)) as u32;
            let cy = |v: f64| (v * s).round().clamp(0.0, f64::from(self.height - 1)) as u32;
            let (px0, px1) = (cx(x0), cx(x1));
            let mut rows = 0;
            for y in cy(y0)..=cy(y1) {
                if (px0..=px1).any(|x| self.luma(x, y) < 0.6) {
                    rows += 1;
                }
            }
            rows
        }

        /// Luminance at a physical pixel, 0.0 (black) to 1.0 (white).
        fn luma(&self, x: u32, y: u32) -> f64 {
            let i = ((y * self.width + x) * 4) as usize;
            let [r, g, b] = [
                self.pixels[i] as f64,
                self.pixels[i + 1] as f64,
                self.pixels[i + 2] as f64,
            ];
            (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
        }

        /// Darkest luminance inside a logical-unit rect.
        fn darkest_in(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
            let s = self.frame.scale;
            let to_px = |v: f64, max: u32| (v * s).round().clamp(0.0, f64::from(max - 1)) as u32;
            let (px0, py0) = (to_px(x0, self.width), to_px(y0, self.height));
            let (px1, py1) = (to_px(x1, self.width), to_px(y1, self.height));
            let mut darkest = 1.0f64;
            for y in py0..=py1 {
                for x in px0..=px1 {
                    darkest = darkest.min(self.luma(x, y));
                }
            }
            darkest
        }
    }

    fn nodes() -> Vec<(&'static str, Model)> {
        states::names()
            .into_iter()
            .map(|name| (name, states::by_name(name).expect("listed node")))
            .collect()
    }

    #[test]
    #[ignore = "requires a GPU"]
    fn nothing_draws_in_the_gap_above_the_composer() {
        for (name, model) in nodes() {
            let Some(r) = Rendered::new(&model) else {
                eprintln!("skipping {name}: no GPU");
                return;
            };
            let f = r.frame;
            // The band between the transcript and the well must stay paper:
            // this is the overlap bug that made long replies collide.
            let darkest = r.darkest_in(f.left, f.body_bottom + 2.0, f.right, f.composer_top - 2.0);
            assert!(
                darkest > 0.9,
                "{name}: ink ({darkest:.3} luma) in the composer gap"
            );
        }
    }

    #[test]
    #[ignore = "requires a GPU"]
    fn masthead_rule_is_clear_of_text() {
        for (name, model) in nodes() {
            let Some(r) = Rendered::new(&model) else {
                return;
            };
            let f = r.frame;
            // Just below the rule must be paper: status text that wraps past
            // its own rule was the second bug.
            let darkest = r.darkest_in(f.left, f.masthead_rule + 3.0, f.right, f.body_top - 3.0);
            assert!(darkest > 0.9, "{name}: text crossed the masthead rule");
        }
    }

    #[test]
    #[ignore = "requires a GPU"]
    fn body_text_has_readable_contrast() {
        for (name, model) in nodes() {
            let Some(r) = Rendered::new(&model) else {
                return;
            };
            let f = r.frame;
            // Some real ink must exist in the transcript band, dark enough to
            // read. Catches invisible text and silent layout collapse.
            let darkest = r.darkest_in(f.left, f.body_top, f.right, f.body_bottom);
            assert!(
                darkest < 0.65,
                "{name}: transcript is too faint to read (darkest {darkest:.3})"
            );
        }
    }

    /// The founding bug: layout in physical pixels with text laid out at 1x
    /// made everything render tiny and blurry on a 1.75x display. Physical
    /// text height must scale with the scale factor.
    #[test]
    #[ignore = "requires a GPU"]
    fn text_is_rasterized_at_physical_size() {
        let model = states::by_name("turn_done").expect("node");
        const W: u32 = 1100;
        const H: u32 = 720;
        let Some(one) = Rendered::at(&model, W, H, 1.0) else {
            return;
        };
        let Some(two) = Rendered::at(&model, W * 2, H * 2, 2.0) else {
            return;
        };
        let f = one.frame;
        let base = one.ink_rows(f.left, f.body_top, f.right, f.body_bottom);
        let scaled = two.ink_rows(f.left, f.body_top, f.right, f.body_bottom);
        assert!(base > 0 && scaled > 0, "no text was drawn");
        let ratio = f64::from(scaled) / f64::from(base);
        assert!(
            (1.7..=2.3).contains(&ratio),
            "text did not scale with DPI: {base} rows at 1x vs {scaled} at 2x (ratio {ratio:.2})"
        );
    }

    #[test]
    #[ignore = "requires a GPU"]
    fn margins_stay_empty() {
        for (name, model) in nodes() {
            let Some(r) = Rendered::new(&model) else {
                return;
            };
            let f = r.frame;
            // Nothing may be drawn outside the measure column: proves text is
            // wrapped to the column and not clipped by the window edge.
            let left_margin = r.darkest_in(0.0, 0.0, f.left - 3.0, f.height - 1.0);
            assert!(left_margin > 0.9, "{name}: ink in the left margin");
            let bottom = r.darkest_in(0.0, f.composer_bottom + 3.0, f.width - 1.0, f.height - 1.0);
            assert!(bottom > 0.9, "{name}: ink below the composer");
        }
    }
}
