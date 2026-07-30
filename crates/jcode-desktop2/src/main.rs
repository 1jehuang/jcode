//! jcode-desktop2: greenfield desktop app.
//!
//! Milestone 3+4 of docs/HARNESS_API_AND_DESKTOP_REWRITE.md: winit window,
//! Vello vector rendering, Parley text layout, and a live harness API
//! connection (via jcode-harness-api-bridge) with a minimal chat loop.

mod ack;
mod activity;
mod app_harness;
mod app_overview;
mod app_selection;
mod boot;
mod capture;
mod caret;
mod cli;
mod clipboard;
mod donut;
mod editor;
mod edits;
mod frame_meter;
mod harness;
mod hints;
mod input;
mod keymap;
mod layout;
mod mem;
mod meta;
mod overview;
mod paint;
mod place;
mod profile;
mod reasoning;
mod render;
mod scene;
mod scene_overview;
mod scroll;
mod scroll_bench;
mod scroll_profile;
mod select;
mod states;
mod stream;
mod stream_bench;
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
    /// When Super went down with nothing else pressed since, or `None` when
    /// Super is up or has already been used as part of a chord. The field
    /// opens on the keydown; this is what lets a tap shorter than
    /// [`SUPER_TAP`] take it straight back off screen.
    super_held_since: Option<std::time::Instant>,
    /// A Super release that has not been acted on yet, as `(when, was_tap)`.
    ///
    /// Key remappers (keyd's `[meta] h = left`, and every "Super+hjkl becomes
    /// arrows" variant) implement the rewrite by *lifting* Super, sending the
    /// arrow, and pressing Super again. The compositor forwards that faithfully,
    /// so a held Super arrives here as a burst of release/press pairs. Acting on
    /// the release immediately committed and reopened the field on every motion
    /// key, which is the flicker that made held-Super hjkl look broken. Holding
    /// the release for [`SUPER_BOUNCE`] lets a re-press inside that window
    /// cancel it, so a synthetic lift is invisible and a real one still
    /// resolves a frame or two later.
    pending_super_release: Option<(std::time::Instant, bool)>,
    /// Whether holding Super opens the card-strip overview. On by default:
    /// the compositor muscle memory this app lives inside (niri, GNOME) puts
    /// "zoom out to everything" on the Super key. The flag stays so the
    /// gesture can be benched again as a flip rather than a revert.
    super_overview: bool,
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
    /// When the overview's zoom last stepped. Separate from `last_frame`
    /// because the donut's clock stops once there is a transcript.
    overview_frame: Option<std::time::Instant>,
    /// Geometry of the most recently built frame. Pointer hit-testing reads
    /// this instead of the GPU state, so input handling is testable without a
    /// window and can never disagree with what was actually drawn.
    frame: layout::Frame,
    /// Throttled `/proc` reader behind the chrome row's RAM caption. Owned
    /// here rather than by the model so a frame stays a pure function of the
    /// model.
    mem_sampler: mem::Sampler,
    /// Live per-frame timing, off unless `JCODE_DESKTOP2_FRAME_METER` is set.
    /// Lives on the app rather than the render state so the CPU-side build span
    /// can be timed around `build_scene`, which the render state never sees.
    frame_meter: frame_meter::FrameMeter,
}

impl Default for App {
    fn default() -> Self {
        Self {
            state: None,
            painter: paint::Painter::default(),
            model: Model::default(),
            harness: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            super_held_since: None,
            pending_super_release: None,
            super_overview: true,
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
            overview_frame: None,
            // A sensible frame until the first real one is built, so input
            // before the first paint is still handled sanely.
            frame: layout::Frame::new((1100, 720), 1.0),
            mem_sampler: mem::Sampler::default(),
            frame_meter: frame_meter::FrameMeter::from_env(),
        }
    }
}

/// Frame interval while the donut animates (~60fps).
const DONUT_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Maximum gap between two clicks that still counts as a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// How quickly Super must go back up for the press to count as a tap rather
/// than a gesture.
///
/// The field opens the instant Super goes down, because a hold threshold made
/// the most-used shortcut in the app feel like it was thinking. Super is
/// still the first half of editing and navigation chords, so the two escapes
/// are: any chord key pressed with Super aborts the field in the same frame,
/// and letting Super back up this fast dismisses without switching session.
/// Neither costs the user anything they had, and a deliberate hold shows the
/// field immediately.
const SUPER_TAP: std::time::Duration = std::time::Duration::from_millis(180);

