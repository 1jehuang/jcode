//! jcode-desktop2: greenfield desktop app.
//!
//! Milestone 3+4 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md: winit window,
//! Vello vector rendering, Parley text layout, and a live harness API
//! connection (via jcode-harness-api-bridge) with a minimal chat loop.

mod capture;
mod caret;
mod clipboard;
mod editor;
mod harness;
mod keymap;
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

use winit::window::{Window, WindowId};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--keys") {
        print_keys();
        return Ok(());
    }
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

/// `--keys`: print the keybindings ported from the TUI, and the ones that were
/// deliberately skipped. Makes the parity table discoverable to users instead
/// of living only in the source.
fn print_keys() {
    println!("keybindings (ported from the jcode TUI)\n");
    let width = keymap::PORTED
        .iter()
        .map(|row| row.chord.len())
        .max()
        .unwrap_or(0);
    for row in keymap::PORTED {
        println!(
            "  {:<width$}  {:<20}  {}",
            row.chord,
            format!("{:?}", row.action),
            row.tui,
            width = width
        );
    }
    println!("\nnot ported yet:\n");
    for (chord, reason) in keymap::NOT_PORTED {
        println!("  {chord:<width$}  {reason}", width = width);
    }
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
    /// Latest modifier state; winit reports it separately from key events.
    modifiers: winit::keyboard::ModifiersState,
    clipboard: clipboard::Clipboard,
}

/// UI model: what the frame is built from.
pub struct Model {
    pub theme: theme::Theme,
    pub status: String,
    pub session_id: Option<String>,
    pub transcript: String,
    /// The composer: a real text buffer with a cursor, not an append-only
    /// string.
    pub editor: editor::Editor,
    pub caret: caret::Caret,
    pub busy: bool,
    /// Lines scrolled up from the tail. 0 follows the newest output.
    pub scroll: usize,
    /// Transient one-line notice (e.g. "nothing to undo").
    pub notice: Option<String>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            theme: theme::Theme::from_env(),
            status: "starting...".into(),
            session_id: None,
            transcript: String::new(),
            editor: editor::Editor::default(),
            caret: caret::Caret::default(),
            busy: false,
            scroll: 0,
            notice: None,
        }
    }
}

impl Model {
    /// Total transcript lines, used to clamp scrolling.
    fn transcript_lines(&self) -> usize {
        self.transcript.lines().count()
    }

    /// Scroll up by `lines`, clamped so the view cannot run past the top.
    fn scroll_up(&mut self, lines: usize, visible: usize) {
        let max = self.transcript_lines().saturating_sub(visible);
        self.scroll = (self.scroll + lines).min(max);
    }

    /// Scroll down by `lines`; reaching 0 re-follows the tail.
    fn scroll_down(&mut self, lines: usize) {
        self.scroll = self.scroll.saturating_sub(lines);
    }

    fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
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
        if self.model.editor.text().trim().is_empty() {
            return;
        }
        if self.model.session_id.is_none() {
            self.model.set_notice("not attached yet");
            return;
        }
        let content = self.model.editor.take_for_submit();
        self.model
            .transcript
            .push_str(&format!("\n> {content}\n\n"));
        self.model.busy = true;
        // Submitting jumps back to the live tail; otherwise the reply streams
        // in off-screen.
        self.model.scroll = 0;
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(content);
        }
    }

    /// Lines of transcript currently visible, needed to clamp scrolling.
    fn visible_lines(&self) -> usize {
        self.state
            .as_ref()
            .map(|state| {
                layout::Frame::new(state.size(), state.scale_factor()).visible_body_lines()
            })
            .unwrap_or(20)
    }

    /// Apply one resolved action. Returns false when the app should exit, so
    /// quitting stays an explicit outcome rather than a side effect.
    fn apply(&mut self, action: keymap::Action, typed: Option<&str>) -> bool {
        use keymap::Action;
        let page = self.visible_lines().saturating_sub(1).max(1);
        self.model.notice = None;
        match action {
            Action::Insert => {
                if let Some(text) = typed {
                    self.model.editor.insert_str(text);
                }
            }
            Action::Submit => self.submit_input(),
            Action::InsertNewline => self.model.editor.insert_char(' '),

            Action::MoveLeft => self.model.editor.move_left(),
            Action::MoveRight => self.model.editor.move_right(),
            Action::MoveWordLeft => self.model.editor.move_word_left(),
            Action::MoveWordRight => self.model.editor.move_word_right(),
            Action::MoveHome => self.model.editor.move_home(),
            Action::MoveEnd => self.model.editor.move_end(),

            Action::DeleteBack => self.model.editor.delete_back(),
            Action::DeleteForward => self.model.editor.delete_forward(),
            Action::DeleteWordBack => self.model.editor.delete_word_back(),
            Action::DeleteWordForward => self.model.editor.delete_word_forward(),
            Action::KillToStart => {
                let killed = self.model.editor.kill_to_start();
                self.clipboard.set(&killed);
            }
            Action::KillToEnd => {
                let killed = self.model.editor.kill_to_end();
                self.clipboard.set(&killed);
            }
            Action::CutLine => {
                let cut = self.model.editor.cut_line();
                self.clipboard.set(&cut);
            }

            Action::Undo => {
                if !self.model.editor.undo() {
                    self.model.set_notice("nothing to undo");
                }
            }
            Action::Copy => self.clipboard.set(self.model.editor.text()),
            Action::Paste => match self.clipboard.get() {
                Some(text) => self.model.editor.insert_str(&text),
                None => self.model.set_notice("clipboard is empty"),
            },

            Action::HistoryPrev => {
                if !self.model.editor.history_prev() {
                    self.model.set_notice("no earlier input");
                }
            }
            Action::HistoryNext => {
                self.model.editor.history_next();
            }

            Action::ScrollUp => self.model.scroll_up(1, self.visible_lines()),
            Action::ScrollDown => self.model.scroll_down(1),
            Action::PageUp => self.model.scroll_up(page, self.visible_lines()),
            Action::PageDown => self.model.scroll_down(page),
            Action::ScrollTop => {
                let visible = self.visible_lines();
                self.model.scroll_up(usize::MAX / 2, visible);
            }
            Action::ScrollBottom => self.model.scroll = 0,

            // Escape never quits: it cancels, then clears, then re-follows the
            // tail, matching the TUI.
            Action::Cancel => {
                if self.model.busy {
                    self.model.busy = false;
                    self.model.set_notice("interrupting...");
                } else if !self.model.editor.is_empty() {
                    self.model.editor.clear();
                } else {
                    self.model.scroll = 0;
                }
            }
            // Ctrl+C interrupts while busy and only quits when idle with an
            // empty composer, so it cannot discard typed work.
            Action::InterruptOrQuit => {
                if self.model.busy {
                    self.model.busy = false;
                    self.model.set_notice("interrupting...");
                } else if !self.model.editor.is_empty() {
                    self.model.editor.clear();
                } else {
                    return false;
                }
            }
        }
        self.model.caret.touch();
        true
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
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
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
                let action =
                    keymap::resolve(&logical_key, self.modifiers).unwrap_or(keymap::Action::Insert);
                let typed = text.as_ref().map(|t| t.as_str());
                if !self.apply(action, typed) {
                    event_loop.exit();
                    return;
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
                // Wake exactly when the caret next toggles: blinking without a
                // busy redraw loop.
                if let Some(at) = self.model.caret.next_toggle_at(std::time::Instant::now()) {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(at));
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
    // `scroll` counts lines held back from the tail, so 0 follows live output.
    let end = lines
        .len()
        .saturating_sub(model.scroll)
        .max(1)
        .min(lines.len().max(1));
    let lines = &lines[..end];
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

    // Prompt line inside the well: a real input box. The caret is drawn at
    // the measured width of the text before the cursor, so it sits between
    // glyphs and moves with Ctrl+A/E, word motion, and the arrows.
    let prompt_style = ParagraphStyle {
        font_size: layout::BODY_SIZE,
        color: theme.text,
        ..Default::default()
    };
    let prompt_x = frame.left + layout::COMPOSER_PAD_X;
    let prompt_y = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
    let prompt_width = (frame.column() - layout::COMPOSER_PAD_X * 2.0) as f32;

    if model.busy {
        text.draw_paragraph_scaled(
            scene,
            "working...",
            (prompt_x, prompt_y),
            prompt_width,
            ParagraphStyle {
                color: theme.muted,
                ..prompt_style
            },
            scale,
        );
    } else {
        if model.editor.is_empty() {
            text.draw_paragraph_scaled(
                scene,
                "message jcode",
                (prompt_x, prompt_y),
                prompt_width,
                ParagraphStyle {
                    color: theme.faint,
                    ..prompt_style
                },
                scale,
            );
        } else {
            text.draw_paragraph_scaled(
                scene,
                model.editor.text(),
                (prompt_x, prompt_y),
                prompt_width,
                prompt_style,
                scale,
            );
        }
        if model.caret.visible() {
            let offset = text.measure_width(model.editor.before_cursor(), prompt_style, scale);
            let caret_x = (prompt_x + offset).min(frame.right - layout::COMPOSER_PAD_X);
            let top = prompt_y - 1.0;
            let bottom = top + layout::CARET_HEIGHT;
            fill(
                scene,
                theme.text,
                &Rect::new(caret_x, top, caret_x + layout::CARET_WIDTH, bottom),
            );
        }
    }

    // A transient notice, or a scrollback indicator, as a caption under the
    // well. Never covers content.
    let footnote = model
        .notice
        .clone()
        .or_else(|| (model.scroll > 0).then(|| format!("scrolled back {} lines", model.scroll)));
    if let Some(footnote) = footnote {
        text.draw_paragraph_scaled(
            scene,
            &footnote,
            (frame.left, frame.footnote_top),
            frame.column() as f32,
            ParagraphStyle {
                font_size: layout::CAPTION_SIZE,
                color: theme.faint,
                letter_spacing_em: 0.1,
                ..Default::default()
            },
            scale,
        );
    }
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

/// Action-level tests: drive the real `App::apply` dispatch so the wiring
/// between keymap, editor, scrolling, and interrupt semantics is covered, not
/// just the pure modules.
#[cfg(test)]
mod action_tests {
    use super::keymap::Action;
    use super::{App, keymap};
    use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};

    fn app_with(text: &str) -> App {
        let mut app = App::default();
        app.model.session_id = Some("session_test".into());
        app.apply(Action::Insert, Some(text));
        app
    }

    /// Press a chord the way the window event handler does: resolve it, then
    /// apply it. Returns false when the app would exit.
    fn press(app: &mut App, key: Key, mods: ModifiersState, typed: Option<&str>) -> bool {
        let action = keymap::resolve(&key, mods).unwrap_or(Action::Insert);
        app.apply(action, typed)
    }

    fn ch(c: char) -> Key {
        Key::Character(SmolStr::new(c.to_string()))
    }

    #[test]
    fn escape_clears_the_input_instead_of_quitting() {
        // The starter quit the app on Escape, silently losing typed work.
        let mut app = app_with("a draft message");
        assert!(
            press(
                &mut app,
                Key::Named(NamedKey::Escape),
                ModifiersState::empty(),
                None
            ),
            "Escape asked the app to exit"
        );
        assert!(
            app.model.editor.is_empty(),
            "Escape did not clear the input"
        );
    }

    #[test]
    fn escape_on_an_empty_composer_still_does_not_quit() {
        let mut app = App::default();
        assert!(press(
            &mut app,
            Key::Named(NamedKey::Escape),
            ModifiersState::empty(),
            None
        ));
    }

    #[test]
    fn escape_interrupts_a_running_turn_before_clearing_input() {
        let mut app = app_with("keep me");
        app.model.busy = true;
        press(
            &mut app,
            Key::Named(NamedKey::Escape),
            ModifiersState::empty(),
            None,
        );
        assert!(!app.model.busy, "Escape did not interrupt the turn");
        assert_eq!(
            app.model.editor.text(),
            "keep me",
            "Escape cleared the input while interrupting"
        );
    }

    #[test]
    fn ctrl_c_quits_only_when_idle_and_empty() {
        // While busy: interrupt.
        let mut app = App::default();
        app.model.busy = true;
        assert!(press(&mut app, ch('c'), ModifiersState::CONTROL, None));
        assert!(!app.model.busy);

        // With typed text: clear rather than discard the session.
        let mut app = app_with("unsent");
        assert!(press(&mut app, ch('c'), ModifiersState::CONTROL, None));
        assert!(app.model.editor.is_empty());

        // Idle and empty: quit.
        let mut app = App::default();
        assert!(
            !press(&mut app, ch('c'), ModifiersState::CONTROL, None),
            "Ctrl+C on an idle empty composer should quit"
        );
    }

    #[test]
    fn editing_chords_reach_the_editor() {
        let mut app = app_with("alpha beta");
        press(&mut app, ch('a'), ModifiersState::CONTROL, None);
        assert_eq!(app.model.editor.cursor(), 0, "Ctrl+A did not go home");
        press(&mut app, ch('e'), ModifiersState::CONTROL, None);
        assert_eq!(
            app.model.editor.cursor(),
            10,
            "Ctrl+E did not go to the end"
        );
        press(&mut app, ch('w'), ModifiersState::CONTROL, None);
        assert_eq!(
            app.model.editor.text(),
            "alpha ",
            "Ctrl+W did not cut a word"
        );
        press(&mut app, ch('u'), ModifiersState::CONTROL, None);
        assert!(app.model.editor.is_empty(), "Ctrl+U did not kill to start");
        press(&mut app, ch('z'), ModifiersState::CONTROL, None);
        assert_eq!(app.model.editor.text(), "alpha ", "Ctrl+Z did not undo");
    }

    #[test]
    fn cut_then_paste_round_trips_through_the_clipboard() {
        let mut app = app_with("cut me");
        press(&mut app, ch('x'), ModifiersState::CONTROL, None);
        assert!(app.model.editor.is_empty());
        press(&mut app, ch('v'), ModifiersState::CONTROL, None);
        assert_eq!(
            app.model.editor.text(),
            "cut me",
            "paste did not restore the cut"
        );
    }

    #[test]
    fn typing_inserts_at_the_caret_after_moving() {
        let mut app = app_with("ac");
        press(
            &mut app,
            Key::Named(NamedKey::ArrowLeft),
            ModifiersState::empty(),
            None,
        );
        press(&mut app, ch('b'), ModifiersState::empty(), Some("b"));
        assert_eq!(app.model.editor.text(), "abc");
    }

    #[test]
    fn typing_keeps_the_caret_solid() {
        let mut app = App::default();
        press(&mut app, ch('x'), ModifiersState::empty(), Some("x"));
        assert!(
            app.model.caret.visible(),
            "caret was not solid while typing"
        );
    }

    #[test]
    fn history_recall_walks_submitted_messages() {
        let mut app = app_with("first message");
        app.submit_input();
        app.apply(Action::Insert, Some("draft"));
        press(
            &mut app,
            Key::Named(NamedKey::ArrowUp),
            ModifiersState::empty(),
            None,
        );
        assert_eq!(app.model.editor.text(), "first message");
        press(
            &mut app,
            Key::Named(NamedKey::ArrowDown),
            ModifiersState::empty(),
            None,
        );
        assert_eq!(app.model.editor.text(), "draft", "live draft was lost");
    }

    #[test]
    fn submitting_without_a_session_keeps_the_text_and_says_why() {
        let mut app = App::default();
        app.apply(Action::Insert, Some("hello"));
        app.apply(Action::Submit, None);
        assert_eq!(
            app.model.editor.text(),
            "hello",
            "text was discarded while detached"
        );
        assert!(app.model.notice.is_some(), "no notice explained the no-op");
    }

    #[test]
    fn scrolling_clamps_and_returns_to_the_tail() {
        let mut app = App::default();
        app.model.transcript = (1..=100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.apply(Action::ScrollTop, None);
        let top = app.model.scroll;
        assert!(top > 0, "scrolling up did nothing");
        app.apply(Action::ScrollUp, None);
        assert_eq!(app.model.scroll, top, "scroll ran past the top of history");
        app.apply(Action::ScrollBottom, None);
        assert_eq!(app.model.scroll, 0, "did not return to the live tail");
        app.apply(Action::ScrollDown, None);
        assert_eq!(app.model.scroll, 0, "scrolled below the tail");
    }

    #[test]
    fn submitting_jumps_back_to_the_live_tail() {
        let mut app = app_with("question");
        app.model.transcript = (1..=100)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        app.apply(Action::PageUp, None);
        assert!(app.model.scroll > 0);
        app.submit_input();
        assert_eq!(app.model.scroll, 0, "reply would stream in off-screen");
    }

    #[test]
    fn a_notice_is_cleared_by_the_next_keypress() {
        let mut app = App::default();
        app.apply(Action::Undo, None);
        assert!(
            app.model.notice.is_some(),
            "undo with empty stack said nothing"
        );
        press(&mut app, ch('a'), ModifiersState::empty(), Some("a"));
        assert!(app.model.notice.is_none(), "stale notice persisted");
    }

    /// Every action must be dispatchable without panicking, including on an
    /// empty model: a crash on an edge key is worse than a no-op.
    #[test]
    fn every_action_is_safe_on_an_empty_model() {
        let actions = [
            Action::Insert,
            Action::Submit,
            Action::InsertNewline,
            Action::MoveLeft,
            Action::MoveRight,
            Action::MoveWordLeft,
            Action::MoveWordRight,
            Action::MoveHome,
            Action::MoveEnd,
            Action::DeleteBack,
            Action::DeleteForward,
            Action::DeleteWordBack,
            Action::DeleteWordForward,
            Action::KillToStart,
            Action::KillToEnd,
            Action::CutLine,
            Action::Undo,
            Action::Copy,
            Action::Paste,
            Action::HistoryPrev,
            Action::HistoryNext,
            Action::ScrollUp,
            Action::ScrollDown,
            Action::PageUp,
            Action::PageDown,
            Action::ScrollTop,
            Action::ScrollBottom,
            Action::Cancel,
        ];
        for action in actions {
            let mut app = App::default();
            app.apply(action, Some("x"));
        }
    }

    /// Every chord in the parity table must survive real dispatch.
    #[test]
    fn every_ported_chord_dispatches_without_panicking() {
        for row in keymap::PORTED {
            let mut app = app_with("alpha beta gamma");
            app.apply(row.action, Some("x"));
        }
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

    /// A node must render identically no matter when it is rendered, or every
    /// pixel test becomes timing-dependent and flaky.
    #[test]
    #[ignore = "requires a GPU"]
    fn state_nodes_render_deterministically() {
        for (name, model) in nodes() {
            let Some(first) = Rendered::new(&model) else {
                return;
            };
            std::thread::sleep(std::time::Duration::from_millis(700));
            let Some(second) = Rendered::new(&model) else {
                return;
            };
            assert!(
                first.pixels == second.pixels,
                "{name} rendered differently 700ms later (time-dependent frame)"
            );
        }
    }

    /// Columns of ink inside the composer well, as physical x positions.
    /// Used to find the caret without knowing font metrics.
    fn caret_columns(r: &Rendered) -> Vec<u32> {
        let f = r.frame;
        let s = f.scale;
        let y0 = ((f.composer_top + super::layout::COMPOSER_TEXT_OFFSET + 2.0) * s) as u32;
        let y1 = ((f.composer_top + super::layout::COMPOSER_TEXT_OFFSET + 12.0) * s) as u32;
        let x0 = (f.left * s) as u32;
        let x1 = (f.right * s) as u32;
        (x0..x1)
            .filter(|&x| (y0..=y1).all(|y| r.luma(x, y) < 0.5))
            .collect()
    }

    /// A caret is a full-height vertical bar, so it inks every sampled row in
    /// its column. Empty input has no glyphs, so any such column is the caret.
    #[test]
    #[ignore = "requires a GPU"]
    fn an_insert_caret_is_drawn_in_the_empty_composer() {
        let model = states::by_name("attached_empty").expect("node");
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let columns = caret_columns(&r);
        assert!(
            !columns.is_empty(),
            "no insert caret was drawn in the empty composer"
        );
        let f = r.frame;
        let expected = ((f.left + super::layout::COMPOSER_PAD_X) * f.scale) as u32;
        assert!(
            columns.iter().any(|&x| x.abs_diff(expected) <= 4),
            "caret was not at the start of the empty input (columns {:?}, expected ~{expected})",
            &columns[..columns.len().min(8)]
        );
    }

    /// The caret must track the cursor index, which is what makes this a real
    /// input box rather than a trailing underscore. Compared against a caret
    /// rendered on the *same* text with the cursor at the end, so the only
    /// difference is the cursor position.
    #[test]
    #[ignore = "requires a GPU"]
    fn the_caret_moves_with_the_cursor() {
        let mut inside = states::by_name("mid_input_caret_inside").expect("node");
        let mut at_end = states::by_name("mid_input_caret_inside").expect("node");
        at_end.editor.set_cursor_public(at_end.editor.text().len());
        // Same text, same node, different cursor.
        assert_eq!(inside.editor.text(), at_end.editor.text());
        assert!(inside.editor.cursor() < at_end.editor.cursor());
        inside.caret = super::caret::Caret::pinned(true);
        at_end.caret = super::caret::Caret::pinned(true);

        let Some(a) = Rendered::new(&inside) else {
            return;
        };
        let Some(b) = Rendered::new(&at_end) else {
            return;
        };
        let mid = caret_columns(&a);
        let tail = caret_columns(&b);
        assert!(!mid.is_empty(), "no caret drawn with the cursor mid-text");
        assert!(
            !tail.is_empty(),
            "no caret drawn with the cursor at the end"
        );
        let mid_x = *mid.iter().max().expect("columns");
        let tail_x = *tail.iter().max().expect("columns");
        assert!(
            tail_x > mid_x + 20,
            "caret did not follow the cursor: mid-text at {mid_x}, at end {tail_x}"
        );
    }

    /// The blink must actually blink: the off phase draws no caret.
    #[test]
    #[ignore = "requires a GPU"]
    fn the_caret_disappears_on_the_blink_off_phase() {
        let hidden = states::by_name("caret_hidden").expect("node");
        assert!(
            !hidden.caret.visible(),
            "the caret_hidden node is not actually in an off phase"
        );
        let Some(r) = Rendered::new(&hidden) else {
            return;
        };
        // Sample past the end of the text, where only a caret could ink.
        let f = r.frame;
        let text_end = f.left + super::layout::COMPOSER_PAD_X + 200.0;
        let darkest = r.darkest_in(
            text_end,
            f.composer_top + 4.0,
            f.right - 2.0,
            f.composer_bottom - 4.0,
        );
        assert!(
            darkest > 0.85,
            "something was drawn past the text on the blink off phase ({darkest:.3})"
        );
    }

    /// The caret must never escape its well, at any window size.
    #[test]
    #[ignore = "requires a GPU"]
    fn the_caret_stays_inside_the_composer_well() {
        for (name, model) in nodes() {
            let Some(r) = Rendered::new(&model) else {
                return;
            };
            let f = r.frame;
            // Bands immediately above and below the well must stay paper.
            let above = r.darkest_in(f.left, f.composer_top - 6.0, f.right, f.composer_top - 2.0);
            assert!(above > 0.9, "{name}: ink just above the composer well");
            let below = r.darkest_in(
                f.left,
                f.composer_bottom + 1.0,
                f.right,
                f.footnote_top - 1.0,
            );
            assert!(below > 0.9, "{name}: ink between the well and the footnote");
        }
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
            let bottom = r.darkest_in(0.0, f.footnote_bottom + 2.0, f.width - 1.0, f.height - 1.0);
            assert!(bottom > 0.9, "{name}: ink below the footnote row");
        }
    }
}
