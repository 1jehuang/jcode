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
mod meta;
mod render;
mod scene;
mod states;
#[cfg(test)]
mod tests;
mod text;
mod theme;
mod window_state;
mod wrap;

use anyhow::Result;
use scene::build_scene;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use vello::Scene;
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
    /// Current mouse pointer shape, tracked so it is only set when it changes.
    cursor_icon: winit::window::CursorIcon,
    /// Window size and position, persisted so the app reopens as it was left.
    geometry: window_state::Geometry,
    /// When the geometry was last written, and what was written, so resizing
    /// does not hit the disk on every event.
    geometry_saved: Option<(std::time::Instant, window_state::Geometry)>,
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
            cursor_icon: winit::window::CursorIcon::Default,
            geometry: window_state::Geometry::default(),
            geometry_saved: None,
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
    /// Build identity shown in the masthead: version, updates, account.
    pub meta: meta::Meta,
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
            meta: meta::Meta::detect(),
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
    /// Visual rows of the composer for a character budget, so layout sizing,
    /// rendering, and hit-testing all wrap identically.
    fn composer_rows(&self, max_chars: usize) -> Vec<wrap::Row> {
        wrap::wrap(self.editor.text(), max_chars)
    }

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
        // Size from *wrapped* rows: a single long line still needs the room it
        // occupies on screen, or it would spill outside the well.
        let probe = layout::Frame::new(size, scale);
        let rows = model.composer_rows(probe.composer_char_budget());
        layout::Frame::with_composer_lines(size, scale, rows.len())
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
        // Pick the visual row from y, then the column within it, so clicking a
        // wrapped row lands where the user aimed.
        let source = self.model.editor.text().to_string();
        let rows = crate::wrap::wrap(&source, frame.composer_char_budget());
        let shown = frame.composer_lines().min(rows.len());
        let first = rows.len() - shown;
        let text_top = frame.composer_top + layout::COMPOSER_TEXT_OFFSET;
        let offset_row = ((y - text_top) / layout::COMPOSER_LINE_HEIGHT).floor();
        let offset_row = (offset_row.max(0.0) as usize).min(shown.saturating_sub(1));
        let row = rows[first + offset_row];
        let column = self
            .text
            .offset_at_x(row.text(&source), text_x, style, scale);
        Some(row.start + column)
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

    /// Persist the window geometry if it changed and the throttle has elapsed.
    /// Saving as we go (rather than only on exit) means the size survives a
    /// crash or a kill.
    fn save_geometry(&mut self, force: bool) {
        let now = std::time::Instant::now();
        if !force && !self.geometry.should_save(self.geometry_saved, now) {
            return;
        }
        if self.geometry_saved.map(|(_, g)| g) == Some(self.geometry.sanitized()) && !force {
            return;
        }
        self.geometry.save();
        self.geometry_saved = Some((now, self.geometry.sanitized()));
    }

    /// Whether a logical point is inside the composer well.
    fn in_composer(&self, x: f64, y: f64) -> bool {
        y >= self.frame.composer_top
            && y <= self.frame.composer_bottom
            && x >= self.frame.left
            && x <= self.frame.right
    }

    /// Show a text caret over the composer and the default arrow elsewhere, so
    /// the input box looks editable before it is clicked.
    fn update_cursor_icon(&mut self) {
        let (x, y) = self.pointer;
        let wanted = if self.in_composer(x, y) {
            winit::window::CursorIcon::Text
        } else {
            winit::window::CursorIcon::Default
        };
        if self.cursor_icon != wanted {
            self.cursor_icon = wanted;
            if let Some(state) = self.state.as_ref() {
                state.set_cursor_icon(wanted);
            }
        }
    }

    fn on_pointer_moved(&mut self) {
        self.update_cursor_icon();
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
        // Reopen where the user left off.
        let geometry = window_state::Geometry::load();
        let mut attributes = Window::default_attributes()
            .with_title("jcode desktop2")
            .with_inner_size(winit::dpi::LogicalSize::new(
                geometry.width,
                geometry.height,
            ));
        if let Some((x, y)) = geometry.position {
            attributes = attributes.with_position(winit::dpi::LogicalPosition::new(x, y));
        }
        self.geometry = geometry;
        let window = Arc::new(event_loop.create_window(attributes).expect("create window"));
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
            WindowEvent::CloseRequested => {
                self.save_geometry(true);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(state) = self.state.as_mut() {
                    state.resize(size.width, size.height);
                    let scale = state.scale_factor();
                    self.geometry.width = f64::from(size.width) / scale;
                    self.geometry.height = f64::from(size.height) / scale;
                }
                self.save_geometry(false);
            }
            WindowEvent::Moved(position) => {
                let scale = self
                    .state
                    .as_ref()
                    .map(|state| state.scale_factor())
                    .unwrap_or(1.0);
                self.geometry.position =
                    Some((f64::from(position.x) / scale, f64::from(position.y) / scale));
                self.save_geometry(false);
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
                    self.save_geometry(true);
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