/// How long a Super release is held before it is acted on, so a remapper's
/// synthetic lift-and-press around a rewritten key does not read as the user
/// letting go. Long enough to cover the release/press pair (which arrive in the
/// same input batch, microseconds apart) and short enough that a real release
/// still commits within a frame or two.
const SUPER_BOUNCE: std::time::Duration = std::time::Duration::from_millis(90);

/// Frame interval while the overview zooms (~60fps).
const OVERVIEW_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Deadlines closer than this are a continuous animation (the donut, the
/// overview zoom, the streaming reveal, the scroll glide), and those are paced
/// by the display rather than by the timer: the next frame is requested the
/// moment one is drawn, and the compositor's frame callback decides when it
/// paints. A timer cannot do this job. 16ms beats against a 16.7ms refresh, so
/// the wake periodically lands just after the compositor's deadline and a
/// frame is skipped: a visible hitch every few hundred milliseconds in an
/// otherwise smooth glide. Wide enough to catch every per-frame cadence in
/// use, narrow enough to exclude the sparse wakes (the 90ms spinner, the
/// caret's half-second blink), which stay on timers so an idle window sleeps.
const CONTINUOUS_FRAME: std::time::Duration = std::time::Duration::from_millis(20);

/// Frame interval for an *unfocused* window that still has something to say:
/// a turn in flight, a reply streaming in, a scroll glide landing. Those
/// animations carry information (the spinner proves the agent is alive, the
/// reveal is the reply arriving), so freezing them while the user watches
/// from another window reads as a hang. Ten frames a second keeps the window
/// visibly alive at a fraction of the display-rate cost; decorative motion
/// (the caret's blink, the donut) stays asleep until focus returns. Wider
/// than [`CONTINUOUS_FRAME`], so background wakes stay on the timer path
/// rather than chaining display-paced frames.
const BACKGROUND_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// Lines of transcript one notch of a discrete mouse wheel travels.
///
/// A notch is not a line: it is the coarsest input the user has, and every
/// other application on the desktop moves a handful of lines per notch (three
/// is the GTK, Chromium and Firefox default). Moving one line per notch is the
/// single loudest reason wheel scrolling here felt like wading, because it
/// makes a page of transcript cost thirty notches.
pub(crate) const WHEEL_LINES: f64 = 3.0;

/// UI model: what the frame is built from.
pub struct Model {
    pub theme: theme::Theme,
    /// What the user asked for (light, dark, or follow the system). Kept
    /// beside the resolved theme so a system-mode window can re-resolve when
    /// the desktop flips its preference, without forgetting it was "system".
    pub theme_preference: theme::ThemeMode,
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
    /// The most recent failure, until the next turn starts.
    ///
    /// Separate from [`Self::status`] because the status line is suppressed for
    /// an attached session (a healthy connection is noise), and a failure is
    /// the one thing that must survive that rule: losing the connection
    /// mid-session is invisible otherwise.
    pub failure: Option<String>,
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
    /// Scroll smoothing and the scrollbar's fade. The logical position stays
    /// in `scroll`; this is only how the view catches up to it.
    pub smooth: scroll::Smooth,
    /// Live sessions, drawn as the strip at the top of the window.
    pub strip: strip::Strip,
    /// The session overview: the card strip held Super zooms out into. Part of
    /// the model so a frame stays a pure function of it and every phase of
    /// the zoom is capturable.
    pub overview: overview::Overview,
    /// Fetched tails of the other sessions, so the field can show *which*
    /// conversation each card is rather than only its name.
    pub peeks: overview::Peeks,
    /// Working directory of the attached session, as the daemon reports it.
    /// `None` until attach, because a guess here is worse than silence: it is
    /// the fact that decides whether an answer applies to your project.
    pub working_dir: Option<String>,
    /// Provider and model serving this session, once the harness reports it.
    /// `None` until then, so the caption appears rather than showing a guess
    /// that could be wrong.
    pub model: Option<ModelId>,
    /// The boot-up reveal: black paper, the donut growing in, then the rest of
    /// the window. Default is *finished*, so captures and tests see the settled
    /// frame; the real window replaces it on the first paint.
    pub boot: boot::Boot,
    /// Live RAM readout for this window and the daemon, drawn on the trailing
    /// end of the top chrome row. `None` until the first sample, and always
    /// `None` in pinned captures, so frames stay deterministic.
    pub mem: Option<mem::Readout>,
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
            theme_preference: theme::Theme::preference_from_env(),
            meta: meta::Meta::detect(),
            status: "starting...".into(),
            session_id: None,
            transcript: {
                // The user's thinking-display choice is theirs, so it is read
                // once here rather than consulted per delta: a mid-turn config
                // edit must not change what the transcript on screen means.
                let mut transcript = transcript::Transcript::default();
                transcript.set_reasoning_mode(reasoning::ReasoningMode::from_env());
                transcript
            },
            editor: editor::Editor::default(),
            caret: caret::Caret::default(),
            busy: false,
            activity: activity::Activity::default(),
            focused: true,
            scroll: 0.0,
            selection: None,
            notice: None,
            failure: None,
            donut: (!donut_disabled()).then(|| donut::Donut::new(DONUT_GRID)),
            spin: donut::Spin::default(),
            hint: hints::arbitrary_index(),
            stream: stream::Stream::default(),
            smooth: scroll::Smooth::default(),
            strip: strip::Strip::default(),
            overview: overview::Overview::default(),
            peeks: overview::Peeks::default(),
            working_dir: None,
            model: None,
            boot: boot::Boot::default(),
            mem: None,
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
    pub(crate) fn scroll_up(&mut self, amount: f64, max: f64) {
        let before = self.scroll;
        self.scroll = (self.scroll + amount).clamp(0.0, max.max(0.0));
        self.smooth
            .nudge(self.scroll - before, std::time::Instant::now());
    }

    /// Scroll down by `amount` logical pixels; reaching 0 re-follows the tail.
    pub(crate) fn scroll_down(&mut self, amount: f64) {
        let before = self.scroll;
        self.scroll = (self.scroll - amount).max(0.0);
        self.smooth
            .nudge(self.scroll - before, std::time::Instant::now());
    }

    /// Move the view by `delta` logical pixels from a wheel or trackpad
    /// gesture: positive travels toward older content. Unlike a keyboard jump
    /// this lands the pixels immediately and feeds the momentum estimator, so
    /// the transcript tracks the fingers and then coasts like a browser page.
    ///
    /// Returns whether the whole delta fitted; a clamped edge is reported so
    /// the caller can end the fling rather than grind against the boundary.
    pub(crate) fn scroll_gesture(&mut self, delta: f64, max: f64, now: std::time::Instant) -> bool {
        let before = self.scroll;
        self.scroll = (self.scroll + delta).clamp(0.0, max.max(0.0));
        let moved = self.scroll - before;
        self.smooth.glide_from(delta, now);
        (moved - delta).abs() < 0.5
    }

    /// Apply the travel a fling still owes the view. Called once per frame, so
    /// the coast is paced by the display rather than by event arrival.
    /// Whether a fling still owes the view travel, so the caller can skip the
    /// cost of measuring the document on an idle frame.
    pub(crate) fn has_momentum(&self) -> bool {
        self.smooth.has_momentum()
    }

    pub(crate) fn apply_momentum(&mut self, max: f64) {
        let pending = self.smooth.take_momentum();
        if pending == 0.0 {
            return;
        }
        let before = self.scroll;
        self.scroll = (self.scroll + pending).clamp(0.0, max.max(0.0));
        // Coasting into the top or the tail is over: grinding against the
        // clamp would keep the window repainting for no visible movement.
        if ((self.scroll - before) - pending).abs() >= 0.5 {
            self.smooth.stop();
        }
    }

    /// The scroll offset the frame is actually drawn at.
    ///
    /// The logical `scroll` is where the user asked to be; the glide holds the
    /// view above the tail while a reply grows, and the smoothing lag lets a
    /// wheel notch ease in. Everything that has to agree with the pixels on
    /// screen (the renderer, hit-testing, selection) reads this one function,
    /// so a click during an ease lands on the glyph under the pointer.
    pub fn view_scroll(&self) -> f64 {
        self.scroll + self.stream.glide() - self.smooth.lag()
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
        // A failure outranks progress: whatever the app was doing, this is what
        // the user needs to know, and it is shown whether or not a session is
        // attached.
        if let Some(failure) = &self.failure {
            return Some(failure.clone());
        }
        if self.scroll > 0.0 {
            return Some("scrolled back".to_string());
        }
        self.status_footnote()
            .or_else(|| self.meta.alert())
            // Nothing to report: on an empty page say which build this is.
            // The version is the one question a user cannot answer from the
            // window, and an idle transcript is the only place with room for
            // it, so it never competes with an actionable message.
            .or_else(|| {
                self.transcript
                    .is_empty()
                    .then(|| self.meta.version.clone())
            })
    }
}

impl App {
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
        // Mid-turn, the daemon refuses a second message outright ("already
        // processing"), so the message is not sent: it waits at the tail of
        // the transcript in the queued tone, and the turn ending is what
        // sends it (see `flush_queued_message`).
        let queued = self.model.busy;
        self.model.transcript.push(match queued {
            true => transcript::Message::queued(content.clone()),
            false => transcript::Message::sent(content.clone()),
        });
        // Sending clears the last failure: the user has responded to it, and a
        // stale "no network" footnote hanging over a fresh turn would be a
        // report about the past.
        self.model.failure = None;
        // Submitting jumps back to the live tail; otherwise the reply streams
        // in off-screen.
        self.model.scroll = 0.0;
        if queued {
            // The turn is still streaming its reply above the queued card, so
            // the reveal must keep running; resetting it here would replay
            // text the user has already read.
            return;
        }
        // The user's own message was typed, not streamed: revealing it would
        // animate text they are already looking at.
        self.model.stream.reveal_all();
        self.model.busy = true;
        self.model.activity.start(std::time::Instant::now());
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(harness::Command::Send(content));
        }
    }

    /// Send the oldest queued message, now that the turn it waited out is
    /// over. One per turn boundary: the daemon takes one message at a time,
    /// and the rest of the queue waits for the turn this one starts.
    ///
    /// The promotion is what the queued tone and the wiggle hang off: the
    /// card leaves the faint "not sent yet" state, nods, and takes the normal
    /// transcript ink, exactly as if it had been submitted at this moment.
    pub(crate) fn flush_queued_message(&mut self) {
        let Some(content) = self.model.transcript.promote_oldest_queued() else {
            return;
        };
        self.model.busy = true;
        self.model.activity.start(std::time::Instant::now());
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
        let strip = model.strip.len() > 1 || model.working_dir.is_some() || model.mem.is_some();
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
        // The field is modal: a click in it picks a session, and a click on
        // the paper between cards dismisses, like any overview.
        if self.model.overview.is_open() {
            match self.overview_field().hit(x, y) {
                Some(card) => {
                    let id = card.session_id.clone();
                    self.model.overview.set_focus(&id);
                    self.close_overview(true);
                }
                None => self.close_overview(false),
            }
            return;
        }
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
        // Hovering the field moves the highlight, so the mouse and the arrows
        // drive one selection rather than two competing ones. A hover off the
        // cards leaves the highlight where it was: sliding across the gap
        // between two sessions must not deselect the one you were on.
        if self.model.overview.is_open() {
            let (x, y) = self.pointer;
            if let Some(card) = self.overview_field().hit(x, y) {
                let id = card.session_id.clone();
                if self.model.overview.focus() != Some(id.as_str()) {
                    self.model.overview.set_focus(&id);
                    self.request_peek();
                    self.request_redraw();
                }
            }
            return;
        }
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
        // The overview is the one animation that must run whether or not the
        // window thinks it is focused: a compositor that steals focus while
        // Super is held would otherwise freeze the field mid-zoom.
        let overview = self
            .model
            .overview
            .is_animating()
            .then(|| now + OVERVIEW_FRAME);
        // A held Super release waiting out the remapper bounce needs a frame to
        // resolve on, or a gesture ended by letting go with nothing else moving
        // on screen would sit open until the next unrelated event.
        let bounce = self
            .pending_super_release
            .map(|(at, _)| at + crate::SUPER_BOUNCE);
        if !self.model.focused {
            // An unfocused window drops the decorative motion (the caret, the
            // donut) but not the informative kind: the spinner is the proof a
            // turn is still running, and the streaming reveal is the reply
            // itself arriving. Freezing those while the user watches from
            // another window makes progress look like a hang, so they keep
            // ticking at a low background cadence instead of the display rate.
            let spinner = self
                .model
                .activity
                .next_frame_at(now)
                .map(|_| now + BACKGROUND_FRAME);
            let stream = self
                .model
                .stream
                .next_frame_at(now)
                .map(|_| now + BACKGROUND_FRAME);
            // The scroll ease and the bar's fade also finish in the
            // background, so a window left mid-glide does not freeze with the
            // scrollbar half-faded until focus returns.
            let smooth = self
                .model
                .smooth
                .next_frame_at(now)
                .map(|_| now + BACKGROUND_FRAME);
            // The reveal keeps running unfocused: a window opened behind the
            // pointer's focus must not be stuck on a black frame.
            let boot = self.model.boot.next_frame_at(now);
            return [overview, bounce, spinner, stream, smooth, boot]
                .into_iter()
                .flatten()
                .min();
        }
        // The boot reveal outranks everything: it is the first thing on screen
        // and it must not be paced by whatever else happens to be animating.
        let boot = self.model.boot.next_frame_at(now);
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
        // Scroll smoothing and the scrollbar fade both need frames, and both
        // stop themselves once the view has landed and the bar is gone.
        let smooth = self.model.smooth.next_frame_at(now);
        // The delivery wiggle: short, and it stops itself, so an idle window
        // with every message acknowledged still sleeps.
        let ack = self
            .model
            .transcript
            .deliveries()
            .filter_map(|delivery| delivery.next_frame_at(now))
            .min();
        [
            caret, donut, spinner, stream, smooth, overview, bounce, boot, ack,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Whether the frame just drawn should be followed immediately by another:
    /// true while a continuous animation (donut, overview zoom, streaming
    /// reveal, scroll glide) is running. Sparse wakes (the caret's blink, the
    /// 90ms spinner) return false and stay on the `WaitUntil` timer, so an
    /// idle window still sleeps.
    pub fn wants_display_paced_frame(&self, now: std::time::Instant) -> bool {
        self.animation_deadline(now)
            .is_some_and(|at| at <= now + CONTINUOUS_FRAME)
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

    /// Report the conversation's measured height to the stream, so growth at
    /// the tail becomes a glide rather than a jump. Runs against the warm
    /// layout cache, so a steady-state frame pays a lookup, not a relayout.
    fn observe_stream_growth(&mut self) {
        let frame = self.frame;
        let style = crate::scene::transcript_body_style(&self.model);
        let width = (frame.column() - crate::transcript::USER_PAD_X * 2.0).max(1.0);
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
        let content = crate::viewport::Viewport::new(laid, 0.0, 0.0).content_height;
        // Only a view following the tail is glided: a user who scrolled back
        // is holding a position, and moving it would be the app fighting them.
        state.stream.observe_content(content, state.scroll == 0.0);
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

            // The overview owns the keyboard while it is up, so these are the
            // only actions that can reach here from that state.
            Action::OverviewLeft => self.move_overview(overview::Dir::Left),
            Action::OverviewRight => self.move_overview(overview::Dir::Right),
            Action::OverviewUp => self.move_overview(overview::Dir::Up),
            Action::OverviewDown => self.move_overview(overview::Dir::Down),
            Action::OverviewNext => self.cycle_overview(1),
            Action::OverviewPrev => self.cycle_overview(-1),
            Action::OverviewCommit => self.close_overview(true),
            Action::OverviewCancel => self.close_overview(false),

            // A view choice, applied live: the notice is the only feedback the
            // user gets when the mode change has no immediate visible effect
            // (nothing is thinking right now).
            Action::CycleReasoningDisplay => {
                let next = self.model.transcript.reasoning_mode().cycle();
                self.model.transcript.set_reasoning_mode(next);
                self.model.set_notice(format!("thinking: {}", next.label()));
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
            Action::ExtendLineUp => {
                self.model.editor.extend_line(-1);
            }
            Action::ExtendLineDown => {
                self.model.editor.extend_line(1);
            }
            Action::MoveDocStart => self.model.editor.move_to_start(),
            Action::MoveDocEnd => self.model.editor.move_to_end(),
            Action::ExtendDocStart => self.model.editor.extend_to_start(),
            Action::ExtendDocEnd => self.model.editor.extend_to_end(),
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
            Action::Redo => {
                if !self.model.editor.redo() {
                    self.model.set_notice("nothing to redo");
                }
            }
            // Web Ctrl+C: copying is what the user meant whenever there is
            // anything selected. Only a completely unselected Ctrl+C is allowed
            // to interrupt, so the chord can never eat a copy.
            Action::CopyOrInterrupt => {
                let has_selection =
                    self.model.selection.is_some() || self.model.editor.selection().is_some();
                if has_selection {
                    return self.apply(Action::Copy, None);
                }
                return self.apply(Action::InterruptOrQuit, None);
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
            Action::ScrollBottom => {
                let travelled = -self.model.scroll;
                self.model.scroll = 0.0;
                self.model
                    .smooth
                    .nudge(travelled, std::time::Instant::now());
            }

            // Escape never quits: it cancels, then clears, then re-follows the
            // tail, matching the TUI.
            Action::Cancel => {
                // A visible highlight is the most recent thing the user did,
                // so Escape dismisses that first rather than reaching past it
                // to clear typed work.
                if self.model.selection.take().is_some() {
                } else if self.model.busy {
                    self.model.busy = false;
                    // The card shows a call in flight; the user just said
                    // there is none.
                    self.model.transcript.clear_live_tool();
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
                    self.model.transcript.clear_live_tool();
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
        // Start the reveal at the moment the surface exists, not at process
        // start: GPU init takes long enough that timing it from `main` would
        // spend the whole sequence before the first frame is presented.
        self.model.boot = boot::Boot::start(std::time::Instant::now());
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
                // A keystroke ends the reveal at once: an animation that eats
                // the user's first character, or makes them watch it finish, is
                // worse than no animation.
                self.model.boot = boot::Boot::default();
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
            WindowEvent::MouseWheel { delta, phase, .. } => {
                // Scrolling the transcript with the wheel, in logical pixels
                // so a trackpad's fine-grained deltas are not quantised to
                // whole lines.
                //
                // A notch from a discrete wheel is a jump, so it keeps the
                // exponential ease that turns the teleport into a glide. A
                // trackpad's pixel deltas are the finger's own position, so
                // they land immediately and feed the momentum estimator: the
                // page tracks the fingers and then coasts, like a browser.
                //
                // `phase` is what makes the coast trustworthy. Guessing the
                // release from event timing turns every pause in a slow drag
                // into a fling that fights the finger, so the backend's own
                // start/end is taken whenever it offers one.
                match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => {
                        let pixels = f64::from(y) * self.frame.body_line_height() * WHEEL_LINES;
                        if pixels > 0.0 {
                            let max = self.max_scroll();
                            self.model.scroll_up(pixels, max);
                        } else if pixels < 0.0 {
                            self.model.scroll_down(-pixels);
                        }
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        // Only here is the phase meaningful: a discrete notch
                        // arrives with a hard-coded `Moved` that never ends, so
                        // reading it would pin the view in a gesture that never
                        // closes. See `Smooth::phase_known`.
                        let held = matches!(
                            phase,
                            winit::event::TouchPhase::Started | winit::event::TouchPhase::Moved
                        );
                        self.model.smooth.gesture_held(held);
                        let pixels = pos.y / self.frame.scale;
                        if pixels != 0.0 {
                            let max = self.max_scroll();
                            let now = std::time::Instant::now();
                            if !self.model.scroll_gesture(pixels, max, now) {
                                self.model.smooth.stop();
                            }
                        }
                    }
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                self.on_super_changed(self.modifiers.super_key(), std::time::Instant::now());
            }
            // The desktop flipped between light and dark. Only a window in
            // System mode follows: an explicit JCODE_DESKTOP2_THEME choice is
            // the user overriding the desktop, and must keep winning.
            WindowEvent::ThemeChanged(system) => {
                if self.model.theme_preference == theme::ThemeMode::System {
                    let dark = system == winit::window::Theme::Dark;
                    self.model.theme = theme::Theme::for_mode(theme::ThemeMode::System, dark);
                    // The transcript cache keys on the theme, so the switch
                    // relayouts on the next frame without an explicit flush.
                    self.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                self.model.focused = focused;
                if !focused {
                    // The compositor stole the rest of the gesture (niri
                    // grabs most Super chords for itself), so the release
                    // will never arrive here. Take the field down rather
                    // than leaving it stuck open under a window the user
                    // has already left.
                    self.super_held_since = None;
                    self.pending_super_release = None;
                    if self.model.overview.is_visible() {
                        self.model.overview.abort();
                    }
                }
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
                if std::env::var_os("JCODE_DESKTOP2_LOG_INPUT").is_some() {
                    eprintln!(
                        "[input] key {logical_key:?} mods {:?} overview_open {} visible {}",
                        self.modifiers,
                        self.model.overview.is_open(),
                        self.model.overview.is_visible()
                    );
                }
                // While the field is up it owns the keyboard: bound keys
                // drive the field, and anything else is a chord that takes
                // the field straight back off screen.
                if !self.key_pressed(&logical_key, text.as_ref().map(|t| t.as_str())) {
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
                // Refresh the chrome row's RAM caption. Throttled inside the
                // sampler, so per-frame redraws do not become per-frame /proc
                // reads; between samples the previous readout stays shown.
                if let Some(readout) = self.mem_sampler.sample(std::time::Instant::now()) {
                    self.model.mem = Some(readout);
                }
                self.settle_super_release(std::time::Instant::now());
                self.tick_overview(std::time::Instant::now());
                self.animate_donut();
                self.model.boot.advance(std::time::Instant::now());
                self.model.stream.advance(std::time::Instant::now());
                self.model.smooth.advance(std::time::Instant::now());
                // A fling coasts on the frame clock, not on event arrival, so
                // the travel stays smooth after the fingers leave the pad.
                if self.model.has_momentum() {
                    let max = self.max_scroll();
                    self.model.apply_momentum(max);
                }
                let mut scene = Scene::new();
                self.frame_meter.start();
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
                }
                // Feed the conversation's measured height to the stream's
                // glide, so a reply growing at the tail slides the page up
                // instead of jumping it a line at a time. The cache is warm
                // (the frame was just measured through it), so this is a
                // lookup rather than a relayout.
                self.observe_stream_growth();
                if let Some(state) = self.state.as_mut() {
                    let scale = state.scale_factor();
                    let size = state.size();
                    build_scene(&mut scene, &mut self.painter, &self.model, size, scale);
                    self.frame_meter.end_build();
                    if let Err(error) = state.render(&scene, &mut self.frame_meter) {
                        eprintln!("render error: {error:#}");
                    }
                }
                // A continuous animation is paced by the display, not by the
                // timer: ask for the next frame now and let the compositor's
                // frame callback schedule it against the refresh. A timer
                // wake (`WaitUntil` + `request_redraw` in `new_events`) beats
                // against the refresh interval and skips a frame every few
                // hundred milliseconds, which is exactly the intermittent
                // hitch this replaces. Sparse animations (blink, spinner)
                // keep the timer path, so an idle window still sleeps.
                if self.wants_display_paced_frame(std::time::Instant::now()) {
                    self.request_redraw();
                } else {
                    // No continuous frame is coming, so the next interval will
                    // be however long the user waits before doing something.
                    // Reporting that as a frame time would make every idle
                    // window look like it dropped hundreds of frames.
                    self.frame_meter.note_idle();
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
