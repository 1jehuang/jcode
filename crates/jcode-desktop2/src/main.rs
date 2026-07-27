//! jcode-desktop2: greenfield desktop app.
//!
//! Milestone 3+4 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md: winit window,
//! Vello vector rendering, Parley text layout, and a live harness API
//! connection (via jcode-harness-api-bridge) with a minimal chat loop.

mod activity;
mod app_selection;
mod capture;
mod caret;
mod cli;
mod clipboard;
mod donut;
mod editor;
mod harness;
mod hints;
mod input;
mod keymap;
mod layout;
mod meta;
mod paint;
mod place;
mod profile;
mod render;
mod scene;
mod select;
mod states;
mod stream;
mod strip;
#[cfg(test)]
mod tests;
mod text;
mod theme;
mod transcript;
mod viewport;
mod window_state;

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
    // Every entry point that runs *instead of* the window lives in `cli`, so
    // this stays a router rather than accumulating subcommands.
    if let Some(result) = cli::dispatch(&args) {
        return result;
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    state: Option<render::RenderState>,
    painter: paint::Painter,
    model: Model,
    harness: Option<(Receiver<harness::HarnessUpdate>, Sender<harness::Command>)>,
    /// Latest modifier state; winit reports it separately from key events.
    modifiers: winit::keyboard::ModifiersState,
    clipboard: clipboard::Clipboard,
    /// Pointer position in logical units, tracked for click and drag.
    pointer: (f64, f64),
    /// True while the primary button is held inside the composer.
    dragging: bool,
    /// True while the primary button is held over the transcript, extending a
    /// transcript selection. Distinct from `dragging`: the two surfaces have
    /// separate selections, and one gesture must only ever drive one of them.
    selecting: bool,
    /// Last click time and offset, for double-click word selection.
    last_click: Option<(std::time::Instant, usize)>,
    /// Consecutive clicks at one spot in the transcript, which decide whether
    /// a press selects a caret, a word, or a line.
    click_streak: select::ClickStreak,
    /// Current mouse pointer shape, tracked so it is only set when it changes.
    cursor_icon: winit::window::CursorIcon,
    /// Window size and position, persisted so the app reopens as it was left.
    geometry: window_state::Geometry,
    /// When the geometry was last written, and what was written, so resizing
    /// does not hit the disk on every event.
    geometry_saved: Option<(std::time::Instant, window_state::Geometry)>,
    /// When the last animated frame was drawn, for the donut's time step.
    last_frame: Option<std::time::Instant>,
    /// Geometry of the most recently built frame. Pointer hit-testing reads
    /// this instead of the GPU state, so input handling is testable without a
    /// window and can never disagree with what was actually drawn.
    frame: layout::Frame,
}

impl Default for App {
    fn default() -> Self {
        Self {
            state: None,
            painter: paint::Painter::default(),
            model: Model::default(),
            harness: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            clipboard: clipboard::Clipboard::default(),
            pointer: (0.0, 0.0),
            dragging: false,
            selecting: false,
            last_click: None,
            click_streak: select::ClickStreak::default(),
            cursor_icon: winit::window::CursorIcon::Default,
            geometry: window_state::Geometry::default(),
            geometry_saved: None,
            last_frame: None,
            // A sensible frame until the first real one is built, so input
            // before the first paint is still handled sanely.
            frame: layout::Frame::new((1100, 720), 1.0),
        }
    }
}

