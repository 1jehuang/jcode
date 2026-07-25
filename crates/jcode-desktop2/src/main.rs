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
    if args.first().map(String::as_str) == Some("--script") {
        return run_script(&args[1..]);
    }
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

/// `--script <chord|text> ...`: drive the app with a keystroke script and
/// print the resulting composer state. Verifies real chord sequences end to
/// end without a compositor, which synthetic-input tools make unreliable.
///
///   jcode-desktop2 --script 'type:alpha beta' ctrl+a shift+right shift+right
fn run_script(steps: &[String]) -> Result<()> {
    let mut app = App::default();
    app.model.session_id = Some("session_script".into());
    for step in steps {
        if let Some(text) = step.strip_prefix("type:") {
            app.apply(keymap::Action::Insert, Some(text));
            continue;
        }
        let (key, mods) =
            keymap::parse_chord(step).ok_or_else(|| anyhow::anyhow!("unknown chord '{step}'"))?;
        let action = keymap::resolve(&key, mods).unwrap_or(keymap::Action::Insert);
        if !app.apply(action, None) {
            println!("quit");
            return Ok(());
        }
    }
    let editor = &app.model.editor;
    println!("text: {:?}", editor.text());
    println!("cursor: {}", editor.cursor());
    match editor.selected_text() {
        Some(selected) => println!("selected: {selected:?}"),
        None => println!("selected: none"),
    }
    if let Some(notice) = &app.model.notice {
        println!("notice: {notice}");
    }
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

struct App {
    state: Option<render::RenderState>,
    text: text::TextSystem,
    model: Model,
    harness: Option<(Receiver<harness::HarnessUpdate>, Sender<String>)>,
    /// Latest modifier state; winit reports it separately from key events.
    modifiers: winit::keyboard::ModifiersState,
    clipboard: clipboard::Clipboard,
    /// Pointer position in logical units, tracked for click and drag.
    pointer: (f64, f64),
    /// True while the primary button is held inside the composer.
    dragging: bool,
    /// Last click time and offset, for double-click word selection.
    last_click: Option<(std::time::Instant, usize)>,
    /// Geometry of the most recently built frame. Pointer hit-testing reads
    /// this instead of the GPU state, so input handling is testable without a
    /// window and can never disagree with what was actually drawn.
    frame: layout::Frame,
}

impl Default for App {
    fn default() -> Self {
        Self {
            state: None,
            text: text::TextSystem::default(),
            model: Model::default(),
            harness: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            clipboard: clipboard::Clipboard::default(),
            pointer: (0.0, 0.0),
            dragging: false,
            last_click: None,
            // A sensible frame until the first real one is built, so input
            // before the first paint is still handled sanely.
            frame: layout::Frame::new((1100, 720), 1.0),
        }
    }
}

/// Maximum gap between two clicks that still counts as a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

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

    /// Geometry for a surface. The single source of truth shared by the
    /// renderer and pointer hit-testing: if these ever diverge, clicks land in
    /// the wrong place after a resize.
    /// Geometry for the current model: the composer is sized to the input's
    /// line count, so a multi-line message is fully visible.
    fn frame_for_model(size: (u32, u32), scale: f64, model: &Model) -> layout::Frame {
        layout::Frame::with_composer_lines(size, scale, model.editor.line_count())
    }

    /// Byte offset in the composer text under a logical x position, or `None`
    /// when the pointer is outside the composer well.
    fn composer_offset_at(&mut self, x: f64, y: f64) -> Option<usize> {
        let frame = self.frame;
        // Generous vertical hit area: the whole well, so clicking anywhere in
        // the box focuses the text like a normal input.
        if y < frame.composer_top || y > frame.composer_bottom {
            return None;
        }
        if x < frame.left || x > frame.right {
            return None;
        }
        let scale = frame.scale;
        let text_x = (x - (frame.left + layout::COMPOSER_PAD_X)).max(0.0);
        let style = text::ParagraphStyle {
            font_size: layout::BODY_SIZE,
            ..Default::default()
        };
        // Pick the line from y, then the column within it, so clicking in a
        // multi-line composer lands on the line the user aimed at.
        let editor_text = self.model.editor.text().to_string();
        let lines: Vec<&str> = editor_text.split('\n').collect();
        let shown = frame.composer_lines().min(lines.len());
        let first = lines.len() - shown;
        let text_top = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
        let row = ((y - text_top) / layout::COMPOSER_LINE_HEIGHT).floor();
        let row = (row.max(0.0) as usize).min(shown.saturating_sub(1));
        let index = first + row;
        let line = lines.get(index).copied().unwrap_or("");
        let line_start: usize = lines[..index].iter().map(|l| l.len() + 1).sum();
        let column = self.text.offset_at_x(line, text_x, style, scale);
        Some(line_start + column)
    }

    fn on_pointer_pressed(&mut self) {
        let (x, y) = self.pointer;
        let hit = self.composer_offset_at(x, y);
        if std::env::var_os("JCODE_DESKTOP2_LOG_INPUT").is_some() {
            eprintln!(
                "[input] press at ({x:.1}, {y:.1}) logical; composer y {:.1}..{:.1}; hit {hit:?}",
                self.frame.composer_top, self.frame.composer_bottom
            );
        }
        let Some(offset) = hit else {
            return;
        };
        let now = std::time::Instant::now();
        let double = self
            .last_click
            .is_some_and(|(at, last)| now.duration_since(at) < DOUBLE_CLICK && last == offset);
        if double {
            // Double click selects the word under the pointer.
            let (start, end) = self.model.editor.word_range_at(offset);
            self.model.editor.place_cursor(start);
            self.model.editor.extend_to(end);
            self.last_click = None;
        } else {
            // Shift+click extends from the existing cursor, like normal fields.
            if self.modifiers.shift_key() {
                self.model.editor.extend_to(offset);
            } else {
                self.model.editor.place_cursor(offset);
            }
            self.dragging = true;
            self.last_click = Some((now, offset));
        }
        self.model.caret.touch();
        self.request_redraw();
    }

    fn on_pointer_moved(&mut self) {
        if !self.dragging {
            return;
        }
        let (x, y) = self.pointer;
        // Clamp to the well vertically so dragging out of the box keeps
        // extending rather than dropping the selection.
        let y = y.clamp(
            self.frame.composer_top + 1.0,
            self.frame.composer_bottom - 1.0,
        );
        if let Some(offset) = self.composer_offset_at(x, y) {
            self.model.editor.extend_to(offset);
            self.model.caret.touch();
            self.request_redraw();
        }
    }

    fn request_redraw(&self) {
        if let Some(state) = self.state.as_ref() {
            state.request_redraw();
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
            Action::InsertNewline => self.model.editor.insert_char('\n'),

            Action::MoveLeft => self.model.editor.move_left(),
            Action::MoveRight => self.model.editor.move_right(),
            Action::MoveWordLeft => self.model.editor.move_word_left(),
            Action::MoveWordRight => self.model.editor.move_word_right(),
            Action::MoveHome => self.model.editor.move_home(),
            Action::MoveEnd => self.model.editor.move_end(),

            Action::ExtendLeft => self.model.editor.extend_left(),
            Action::ExtendRight => self.model.editor.extend_right(),
            Action::ExtendWordLeft => self.model.editor.extend_word_left(),
            Action::ExtendWordRight => self.model.editor.extend_word_right(),
            Action::ExtendHome => self.model.editor.extend_home(),
            Action::ExtendEnd => self.model.editor.extend_end(),
            Action::SelectAll => self.model.editor.select_all(),

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
                // Cut the selection when there is one, matching normal fields.
                let cut = match self.model.editor.delete_selection() {
                    Some(selected) => selected,
                    None => self.model.editor.cut_line(),
                };
                self.clipboard.set(&cut);
            }

            Action::Undo => {
                if !self.model.editor.undo() {
                    self.model.set_notice("nothing to undo");
                }
            }
            Action::Copy => {
                // Copy the selection when there is one, else the whole line.
                let text = self
                    .model
                    .editor
                    .selected_text()
                    .unwrap_or_else(|| self.model.editor.text())
                    .to_string();
                self.clipboard.set(&text);
            }
            Action::Paste => match self.clipboard.get() {
                Some(text) => self.model.editor.insert_str(&text),
                None => self.model.set_notice("clipboard is empty"),
            },

            // In a multi-line input, Up/Down move between lines first and only
            // fall through to history recall at the edges, like a normal
            // multi-line composer.
            Action::HistoryPrev => {
                if !self.model.editor.move_line(-1) && !self.model.editor.history_prev() {
                    self.model.set_notice("no earlier input");
                }
            }
            Action::HistoryNext => {
                if !self.model.editor.move_line(1) {
                    self.model.editor.history_next();
                }
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
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .state
                    .as_ref()
                    .map(|state| state.scale_factor())
                    .unwrap_or(1.0);
                self.pointer = (position.x / scale, position.y / scale);
                if std::env::var_os("JCODE_DESKTOP2_LOG_INPUT").is_some() {
                    eprintln!(
                        "[input] move to ({:.1}, {:.1}) logical",
                        self.pointer.0, self.pointer.1
                    );
                }
                self.on_pointer_moved();
            }
            WindowEvent::MouseInput {
                state: element_state,
                button: winit::event::MouseButton::Left,
                ..
            } => match element_state {
                ElementState::Pressed => self.on_pointer_pressed(),
                ElementState::Released => self.dragging = false,
            },
            WindowEvent::MouseWheel { delta, .. } => {
                // Scrolling the transcript with the wheel, in line units.
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        pos.y / (layout::BODY_SIZE as f64 * layout::BODY_LEADING)
                    }
                };
                let steps = lines.abs().round().max(1.0) as usize;
                if lines > 0.0 {
                    self.model.scroll_up(steps, self.visible_lines());
                } else if lines < 0.0 {
                    self.model.scroll_down(steps);
                }
                self.request_redraw();
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
                    let size = state.size();
                    // Record the geometry the frame was built with, so pointer
                    // hit-testing uses exactly what the user sees.
                    self.frame = Self::frame_for_model(size, scale, &self.model);
                    build_scene(&mut scene, &mut self.text, &self.model, size, scale);
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
    let frame = Frame::with_composer_lines(size, scale, model.editor.line_count());
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
        let line_height = layout::COMPOSER_LINE_HEIGHT;
        let lines: Vec<&str> = model.editor.text().split('\n').collect();
        // Show the tail of a long input, so the caret stays visible while
        // typing past the composer's line cap.
        let shown = frame.composer_lines().min(lines.len());
        let first = lines.len() - shown;
        let line_y = |line: usize| prompt_y + (line.saturating_sub(first)) as f64 * line_height;
        // Byte offset of the start of each line, for selection and the caret.
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut at = 0usize;
        for line in &lines {
            line_starts.push(at);
            at += line.len() + 1;
        }

        // Selection band, drawn under the text so glyphs stay legible. A
        // selection can span lines, so each visible line gets its own band.
        if let Some((sel_start, sel_end)) = model.editor.selection() {
            for (index, line) in lines.iter().enumerate().skip(first) {
                let line_start = line_starts[index];
                let line_end = line_start + line.len();
                let from = sel_start.max(line_start);
                let to = sel_end.min(line_end);
                if from >= to && !(sel_start <= line_end && sel_end > line_end) {
                    continue;
                }
                let from = from.min(line_end);
                let to = to.max(from);
                let x0 =
                    prompt_x + text.measure_width(&line[..from - line_start], prompt_style, scale);
                let x1 =
                    prompt_x + text.measure_width(&line[..to - line_start], prompt_style, scale);
                // A selection continuing past this line highlights the break.
                let x1 = if sel_end > line_end {
                    x1 + f64::from(layout::BODY_SIZE) * 0.5
                } else {
                    x1
                };
                if x1 <= x0 {
                    continue;
                }
                let top = line_y(index) - 1.0;
                fill(
                    scene,
                    theme.selection,
                    &Rect::new(
                        x0.min(frame.right),
                        top,
                        x1.min(frame.right - layout::COMPOSER_PAD_X * 0.5),
                        top + layout::CARET_HEIGHT,
                    ),
                );
            }
        }

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
            // Draw line by line so each sits on the composer's line grid
            // rather than relying on paragraph wrapping.
            for (index, line) in lines.iter().enumerate().skip(first) {
                if line.is_empty() {
                    continue;
                }
                text.draw_paragraph_scaled(
                    scene,
                    line,
                    (prompt_x, line_y(index)),
                    prompt_width,
                    prompt_style,
                    scale,
                );
            }
        }

        if model.caret.visible() {
            let (line, col) = model.editor.cursor_line_col();
            let prefix = &lines[line][..col.min(lines[line].len())];
            let offset = text.measure_width(prefix, prompt_style, scale);
            let caret_x = (prompt_x + offset).min(frame.right - layout::COMPOSER_PAD_X);
            let top = line_y(line.max(first)) - 1.0;
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
    use super::{App, DOUBLE_CLICK, Model, keymap};
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

    // --- mouse ---

    /// A composer-relative x for the given byte offset, so mouse tests do not
    /// hardcode font metrics.
    fn x_for_offset(app: &mut App, offset: usize) -> f64 {
        let frame = app.frame;
        x_for_offset_in(app, offset, frame)
    }

    fn x_for_offset_in(app: &mut App, offset: usize, frame: super::layout::Frame) -> f64 {
        let style = super::text::ParagraphStyle {
            font_size: super::layout::BODY_SIZE,
            ..Default::default()
        };
        let text = app.model.editor.text()[..offset].to_string();
        frame.left
            + super::layout::COMPOSER_PAD_X
            + app.text.measure_width(&text, style, frame.scale)
    }

    /// Mouse tests need a laid-out window, which needs a GPU surface. Instead
    /// of a real window, exercise the offset math and editor transitions
    /// directly through the same helpers the events call.
    #[test]
    fn hit_testing_maps_x_to_the_nearest_character_gap() {
        let mut app = app_with("alpha beta");
        let style = super::text::ParagraphStyle {
            font_size: super::layout::BODY_SIZE,
            ..Default::default()
        };
        let text = app.model.editor.text().to_string();
        // Clicking left of the text lands at 0; far right lands at the end.
        assert_eq!(app.text.offset_at_x(&text, -50.0, style, 1.0), 0);
        assert_eq!(
            app.text.offset_at_x(&text, 10_000.0, style, 1.0),
            text.len()
        );
        // Clicking at the measured width of a prefix lands on that boundary.
        for offset in [0, 1, 5, 6, text.len()] {
            let width = app.text.measure_width(&text[..offset], style, 1.0);
            assert_eq!(
                app.text.offset_at_x(&text, width, style, 1.0),
                offset,
                "click at the boundary of offset {offset} missed"
            );
        }
    }

    #[test]
    fn hit_testing_never_splits_a_character() {
        let mut app = app_with("héllo wörld 🌼");
        let style = super::text::ParagraphStyle {
            font_size: super::layout::BODY_SIZE,
            ..Default::default()
        };
        let text = app.model.editor.text().to_string();
        for step in 0..200 {
            let offset = app.text.offset_at_x(&text, step as f64 * 3.0, style, 1.0);
            assert!(
                text.is_char_boundary(offset),
                "hit test returned a mid-character offset"
            );
        }
    }

    /// Logical y in the middle of the composer well.
    fn composer_y(app: &App) -> f64 {
        (app.frame.composer_top + app.frame.composer_bottom) / 2.0
    }

    /// A full press/release at a logical x, through the real handlers.
    fn click(app: &mut App, x: f64) {
        let y = composer_y(app);
        app.pointer = (x, y);
        app.on_pointer_pressed();
        app.dragging = false;
    }

    /// Press at `from`, drag to `to`, release: the real selection gesture.
    fn drag(app: &mut App, from: f64, to: f64) {
        let y = composer_y(app);
        app.pointer = (from, y);
        app.on_pointer_pressed();
        app.pointer = (to, y);
        app.on_pointer_moved();
        app.dragging = false;
    }

    /// Clicking at a character's x must put the caret there, going through the
    /// same measurement the renderer uses.
    #[test]
    fn clicking_places_the_caret_at_the_clicked_character() {
        let mut app = app_with("alpha beta");
        app.model.editor.select_all();
        for offset in [0usize, 3, 6, 10] {
            let x = x_for_offset(&mut app, offset);
            click(&mut app, x);
            assert_eq!(
                app.model.editor.cursor(),
                offset,
                "clicking at offset {offset} placed the caret at {}",
                app.model.editor.cursor()
            );
            assert_eq!(
                app.model.editor.selection(),
                None,
                "a plain click left a selection behind"
            );
        }
    }

    #[test]
    fn clicking_outside_the_composer_is_ignored() {
        let mut app = app_with("alpha beta");
        app.model.editor.place_cursor(2);
        let x = x_for_offset(&mut app, 8);
        // Well above the composer: in the transcript area.
        app.pointer = (x, app.frame.body_top + 4.0);
        app.on_pointer_pressed();
        assert_eq!(
            app.model.editor.cursor(),
            2,
            "a click in the transcript moved the composer caret"
        );
        assert!(!app.dragging, "a click outside the well started a drag");
    }

    #[test]
    fn dragging_selects_the_text_between_press_and_release() {
        let mut app = app_with("alpha beta");
        let from = x_for_offset(&mut app, 0);
        let to = x_for_offset(&mut app, 5);
        drag(&mut app, from, to);
        assert_eq!(app.model.editor.selected_text(), Some("alpha"));
    }

    #[test]
    fn dragging_right_to_left_selects_the_same_range() {
        let mut app = app_with("alpha beta");
        let a = x_for_offset(&mut app, 6);
        let b = x_for_offset(&mut app, 10);
        drag(&mut app, b, a);
        assert_eq!(app.model.editor.selected_text(), Some("beta"));
    }

    #[test]
    fn dragging_above_or_below_the_well_keeps_extending() {
        // Dragging out of the box must not drop the selection.
        let mut app = app_with("alpha beta");
        let from = x_for_offset(&mut app, 0);
        let to = x_for_offset(&mut app, 5);
        app.pointer = (from, composer_y(&app));
        app.on_pointer_pressed();
        let below = app.frame.composer_bottom + 200.0;
        app.pointer = (to, below);
        app.on_pointer_moved();
        assert_eq!(app.model.editor.selected_text(), Some("alpha"));
    }

    #[test]
    fn double_clicking_selects_the_word_under_the_pointer() {
        let mut app = app_with("alpha beta gamma");
        let x = x_for_offset(&mut app, 8);
        click(&mut app, x);
        // Second click at the same spot, within the double-click window.
        app.pointer = (x, composer_y(&app));
        app.on_pointer_pressed();
        app.dragging = false;
        assert_eq!(app.model.editor.selected_text(), Some("beta"));
    }

    #[test]
    fn two_slow_clicks_are_not_a_double_click() {
        let mut app = app_with("alpha beta gamma");
        let x = x_for_offset(&mut app, 8);
        click(&mut app, x);
        // Simulate the gap expiring.
        app.last_click = Some((std::time::Instant::now() - DOUBLE_CLICK * 2, 8));
        app.pointer = (x, composer_y(&app));
        app.on_pointer_pressed();
        assert_eq!(
            app.model.editor.selection(),
            None,
            "two slow clicks selected a word"
        );
    }

    #[test]
    fn shift_clicking_extends_from_the_existing_caret() {
        let mut app = app_with("alpha beta");
        let home = x_for_offset(&mut app, 0);
        click(&mut app, home);
        app.modifiers = ModifiersState::SHIFT;
        let to = x_for_offset(&mut app, 5);
        app.pointer = (to, composer_y(&app));
        app.on_pointer_pressed();
        assert_eq!(app.model.editor.selected_text(), Some("alpha"));
    }

    #[test]
    fn a_pointer_move_without_a_press_changes_nothing() {
        let mut app = app_with("alpha beta");
        app.model.editor.place_cursor(3);
        app.pointer = (x_for_offset(&mut app, 9), composer_y(&app));
        app.on_pointer_moved();
        assert_eq!(app.model.editor.cursor(), 3);
        assert_eq!(app.model.editor.selection(), None);
    }

    #[test]
    fn releasing_ends_the_drag_so_later_moves_do_not_select() {
        let mut app = app_with("alpha beta");
        let from = x_for_offset(&mut app, 0);
        let to = x_for_offset(&mut app, 5);
        drag(&mut app, from, to);
        let selected = app.model.editor.selection();
        app.pointer = (x_for_offset(&mut app, 10), composer_y(&app));
        app.on_pointer_moved();
        assert_eq!(
            app.model.editor.selection(),
            selected,
            "selection changed after the button was released"
        );
    }

    #[test]
    fn clicking_keeps_the_caret_solid() {
        let mut app = app_with("alpha");
        let x = x_for_offset(&mut app, 2);
        click(&mut app, x);
        assert!(app.model.caret.visible(), "caret blinked out on click");
    }

    /// The frame used for input must be the frame the scene was built with.
    /// Rendering records it; this asserts the recorded value matches what
    /// `build_scene` would use for the same surface.
    #[test]
    fn the_recorded_frame_matches_the_rendered_geometry() {
        for (size, scale) in [
            ((1100u32, 720u32), 1.0f64),
            ((2400, 1400), 1.75),
            ((800, 600), 2.0),
        ] {
            let recorded = App::frame_for_model(size, scale, &Model::default());
            let rendered = super::layout::Frame::new(size, scale);
            assert_eq!(
                recorded, rendered,
                "input geometry diverged from the rendered frame at {size:?} @ {scale}"
            );
        }
    }

    /// A resize must be reflected in hit-testing, not just in drawing.
    #[test]
    fn resizing_moves_the_hit_test_with_the_layout() {
        let mut app = app_with("alpha beta");
        let narrow = App::frame_for_model((700, 600), 1.0, &Model::default());
        let wide = App::frame_for_model((2000, 1200), 1.0, &Model::default());
        assert_ne!(
            narrow.left, wide.left,
            "test needs two frames with different columns"
        );
        // The same logical x means different offsets in different layouts, so
        // clicking the same character requires the current frame.
        app.frame = wide;
        let x = x_for_offset_in(&mut app, 5, wide);
        click(&mut app, x);
        assert_eq!(app.model.editor.cursor(), 5);

        app.frame = narrow;
        // Clear the click history: clicking the same offset twice in quick
        // succession is legitimately a double click.
        app.last_click = None;
        let x = x_for_offset_in(&mut app, 5, narrow);
        click(&mut app, x);
        assert_eq!(
            app.model.editor.cursor(),
            5,
            "hit-testing did not follow the resized layout"
        );
    }

    #[test]
    fn clicking_a_lower_line_lands_on_that_line() {
        let mut app = app_with("first line\nsecond line\nthird line");
        app.frame = App::frame_for_model((1400, 900), 1.0, &app.model);
        let text_top = app.frame.composer_top + super::layout::COMPOSER_TEXT_OFFSET;
        for (row, expected_line) in [(0usize, 0usize), (1, 1), (2, 2)] {
            let y = text_top + row as f64 * super::layout::COMPOSER_LINE_HEIGHT + 4.0;
            app.pointer = (app.frame.left + super::layout::COMPOSER_PAD_X + 1.0, y);
            app.last_click = None;
            app.on_pointer_pressed();
            app.dragging = false;
            assert_eq!(
                app.model.editor.cursor_line_col().0,
                expected_line,
                "clicking row {row} landed on the wrong line"
            );
        }
    }

    #[test]
    fn shift_enter_makes_a_new_line_and_enter_still_submits() {
        let mut app = app_with("first");
        app.apply(Action::InsertNewline, None);
        app.apply(Action::Insert, Some("second"));
        assert_eq!(app.model.editor.text(), "first\nsecond");
        app.apply(Action::Submit, None);
        assert!(
            app.model.editor.is_empty(),
            "Enter did not submit a multi-line message"
        );
        assert!(app.model.transcript.contains("first\nsecond"));
    }

    #[test]
    fn up_moves_between_lines_before_recalling_history() {
        let mut app = app_with("one");
        app.apply(Action::Submit, None);
        app.apply(Action::Insert, Some("line a"));
        app.apply(Action::InsertNewline, None);
        app.apply(Action::Insert, Some("line b"));
        // Cursor is on line 1: Up moves to line 0, not into history.
        app.apply(Action::HistoryPrev, None);
        assert_eq!(app.model.editor.cursor_line_col().0, 0);
        assert_eq!(app.model.editor.text(), "line a\nline b");
        // Another Up is at the top edge, so now history recall takes over.
        app.apply(Action::HistoryPrev, None);
        assert_eq!(app.model.editor.text(), "one");
    }

    #[test]
    fn down_moves_between_lines_before_returning_from_history() {
        let mut app = app_with("alpha\nbeta");
        app.apply(Action::MoveHome, None);
        app.apply(Action::HistoryPrev, None); // already at top, no history
        app.apply(Action::HistoryNext, None);
        assert_eq!(
            app.model.editor.cursor_line_col().0,
            1,
            "Down did not move to the next line"
        );
    }

    #[test]
    fn the_composer_frame_follows_the_input_line_count() {
        let mut app = app_with("one");
        let single = App::frame_for_model((1400, 900), 1.0, &app.model);
        app.apply(Action::InsertNewline, None);
        app.apply(Action::Insert, Some("two"));
        let double = App::frame_for_model((1400, 900), 1.0, &app.model);
        assert!(
            double.composer_top < single.composer_top,
            "the composer did not grow when a line was added"
        );
    }

    #[test]
    fn hit_testing_uses_the_frame_that_was_actually_drawn() {
        // If input used a different frame than the renderer, clicks would land
        // in the wrong place after a resize.
        let mut app = app_with("alpha beta");
        app.frame = super::layout::Frame::new((2400, 1400), 2.0);
        let frame = app.frame;
        let x = x_for_offset_in(&mut app, 5, frame);
        click(&mut app, x);
        assert_eq!(app.model.editor.cursor(), 5);
    }

    #[test]
    fn the_wheel_scrolls_and_clamps_like_the_keyboard() {
        let mut app = App::default();
        app.model.transcript = (1..=100)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let visible = app.visible_lines();
        app.model.scroll_up(3, visible);
        assert_eq!(app.model.scroll, 3);
        app.model.scroll_down(99);
        assert_eq!(app.model.scroll, 0, "wheel scrolled past the tail");
    }

    #[test]
    fn copy_prefers_the_selection_over_the_whole_line() {
        let mut app = app_with("alpha beta");
        app.model.editor.place_cursor(0);
        app.model.editor.extend_to(5);
        app.apply(Action::Copy, None);
        assert_eq!(app.clipboard.get().as_deref(), Some("alpha"));
    }

    #[test]
    fn cut_removes_only_the_selection_when_there_is_one() {
        let mut app = app_with("alpha beta");
        app.model.editor.place_cursor(0);
        app.model.editor.extend_to(6);
        app.apply(Action::CutLine, None);
        assert_eq!(app.model.editor.text(), "beta");
        assert_eq!(app.clipboard.get().as_deref(), Some("alpha "));
    }

    #[test]
    fn shift_arrow_selection_then_typing_replaces_it() {
        let mut app = app_with("alpha beta");
        app.apply(Action::MoveHome, None);
        for _ in 0..5 {
            app.apply(Action::ExtendRight, None);
        }
        app.apply(Action::Insert, Some("omega"));
        assert_eq!(app.model.editor.text(), "omega beta");
    }

    #[test]
    fn select_all_then_delete_empties_the_composer() {
        let mut app = app_with("throw this away");
        app.apply(Action::SelectAll, None);
        app.apply(Action::DeleteBack, None);
        assert!(app.model.editor.is_empty());
        app.apply(Action::Undo, None);
        assert_eq!(app.model.editor.text(), "throw this away");
    }

    /// `--script` is the manual verification path; if the chord spellings it
    /// accepts drift from the parity table, scripted checks silently stop
    /// testing what they claim to.
    #[test]
    fn every_ported_chord_is_scriptable() {
        for row in keymap::PORTED {
            // The table documents some rows as alternatives (e.g. "ctrl+j / ...")
            // only in NOT_PORTED; ported rows must all parse.
            assert!(
                keymap::parse_chord(row.chord).is_some(),
                "ported chord '{}' cannot be scripted",
                row.chord
            );
        }
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
                // Must match the frame `build_scene` used, which sizes the
                // composer to the model's line count.
                frame: Frame::with_composer_lines(
                    (width, height),
                    scale,
                    model.editor.line_count(),
                ),
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

    /// A selection must be visible as a band, and the selected glyphs must
    /// still be readable on top of it.
    #[test]
    #[ignore = "requires a GPU"]
    fn a_selection_is_visible_and_text_on_it_stays_readable() {
        let model = states::by_name("selection").expect("node");
        let (start, end) = model.editor.selection().expect("node has a selection");
        assert!(start < end);
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let band_y = f.composer_top + super::layout::COMPOSER_TEXT_OFFSET + 6.0;
        // Somewhere in the selection there must be a mid-tone band pixel that
        // is neither paper nor ink.
        let s = f.scale;
        let y = (band_y * s) as u32;
        let mut band_pixels = 0;
        let mut ink_pixels = 0;
        for x in ((f.left * s) as u32)..((f.right * s) as u32) {
            let luma = r.luma(x, y);
            if (0.55..0.95).contains(&luma) {
                band_pixels += 1;
            }
            if luma < 0.4 {
                ink_pixels += 1;
            }
        }
        assert!(band_pixels > 4, "no selection band was drawn");
        assert!(
            ink_pixels > 0,
            "selected text was hidden by the band instead of drawn on top"
        );
    }

    /// No selection means no band: otherwise the composer would always look
    /// highlighted.
    #[test]
    #[ignore = "requires a GPU"]
    fn no_band_is_drawn_without_a_selection() {
        let model = states::by_name("mid_input").expect("node");
        assert!(model.editor.selection().is_none());
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let s = f.scale;
        // Sample a row above the glyph bodies where a band would still paint.
        let y = ((f.composer_top + super::layout::COMPOSER_TEXT_OFFSET + 1.0) * s) as u32;
        let band_pixels = (((f.left + 2.0) * s) as u32..((f.right - 2.0) * s) as u32)
            .filter(|&x| (0.55..0.95).contains(&r.luma(x, y)))
            .count();
        assert!(
            band_pixels < 10,
            "a selection band appeared without a selection ({band_pixels} px)"
        );
    }

    /// A multi-line message must actually render on multiple rows, with the
    /// caret on the last line rather than the first.
    #[test]
    #[ignore = "requires a GPU"]
    fn a_multiline_message_renders_on_multiple_rows() {
        let model = states::by_name("multiline").expect("node");
        assert!(model.editor.line_count() > 1);
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        assert!(
            f.composer_lines() >= model.editor.line_count(),
            "the composer did not grow to fit the input"
        );
        // Each line's row must contain ink.
        let s = f.scale;
        for line in 0..model.editor.line_count() {
            let y = f.composer_top
                + super::layout::COMPOSER_TEXT_OFFSET
                + line as f64 * super::layout::COMPOSER_LINE_HEIGHT
                + 6.0;
            let row = (y * s) as u32;
            let inked = ((f.left * s) as u32..(f.right * s) as u32)
                .filter(|&x| r.luma(x, row) < 0.5)
                .count();
            assert!(inked > 0, "composer line {line} rendered nothing");
        }
    }

    /// A selection spanning a line break must highlight both lines.
    #[test]
    #[ignore = "requires a GPU"]
    fn a_selection_across_lines_highlights_every_line() {
        let model = states::by_name("multiline_selection").expect("node");
        let Some(r) = Rendered::new(&model) else {
            return;
        };
        let f = r.frame;
        let s = f.scale;
        for line in 0..2 {
            let y = f.composer_top
                + super::layout::COMPOSER_TEXT_OFFSET
                + line as f64 * super::layout::COMPOSER_LINE_HEIGHT
                + 2.0;
            let row = (y * s) as u32;
            let band = ((f.left * s) as u32..(f.right * s) as u32)
                .filter(|&x| (0.55..0.95).contains(&r.luma(x, row)))
                .count();
            assert!(
                band > 4,
                "line {line} of a multi-line selection was not highlighted"
            );
        }
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