/// Frame interval while the donut animates (~60fps).
const DONUT_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Maximum gap between two clicks that still counts as a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// UI model: what the frame is built from.
pub struct Model {
    pub theme: theme::Theme,
    /// Build identity shown in the masthead: version, updates, account.
    pub meta: meta::Meta,
    pub status: String,
    pub session_id: Option<String>,
    pub transcript: transcript::Transcript,
    /// The composer: a real text buffer with a cursor, not an append-only
    /// string.
    pub editor: editor::Editor,
    pub caret: caret::Caret,
    pub busy: bool,
    /// What the agent is doing right now, and the spinner that proves the app
    /// is still alive mid-turn.
    pub activity: activity::Activity,
    /// Whether the window has keyboard focus. The field border and the caret
    /// both key off this: an unfocused input that still shows a blinking caret
    /// lies about where typing will go.
    pub focused: bool,
    /// Logical pixels scrolled up from the tail. 0 follows the newest output.
    ///
    /// Pixels rather than lines: the screen moves in pixels, and a wrapped
    /// paragraph has more visual rows than newlines, so a line-based scroll
    /// and the display disagree the moment anything wraps.
    pub scroll: f64,
    /// The transcript selection, when the user has dragged over the
    /// conversation. Held in the model rather than in `App` so a frame stays a
    /// pure function of the model and the highlight can be captured in a
    /// pixel test without a window.
    pub selection: Option<select::Selection>,
    /// Transient one-line notice (e.g. "nothing to undo").
    pub notice: Option<String>,
    /// The hero donut's luminance field, or `None` when the donut is off
    /// (reduced motion, or a headless capture that wants a still frame).
    pub donut: Option<donut::Donut>,
    /// The donut's animation clock and drag momentum.
    pub spin: donut::Spin,
    /// Which ghost hint the empty composer shows. An index rather than a
    /// string, so the model stays trivially comparable and captures can pin it.
    pub hint: usize,
    /// The streaming reveal: how much of the arriving reply is on screen, and
    /// the glide that keeps a growing transcript from jumping.
    pub stream: stream::Stream,
    /// Live sessions, drawn as the strip at the top of the window.
    pub strip: strip::Strip,
    /// Working directory of the attached session, as the daemon reports it.
    /// `None` until attach, because a guess here is worse than silence: it is
    /// the fact that decides whether an answer applies to your project.
    pub working_dir: Option<String>,
    /// Provider and model serving this session, once the harness reports it.
    /// `None` until then, so the caption appears rather than showing a guess
    /// that could be wrong.
    pub model: Option<ModelId>,
}

/// The provider and model answering this session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelId {
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl ModelId {
    /// One-line caption, or `None` when there is nothing to say.
    ///
    /// The model id alone is the useful fact ("sonnet-4.5"), so the provider is
    /// only shown when the model is unknown: `anthropic / claude-sonnet-4` is
    /// mostly the same word twice.
    pub fn caption(&self) -> Option<String> {
        match (self.model.as_deref(), self.provider.as_deref()) {
            (Some(model), _) if !model.is_empty() => Some(model.to_string()),
            (_, Some(provider)) if !provider.is_empty() => Some(provider.to_string()),
            _ => None,
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self {
            theme: theme::Theme::from_env(),
            meta: meta::Meta::detect(),
            status: "starting...".into(),
            session_id: None,
            transcript: transcript::Transcript::default(),
            editor: editor::Editor::default(),
            caret: caret::Caret::default(),
            busy: false,
            activity: activity::Activity::default(),
            focused: true,
            scroll: 0.0,
            selection: None,
            notice: None,
            donut: (!donut_disabled()).then(|| donut::Donut::new(DONUT_GRID)),
            spin: donut::Spin::default(),
            hint: hints::arbitrary_index(),
            stream: stream::Stream::default(),
            strip: strip::Strip::default(),
            working_dir: None,
            model: None,
        }
    }
}

/// Luminance grid resolution for the donut: twice the halftone dot count, so
/// every dot integrates a 2x2 neighbourhood of the field (built-in spatial AA).
/// This is the website's top quality step; the desktop can hold it because the
/// march is parallel and the halftone screen is vector output rather than a
/// per-pixel canvas fill.
pub const DONUT_GRID: usize = 152;

/// Escape hatch: `JCODE_DESKTOP2_DONUT=0` turns the animation off for users who
/// do not want motion, and for benchmarking the rest of the frame.
fn donut_disabled() -> bool {
    matches!(
        std::env::var("JCODE_DESKTOP2_DONUT").as_deref(),
        Ok("0") | Ok("off") | Ok("false")
    )
}

impl Model {
    /// Scroll up by `amount` logical pixels, clamped to `max` so the view
    /// cannot run past the top of the conversation into blank space.
    fn scroll_up(&mut self, amount: f64, max: f64) {
        self.scroll = (self.scroll + amount).clamp(0.0, max.max(0.0));
    }

    /// Scroll down by `amount` logical pixels; reaching 0 re-follows the tail.
    fn scroll_down(&mut self, amount: f64) {
        self.scroll = (self.scroll - amount).max(0.0);
    }

    fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    /// The status line to show as a footnote, or `None` when it is not worth
    /// the user's attention.
    ///
    /// A healthy connection is the expected case, so saying "attached:
    /// session_..." forever is noise; it was the worst of the clutter the old
    /// masthead carried. Startup progress and failures are worth showing,
    /// because otherwise a dead runtime looks like an app that simply ignores
    /// your input.
    pub fn status_footnote(&self) -> Option<String> {
        if self.session_id.is_some() {
            return None;
        }
        let status = self.status.trim();
        if status.is_empty() {
            return None;
        }
        Some(status.to_string())
    }

    /// The footnote line for this frame, in priority order.
    ///
    /// One row, one message: a transient notice beats a scrollback indicator,
    /// which beats a connection problem, which beats a build alert. Choosing
    /// here rather than in the renderer keeps the decision testable.
    pub fn footnote(&self) -> Option<String> {
        if let Some(notice) = &self.notice {
            return Some(notice.clone());
        }
        // The activity line normally lives in the empty composer. When the user
        // has already typed the next message the ghost line is gone, so it
        // moves down here rather than disappearing: "is it working?" must be
        // answerable in every state, not just the idle-composer one.
        if self.busy
            && !self.editor.is_empty()
            && let Some(line) = self.activity.line(std::time::Instant::now())
        {
            return Some(line);
        }
        if self.scroll > 0.0 {
            return Some("scrolled back".to_string());
        }
        self.status_footnote().or_else(|| self.meta.alert())
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
                harness::HarnessUpdate::Attached {
                    session_id,
                    working_dir,
                } => {
                    self.model.status = format!("attached: {session_id}");
                    self.model.strip.focus_session(&session_id);
                    self.model.session_id = Some(session_id);
                    self.model.working_dir = working_dir;
                    self.retitle();
                }
                harness::HarnessUpdate::Model { provider, model } => {
                    self.model.model = Some(ModelId { provider, model });
                }
                harness::HarnessUpdate::Text(text) => {
                    self.model.transcript.append_assistant(&text);
                    // Chase the new length rather than jumping to it: the
                    // reveal is what turns a burst of tokens into a sweep.
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::Reasoning(text) => {
                    self.model.transcript.append_reasoning(&text);
                    // Reasoning is revealed by the same sweep as the reply, so
                    // a thought does not appear as an instant wall of text
                    // while the answer below it types itself out.
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::Activity(label) => {
                    self.model.busy = true;
                    self.model
                        .activity
                        .set_label(label, std::time::Instant::now());
                }
                harness::HarnessUpdate::TurnDone => {
                    self.model.busy = false;
                    self.model.activity.finish();
                }
                harness::HarnessUpdate::Sessions(entries) => {
                    // Rebuild around the session we are actually attached to,
                    // so a refresh never silently moves the highlight off the
                    // conversation currently on screen.
                    self.model.strip =
                        strip::Strip::build(entries, self.model.session_id.as_deref());
                }
            }
        }
    }

    /// Switch to whichever session the strip now points at.
    ///
    /// The transcript belongs to the session, so it is cleared rather than
    /// carried across: appending another conversation's output to the one on
    /// screen would be actively misleading. Reloading real history needs
    /// `GetHistory`; until that is wired, an empty page is the honest state.
    fn attach_focused_session(&mut self) {
        let Some(target) = self.model.strip.focused_session().map(str::to_string) else {
            return;
        };
        if self.model.session_id.as_deref() == Some(target.as_str()) {
            return;
        }
        self.model.transcript = transcript::Transcript::default();
        self.model.stream.reveal_all();
        self.model.busy = false;
        self.model.activity.finish();
        self.model.scroll = 0.0;
        self.model.status = format!("attaching: {target}");
        self.model.session_id = Some(target.clone());
        // The new session's directory arrives with its `Attached` event; until
        // then show the strip's entry rather than the previous session's path,
        // which would name the wrong project.
        self.model.working_dir = self.model.strip.focused_working_dir().map(str::to_string);
        self.retitle();
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(harness::Command::Attach(target));
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
        // Move to the next hint, so the set is discovered across turns instead
        // of one line being the whole of the user's experience of it.
        self.model.hint = self.model.hint.wrapping_add(1);
        self.model
            .transcript
            .push(transcript::Message::user(content.clone()));
        // The user's own message was typed, not streamed: revealing it would
        // animate text they are already looking at.
        self.model.stream.reveal_all();
        self.model.busy = true;
        self.model.activity.start(std::time::Instant::now());
        // Submitting jumps back to the live tail; otherwise the reply streams
        // in off-screen.
        self.model.scroll = 0.0;
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(harness::Command::Send(content));
        }
    }

    /// Geometry for a surface. The single source of truth shared by the
    /// renderer and pointer hit-testing: if these ever diverge, clicks land in
    /// the wrong place after a resize.
    /// Geometry for the current model: the composer is sized to the input's
    /// line count, so a multi-line message is fully visible.
    ///
    /// The line count comes from a real Parley layout rather than a character
    /// budget, so the well is sized by where the text actually wraps.
    /// Only reached from tests and captures now; the live path measures
    /// through the app's warm painter so the transcript is not laid out twice.
    #[cfg_attr(not(test), allow(dead_code))]
    fn frame_for_model(size: (u32, u32), scale: f64, model: &Model) -> layout::Frame {
        let mut painter = paint::Painter::default();
        Self::frame_for_model_with(size, scale, model, &mut painter)
    }

    /// As [`Self::frame_for_model`], reusing an existing text system. Font and
    /// layout contexts are expensive, so the render path passes its own.
    pub fn frame_for_model_with(
        size: (u32, u32),
        scale: f64,
        model: &Model,
        painter: &mut paint::Painter,
    ) -> layout::Frame {
        let paint::Painter {
            text,
            transcript: cache,
        } = painter;
        let probe = layout::Frame::new(size, scale);
        let lines = crate::input::InputLayout::new(
            text,
            model.editor.text(),
            probe.composer_text_width(),
            scene::composer_text_style(model),
            probe.scale,
        )
        .line_count();
        // The top chrome row is reserved when it has something to say: more
        // than one session to move between (the strip proper), or a working
        // directory to name. With neither it would be a widget saying "1 of 1"
        // about nowhere, so nothing is reserved and the page is unchanged.
        let strip = model.strip.len() > 1 || model.working_dir.is_some();
        // Measure the conversation so the composer can sit just under the
        // last reply while it is short, instead of floating at the middle of
        // the page with a gap above it. Content height is a function of the
        // measure column only, which does not depend on where the well ends
        // up, so there is no circularity here.
        let width = (probe.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
        let laid = cache.lay_out(
            text,
            &model.transcript,
            width,
            &model.theme,
            scene::transcript_body_style(model),
            probe.scale,
        );
        let content = crate::viewport::Viewport::new(laid, 0.0, 0.0).content_height;
        layout::Frame::with_content(size, scale, lines, strip, content)
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
        // Hit-test against the same Parley layout the renderer draws, so a
        // click lands on the glyph under the pointer even with proportional
        // fonts, clusters, or bidi text.
        let source = self.model.editor.text().to_string();
        let input = crate::input::InputLayout::new(
            &mut self.painter.text,
            &source,
            frame.composer_text_width(),
            scene::composer_text_style(&self.model),
            frame.scale,
        );
        let origin_y = frame.composer_top + layout::COMPOSER_TEXT_OFFSET
            - input.scroll_offset(self.model.editor.cursor(), frame.composer_lines());
        Some(input.offset_at_point(x - frame.composer_text_left(), y - origin_y))
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
            // In the transcript: start a text selection there. The
            // conversation is the part of the window worth quoting from, so a
            // drag over it has to select rather than do nothing.
            if self.in_transcript(x, y) && self.press_in_transcript(x, y) {
                return;
            }
            // Outside the well: the donut is the only other interactive thing,
            // so a press on it starts a spin drag (as on the website).
            if self.donut_visible() && self.frame.hits_donut(x, y) {
                self.model.spin.press(x);
                self.update_cursor_icon();
                self.request_redraw();
            }
            return;
        };
        // A press in the composer drops any transcript selection: two live
        // highlights would leave Ctrl+C with no honest answer about which one
        // it copies.
        if self.model.selection.take().is_some() {
            self.request_redraw();
        }
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
            // A double click completes on press, not release, so it publishes
            // here rather than waiting for a drag that will never come.
            self.publish_primary_selection();
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

    /// The primary button was released: end the gesture, drop an empty
    /// highlight, and publish a real one.
    ///
    /// A named method rather than an inline arm so the tests drive the same
    /// code the window does. A test that reimplemented this would keep passing
    /// after the real handler stopped auto-copying.
    fn on_pointer_released(&mut self) {
        let was_selecting = self.selecting || self.dragging;
        self.dragging = false;
        self.selecting = false;
        // A click that selected nothing clears the highlight, so a stale band
        // cannot outlive the gesture that made it.
        if self
            .model
            .selection
            .is_some_and(|selection| selection.is_empty())
        {
            self.model.selection = None;
            self.request_redraw();
        }
        // Finishing a selection auto-copies it to the primary selection, so
        // middle click pastes what you just highlighted. The ordinary
        // clipboard is untouched, which is what makes this safe on every drag.
        if was_selecting {
            self.publish_primary_selection();
        }
        // Releasing hands the donut its momentum; it keeps spinning down on
        // its own.
        self.model.spin.release();
        self.update_cursor_icon();
    }

    /// Show a text caret over the composer and the default arrow elsewhere, so
    /// the input box looks editable before it is clicked.
    fn update_cursor_icon(&mut self) {
        let (x, y) = self.pointer;
        let wanted = if self.in_composer(x, y) {
            winit::window::CursorIcon::Text
        } else if self.in_transcript(x, y) {
            // The transcript is selectable, so it must say so before it is
            // dragged; an arrow over selectable text reads as inert.
            winit::window::CursorIcon::Text
        } else if self.model.spin.dragging {
            winit::window::CursorIcon::Grabbing
        } else if self.donut_visible() && self.frame.hits_donut(x, y) {
            winit::window::CursorIcon::Grab
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

    /// Whether the donut is on screen: enabled, and the session is still empty
    /// (the same condition `build_scene` draws it under).
    fn donut_visible(&self) -> bool {
        self.model.donut.is_some() && self.model.transcript.is_empty()
    }

    fn on_pointer_moved(&mut self) {
        if self.model.spin.dragging {
            self.model.spin.drag_to(self.pointer.0);
            self.request_redraw();
            return;
        }
        self.update_cursor_icon();
        if self.selecting {
            // Dragging at the edge scrolls the conversation under the pointer,
            // so a selection is not capped at one screenful.
            let scrolled = self.autoscroll_for_drag(self.pointer.1);
            // Clamp into the region so dragging past the top or bottom keeps
            // extending to the nearest text instead of dropping the gesture.
            let (x, y) = self.pointer;
            let y = y.clamp(self.frame.body_top + 1.0, self.frame.body_bottom - 1.0);
            if let Some(position) = self.transcript_position_at(x, y)
                && let Some(selection) = self.model.selection.as_mut()
            {
                // A drag is a gesture in its own right, so it ends the click
                // streak: pressing again afterwards is a fresh click, not the
                // second half of a double click that would grab a word.
                if selection.focus != position {
                    self.click_streak.interrupt();
                }
                selection.focus = position;
                self.request_redraw();
            } else if scrolled {
                self.request_redraw();
            }
            return;
        }
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

    /// Whether the donut should be driving frames. Decorative motion is not
    /// worth waking the GPU for when the window is not focused or the donut is
    /// not on screen, which is what keeps an idle window off the CPU.
    fn donut_animating(&self) -> bool {
        self.donut_visible() && self.model.focused
    }

    /// Advance the donut one frame and rebuild its luminance field. Skipped
    /// entirely when the donut is not being drawn, so a busy session pays
    /// nothing for it.
    fn animate_donut(&mut self) {
        if !self.donut_animating() {
            return;
        }
        let now = std::time::Instant::now();
        let dt = self
            .last_frame
            .map(|last| now.duration_since(last).as_secs_f32())
            // Clamp so a stall (or a laptop resuming from sleep) does not jump
            // the animation forward by a visible lurch.
            .map_or(DONUT_FRAME.as_secs_f32(), |dt| dt.min(0.1));
        self.last_frame = Some(now);
        self.model.spin.advance(dt);
        let (time, offset) = (self.model.spin.time, self.model.spin.offset);
        if let Some(field) = self.model.donut.as_mut() {
            field.render(time, offset);
        }
    }

    /// Put the session's directory in the window title, so the compositor's
    /// window list distinguishes two jcode windows on different checkouts.
    fn retitle(&self) {
        if let Some(state) = self.state.as_ref() {
            state.set_title(&place::window_title(self.model.working_dir.as_deref()));
        }
    }

    fn request_redraw(&self) {
        if let Some(state) = self.state.as_ref() {
            state.request_redraw();
        }
    }

    /// When the loop must next wake to repaint, or `None` when nothing on
    /// screen is animating.
    ///
    /// Three things want frames: the blinking caret, the donut while it is on
    /// screen, and the activity spinner while a turn runs. All go through this
    /// one function so they cannot fight over `ControlFlow`, and the earliest
    /// deadline wins.
    ///
    /// `None` matters as much as `Some`: an idle window must sleep rather than
    /// spin, so the states that animate nothing (no window focus, a pinned
    /// caret with no donut and no turn in flight) return `None` here.
    pub fn animation_deadline(&self, now: std::time::Instant) -> Option<std::time::Instant> {
        if !self.model.focused {
            return None;
        }
        let caret = (!self.model.busy)
            .then(|| self.model.caret.next_toggle_at(now))
            .flatten();
        let donut = self.donut_animating().then(|| now + DONUT_FRAME);
        // A running turn animates the activity spinner; this is what keeps the
        // window from looking frozen while the agent works.
        let spinner = self.model.activity.next_frame_at(now);
        // The reveal and the scroll glide are the only animations that must
        // keep running while the window is busy, and they stop themselves the
        // moment the text has caught up.
        let stream = self.model.stream.next_frame_at(now);
        [caret, donut, spinner, stream].into_iter().flatten().min()
    }

    /// Height of the transcript region in logical units.
    fn transcript_region_height(&self) -> f64 {
        (self.frame.body_bottom - self.frame.body_top).max(1.0)
    }

    /// Furthest the transcript may scroll, in logical pixels.
    ///
    /// This measures the real laid-out conversation rather than counting
    /// newlines, so the clamp agrees with what is drawn even when a single
    /// streamed paragraph wraps into a screenful.
    fn max_scroll(&mut self) -> f64 {
        let frame = self.frame;
        let style = crate::scene::transcript_body_style(&self.model);
        let width = (frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
        let region = self.transcript_region_height();
        // Split the borrows: the viewport needs the text system mutably while
        // reading the model, so both are taken from `self` up front.
        let App {
            painter,
            model: state,
            ..
        } = self;
        let paint::Painter {
            text,
            transcript: cache,
        } = painter;
        let laid = cache.lay_out(
            text,
            &state.transcript,
            width,
            &state.theme,
            style,
            frame.scale,
        );
        crate::viewport::Viewport::new(laid, region, state.scroll).max_scroll()
    }

    /// Apply one resolved action. Returns false when the app should exit, so
    /// quitting stays an explicit outcome rather than a side effect.
    fn apply(&mut self, action: keymap::Action, typed: Option<&str>) -> bool {
        use keymap::Action;
        // A page is the region minus one line of overlap, so scrolling keeps
        // a row of context rather than jumping blind.
        let page = (self.transcript_region_height() - self.frame.body_line_height()).max(1.0);
        let line = self.frame.body_line_height();
        self.model.notice = None;
        match action {
            Action::Insert => {
                if let Some(text) = typed {
                    self.model.editor.insert_str(text);
                }
            }
            Action::Submit => self.submit_input(),

            // Strip motion moves the highlight and attaches in one step:
            // a selection you then have to confirm would be a second
            // interaction for something the user already asked for.
            Action::SessionLeft => {
                if self.model.strip.focus_left() {
                    self.attach_focused_session();
                }
            }
            Action::SessionRight => {
                if self.model.strip.focus_right() {
                    self.attach_focused_session();
                }
            }
            Action::SessionUp => {
                if self.model.strip.focus_up() {
                    self.attach_focused_session();
                }
            }
            Action::SessionDown => {
                if self.model.strip.focus_down() {
                    self.attach_focused_session();
                }
            }
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
                self.copy_to(clipboard::Target::Clipboard, &killed);
            }
            Action::KillToEnd => {
                let killed = self.model.editor.kill_to_end();
                self.copy_to(clipboard::Target::Clipboard, &killed);
            }
            Action::CutLine => {
                // Cut the selection when there is one, matching normal fields.
                let cut = match self.model.editor.delete_selection() {
                    Some(selected) => selected,
                    None => self.model.editor.cut_line(),
                };
                self.copy_to(clipboard::Target::Clipboard, &cut);
            }

            Action::Undo => {
                if !self.model.editor.undo() {
                    self.model.set_notice("nothing to undo");
                }
            }
            Action::Copy => {
                // A transcript highlight wins: it is the visible selection, and
                // copying the composer instead would silently paste something
                // the user never highlighted.
                if let Some(text) = self.selected_transcript_text() {
                    self.copy_to(clipboard::Target::Clipboard, &text);
                    return true;
                }
                // Copy the selection when there is one, else the whole line.
                let text = self
                    .model
                    .editor
                    .selected_text()
                    .unwrap_or_else(|| self.model.editor.text())
                    .to_string();
                self.copy_to(clipboard::Target::Clipboard, &text);
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

            Action::ScrollUp => {
                let max = self.max_scroll();
                self.model.scroll_up(line, max);
            }
            Action::ScrollDown => self.model.scroll_down(line),
            Action::PageUp => {
                let max = self.max_scroll();
                self.model.scroll_up(page, max);
            }
            Action::PageDown => self.model.scroll_down(page),
            Action::ScrollTop => {
                let max = self.max_scroll();
                self.model.scroll_up(max, max);
            }
            Action::ScrollBottom => self.model.scroll = 0.0,

            // Escape never quits: it cancels, then clears, then re-follows the
            // tail, matching the TUI.
            Action::Cancel => {
                // A visible highlight is the most recent thing the user did,
                // so Escape dismisses that first rather than reaching past it
                // to clear typed work.
                if self.model.selection.take().is_some() {
                } else if self.model.busy {
                    self.model.busy = false;
                    self.model.set_notice("interrupting...");
                } else if !self.model.editor.is_empty() {
                    self.model.editor.clear();
                } else {
                    self.model.scroll = 0.0;
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
        // A keyboard selection auto-copies exactly like a mouse one: the two
        // ways of highlighting text must not behave differently.
        if action.changes_the_selection() {
            self.publish_primary_selection();
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
            .with_title(place::window_title(self.model.working_dir.as_deref()))
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
                ElementState::Released => self.on_pointer_released(),
            },
            // Middle click pastes the primary selection, the other half of the
            // same convention: select to copy, middle click to paste.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: winit::event::MouseButton::Middle,
                ..
            } => {
                self.paste_primary_selection();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Scrolling the transcript with the wheel, in logical pixels
                // so a trackpad's fine-grained deltas are not quantised to
                // whole lines.
                let pixels = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => {
                        f64::from(y) * self.frame.body_line_height()
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y / self.frame.scale,
                };
                if pixels > 0.0 {
                    let max = self.max_scroll();
                    self.model.scroll_up(pixels, max);
                } else if pixels < 0.0 {
                    self.model.scroll_down(-pixels);
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::Focused(focused) => {
                self.model.focused = focused;
                // Restart the blink phase on focus so the caret is immediately
                // solid rather than appearing mid-off-phase.
                if focused {
                    self.model.caret.touch();
                }
                self.request_redraw();
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
                self.animate_donut();
                self.model.stream.advance(std::time::Instant::now());
                let mut scene = Scene::new();
                if let Some(state) = self.state.as_mut() {
                    let scale = state.scale_factor();
                    let size = state.size();
                    // Record the geometry the frame was built with, so pointer
                    // hit-testing uses exactly what the user sees. Measured
                    // through the app's own painter: a throwaway one would
                    // start with a cold cache and re-lay the whole transcript
                    // every frame, which is the cost this cache exists to
                    // remove.
                    self.frame =
                        Self::frame_for_model_with(size, scale, &self.model, &mut self.painter);
                    build_scene(&mut scene, &mut self.painter, &self.model, size, scale);
                    if let Err(error) = state.render(&scene) {
                        eprintln!("render error: {error:#}");
                    }
                }
            }
            _ => {}
        }
    }

    /// An animation deadline expired, so the window has to be repainted.
    /// Setting a `WaitUntil` deadline only wakes the loop, it does not draw
    /// anything, which is why the caret used to sit static and never blink.
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            self.request_redraw();
        }
    }

    /// Schedule the next animation tick before the loop sleeps.
    ///
    /// Done here rather than in the redraw handler so the deadline is refreshed
    /// after *any* event, and so an idle window sleeps indefinitely instead of
    /// waking forever.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let flow = match self.animation_deadline(std::time::Instant::now()) {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        };
        event_loop.set_control_flow(flow);
    }
}
