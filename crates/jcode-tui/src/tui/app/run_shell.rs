use super::idle_animation_repaint::{copy_cells_in, idle_animation_partial_repaint_allowed};
use super::*;

fn report_reload_interaction_gap() {
    let Ok(started) = std::env::var("JCODE_RELOAD_GAP_STARTED_MS") else {
        return;
    };
    crate::env::remove_var("JCODE_RELOAD_GAP_STARTED_MS");
    let Some(started_ms) = started.parse::<u128>().ok() else {
        return;
    };
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return;
    };
    let gap_ms = now.as_millis().saturating_sub(started_ms);
    crate::logging::info(&format!(
        "client_reload_interaction_gap_ms={} milestone=first_frame",
        gap_ms
    ));
}
use crate::tui::TuiState;
use crossterm::cursor::{RestorePosition, SavePosition};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use std::io::Write;

const STATUS_SPINNER_FPS: f32 = 12.5;
pub(super) const STATUS_SPINNER_ONLY_INTERVAL: Duration = Duration::from_millis(80);

pub(super) fn redraw_timer(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    // Redraw ticks represent visual liveness, not elapsed simulation steps. An
    // immediate first tick or Burst catch-up after a slow frame only schedules
    // redundant full renders and can lock the UI into a slow-frame loop.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

pub(super) fn status_spinner_interval() -> tokio::time::Interval {
    status_spinner_interval_after(STATUS_SPINNER_ONLY_INTERVAL)
}

pub(super) fn reset_status_spinner_interval(interval: &mut tokio::time::Interval, app: &App) {
    *interval = status_spinner_interval_after(status_spinner_delay_until_next_frame(
        status_spinner_elapsed(app),
    ));
}

fn status_spinner_interval_after(delay: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + delay,
        STATUS_SPINNER_ONLY_INTERVAL,
    );
    // The spinner is visual liveness, not simulated time. If terminal/input work delays a tick,
    // skip the missed frames instead of bursting them later.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

fn status_spinner_elapsed(app: &App) -> f32 {
    status_spinner_elapsed_for_sources(app.elapsed().map(|duration| duration.as_secs_f32()))
}

fn status_spinner_elapsed_for_sources(turn_elapsed: Option<f32>) -> f32 {
    turn_elapsed.unwrap_or(0.0).max(0.0)
}

fn status_spinner_delay_until_next_frame(elapsed: f32) -> Duration {
    if !elapsed.is_finite() {
        return STATUS_SPINNER_ONLY_INTERVAL;
    }

    let frame_secs = STATUS_SPINNER_ONLY_INTERVAL.as_secs_f64();
    let elapsed_secs = f64::from(elapsed.max(0.0));
    let into_frame = elapsed_secs % frame_secs;
    let remaining = if into_frame <= f64::EPSILON {
        frame_secs
    } else {
        frame_secs - into_frame
    };

    Duration::from_secs_f64(remaining.max(0.001))
}

pub(super) fn status_spinner_only_symbol(app: &App) -> Option<&'static str> {
    let policy = crate::perf::tui_policy();
    // The single-cell spinner fast path is intentionally available even when
    // decorative animations are disabled (Minimal tier, SSH, WSL, etc.). It
    // patches exactly one status cell between full redraws, so it stays very
    // cheap while keeping the "thinking/connecting/streaming" spinner feeling
    // responsive instead of choppy at the ~1 Hz passive-liveness redraw rate.
    // When decorative animations are off it advances at the smooth liveness
    // rate; otherwise it uses the full-rate spinner clock.
    if !app.is_processing
        || !app.streaming.streaming_text.is_empty()
        || app.centered_mode()
        || app.has_pending_mouse_scroll_animation()
        || app.remote_startup_phase_active()
    {
        return None;
    }

    // Slash suggestions are a late overlay and can cover the recorded status
    // row. Do not let the out-of-band one-cell redraw write through them. Check
    // the cheap prefix first so normal spinner ticks never rebuild suggestions.
    if slash_command_palette_may_be_visible(&app.input, || !app.command_suggestions().is_empty()) {
        return None;
    }

    if status_uses_primary_spinner(&app.status) {
        Some(jcode_tui_style::theme::activity_indicator(
            status_spinner_elapsed(app),
            STATUS_SPINNER_FPS,
            policy.enable_decorative_animations,
        ))
    } else {
        None
    }
}

fn is_slash_command_input(input: &str) -> bool {
    input.trim_start().starts_with('/')
}

/// Whether the floating command-suggestion palette may be on screen for the
/// current composer contents.
///
/// The palette is a late overlay (`draw_command_suggestions_overlay`) that
/// floats over the rows directly below the composer without reserving layout
/// height. Partial-repaint fast paths reuse or patch cells from the previous
/// full frame, so they must stand down while the palette is (possibly) up:
/// the one-cell spinner would write through it, and the animation-only repaint
/// resets and redraws the exact rows it floats over on a fresh idle screen.
///
/// `has_suggestions` is lazy so hot paths can check the cheap input prefix
/// first and skip building the suggestion list entirely.
pub(crate) fn slash_command_palette_may_be_visible(
    input: &str,
    has_suggestions: impl FnOnce() -> bool,
) -> bool {
    is_slash_command_input(input) && has_suggestions()
}

/// Statuses whose full status line starts with the primary green circular spinner.
///
/// Keep this in sync with `ui_input::draw_status`: these statuses can be safely
/// refreshed by the one-cell spinner fast path when the status line is left aligned.
/// Tool execution uses its own full-line activity indicator, and network waits use
/// a static amber retry marker, so neither belongs here.
pub(crate) fn status_uses_primary_spinner(status: &ProcessingStatus) -> bool {
    matches!(
        status,
        ProcessingStatus::Sending
            | ProcessingStatus::Connecting(_)
            | ProcessingStatus::Thinking(_)
            | ProcessingStatus::Streaming
    )
}

/// How the next full frame should invalidate ratatui's diff state, if at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullFrameInvalidation {
    /// `Terminal::clear()`: an ED2 Clear-All escape plus a full re-emit.
    /// Needed when the real screen diverged from ratatui's model in cells the
    /// next diff may not repaint (native terminal scroll, external commands).
    HardClear,
    /// Sentinel-invalidate the previous buffer: full re-emit with no
    /// intermediate clear escape, so the repaint stays atomic inside the
    /// synchronized update. Used for scroll-driven repaints (issue #404).
    SoftRepaint,
    /// Normal incremental diff.
    None,
}

/// Pure routing for `draw_full`: a hard clear supersedes a soft repaint.
pub(crate) fn full_frame_invalidation(
    force_full_redraw: bool,
    force_full_repaint: bool,
) -> FullFrameInvalidation {
    if force_full_redraw {
        FullFrameInvalidation::HardClear
    } else if force_full_repaint {
        FullFrameInvalidation::SoftRepaint
    } else {
        FullFrameInvalidation::None
    }
}

/// A cell no real frame produces: a Unicode noncharacter symbol with an
/// improbable style, so a diff against it sees every cell as changed.
fn full_repaint_sentinel_cell() -> ratatui::buffer::Cell {
    let mut cell = ratatui::buffer::Cell::EMPTY;
    cell.set_symbol("\u{FDD0}");
    cell.fg = ratatui::style::Color::Rgb(1, 2, 3);
    cell.bg = ratatui::style::Color::Rgb(3, 2, 1);
    cell
}

/// Fill ratatui's "previous" buffer with sentinel cells so the next
/// `Terminal::draw` diff re-emits every cell.
///
/// This is the flicker-free alternative to `Terminal::clear()` for repaints
/// that need full cell coverage (ratatui #2357 wide-grapheme ghosts on
/// scroll) but not a real screen wipe: `Terminal::clear()` emits an ED2
/// Clear-All escape before the frame is redrawn, and terminals that paint
/// image placeholder cells non-atomically flash blank during the
/// clear-then-repaint on every scroll tick (issue #404). Overwriting every
/// cell in place inside the surrounding synchronized update repaints
/// atomically instead.
pub(crate) fn invalidate_previous_terminal_buffer<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
) {
    // `swap_buffers` resets the inactive buffer and flips. Two swaps with a
    // sentinel fill in between leave: previous = all-sentinel, current = reset
    // and ready for the next `draw`.
    terminal.swap_buffers();
    let sentinel = full_repaint_sentinel_cell();
    for cell in terminal.current_buffer_mut().content.iter_mut() {
        *cell = sentinel.clone();
    }
    terminal.swap_buffers();
}

/// State the animation-only fast path consults before it may run, snapshotted
/// so the decision itself is a pure function (see
/// [`idle_animation_fast_path_blocked_reason`]).
struct IdleAnimationFastPathInputs {
    has_previous_frame: bool,
    animation_active: bool,
    has_animation_area: bool,
    force_full_redraw: bool,
    force_full_repaint: bool,
    composer_changed: bool,
    command_palette_visible: bool,
}

/// Why the animation-only partial repaint must not run, or `None` when it may.
///
/// Pure so the precedence and each individual guard are directly testable;
/// `idle_animation_only_available` feeds it live state and reports the reason
/// to `draw-stats`.
fn idle_animation_fast_path_blocked_reason(
    inputs: &IdleAnimationFastPathInputs,
) -> Option<&'static str> {
    if !inputs.has_previous_frame {
        Some("no_previous_frame")
    } else if !inputs.animation_active {
        Some("animation_inactive")
    } else if !inputs.has_animation_area {
        Some("no_animation_area")
    } else if inputs.force_full_redraw {
        Some("force_full_redraw")
    } else if inputs.force_full_repaint {
        Some("force_full_repaint")
    } else if inputs.composer_changed {
        // The user typed (or edited) since the last full frame. This path
        // reuses that frame outside the animation rows, so it physically
        // cannot show the new input line; taking it would consume the redraw
        // the keystroke requested and defer the glyph to a later full frame.
        Some("input_changed")
    } else if inputs.command_palette_visible {
        // The command palette floats over the rows directly below the
        // composer, which on a fresh idle screen are the animation rows.
        // This path resets and redraws exactly those rows, so it would erase
        // the palette milliseconds after every chrome full frame painted it:
        // pressing `/` in a fresh session made the menu blink in and out at
        // the chrome full-frame cadence. Stand down until the palette closes.
        Some("command_palette_visible")
    } else {
        None
    }
}

#[derive(Default)]
pub(super) struct StatusSpinnerRenderer {
    last_frame: Option<Buffer>,
    last_full_frame_at: Option<Instant>,
    /// Animated rectangle whose surrounding cells are already seeded into
    /// ratatui's working buffer.
    ///
    /// The animation-only repaint used to `clone_from` the whole previous frame
    /// every tick just to re-render one rectangle over it: ~920k cell copies a
    /// second at 60fps on a 160x48 terminal, to update ~2200 cells. Once the
    /// working buffer has been seeded for a given rectangle, everything outside it
    /// is already correct (this path is the only writer between full frames), so
    /// later ticks copy just those rows. Cleared by [`Self::invalidate`] and
    /// re-seeded whenever the rectangle changes.
    seeded_animation_area: Option<Rect>,
    /// Composer contents as of the last full frame.
    ///
    /// The animation-only path reuses that frame for everything except the
    /// decorative rows, so it cannot show a newer input line. Without this it
    /// would "satisfy" the redraw a keystroke asked for while omitting the
    /// character, which then waited for a later full frame. Measured against a
    /// live client: typing echoed to the terminal in ~7ms but only reached the
    /// screen ~500ms later; with the guard the same keystroke paints in ~6ms.
    last_full_frame_input: String,
}

impl StatusSpinnerRenderer {
    pub(super) fn spinner_only_available(&self, app: &App) -> bool {
        status_spinner_only_symbol(app).is_some()
    }

    pub(super) fn invalidate(&mut self) {
        self.last_frame = None;
        self.last_full_frame_at = None;
        // Nothing is known to be seeded any more, so the next animation-only
        // repaint must do one full seed before it may copy just the animated
        // rows again.
        self.seeded_animation_area = None;
        self.last_full_frame_input.clear();
    }

    /// Whether the decorative idle-animation rows can be repainted on their own,
    /// reusing the rest of the previous frame.
    ///
    /// Available only when the previous full frame actually drew the animation
    /// and nothing else in the app needs a repaint this tick. On an idle screen
    /// the animation is the sole moving element, so a full render at animation
    /// FPS re-derives the transcript, header, status, and composer into
    /// byte-identical cells. That was measured at a ~50ms median per tick on an
    /// 8-core laptop, which is what made the animation visibly lag.
    /// Whether the composer differs from what the last full frame drew.
    ///
    /// The animation-only repaint reuses that frame everywhere except the
    /// decorative rows, so it cannot display a newer input line.
    fn composer_changed_since_last_full_frame(&self, input: &str) -> bool {
        input != self.last_full_frame_input
    }

    pub(super) fn idle_animation_only_available(&self, app: &App) -> bool {
        let blocked = idle_animation_fast_path_blocked_reason(&IdleAnimationFastPathInputs {
            has_previous_frame: self.last_frame.is_some(),
            animation_active: crate::tui::idle_donut_active(app),
            has_animation_area: crate::tui::ui::last_idle_animation_area().is_some(),
            force_full_redraw: app.force_full_redraw,
            force_full_repaint: app.force_full_repaint,
            composer_changed: self.composer_changed_since_last_full_frame(&app.input),
            command_palette_visible: slash_command_palette_may_be_visible(&app.input, || {
                !app.command_suggestions().is_empty()
            }),
        });
        if let Some(reason) = blocked {
            crate::tui::ui::note_idle_animation_fast_path_blocked(reason);
            return false;
        }

        let allowed = idle_animation_partial_repaint_allowed(
            crate::tui::periodic_redraw_required_excluding_idle_animation(app),
            self.last_full_frame_at.map(|at| at.elapsed()),
        );
        if !allowed {
            crate::tui::ui::note_idle_animation_fast_path_blocked("chrome_full_frame_due");
        }
        allowed
    }

    /// Repaint just the idle-animation rows over the previous frame.
    ///
    /// Returns `false` when the fast path cannot be used, in which case the
    /// caller must fall back to a full redraw.
    ///
    /// "Cheap" here is relative to a full render, but this used to be far more
    /// expensive than it needed to be: it cloned the entire screen buffer twice
    /// per frame (`clone_from` to seed the working buffer, then `clone` to keep a
    /// copy) even though only `area` changes. At 60fps on a 160x48 terminal that
    /// is ~920k cell copies a second to update ~2200 cells. Measured on a real
    /// session, the animation cost 0.257 CPU cores over an idle client and nearly
    /// doubled keystroke latency (p50 6.6ms vs 3.5ms). Now only the animated rows
    /// are touched, in both buffers.
    pub(super) fn draw_idle_animation_only(
        &mut self,
        app: &App,
        terminal: &mut DefaultTerminal,
    ) -> Result<bool> {
        let Some(previous_frame) = self.last_frame.as_ref() else {
            return Ok(false);
        };
        let Some(area) = crate::tui::ui::last_idle_animation_area() else {
            return Ok(false);
        };
        // The terminal may have been resized since that frame was captured.
        if !previous_frame.area.contains((area.x, area.y).into())
            || area.right() > previous_frame.area.right()
            || area.bottom() > previous_frame.area.bottom()
        {
            return Ok(false);
        }

        {
            let current_buffer = terminal.current_buffer_mut();
            if current_buffer.area != previous_frame.area {
                return Ok(false);
            }
            // Seed the working buffer from the last known frame. Only the
            // animated rows can differ once this path owns the screen (it is the
            // only writer between full frames), so after the first pass we copy
            // just those rows instead of the whole screen. `seeded_animation_area`
            // records which rows that invariant covers, and any change to it (or
            // to the frame identity) forces one full seed again.
            if self.seeded_animation_area == Some(area) {
                copy_cells_in(previous_frame, current_buffer, area);
            } else {
                current_buffer.clone_from(previous_frame);
                self.seeded_animation_area = Some(area);
            }
            crate::tui::ui::render_idle_animation_into(
                current_buffer,
                area,
                crate::tui::TuiState::animation_elapsed(app),
            );
        }

        // Same protocol as the one-cell spinner fast path: keep ratatui's
        // virtual buffers authoritative, flush the diff inside a synchronized
        // update, and preserve the user's cursor position.
        crossterm::queue!(
            terminal.backend_mut(),
            BeginSynchronizedUpdate,
            SavePosition
        )?;
        terminal.flush()?;
        crossterm::queue!(
            terminal.backend_mut(),
            RestorePosition,
            EndSynchronizedUpdate
        )?;
        terminal.swap_buffers();
        terminal.backend_mut().flush()?;
        // Keep the remembered frame current without cloning the whole screen: it
        // already matches everywhere except the rows just re-rendered. Re-render
        // the animation into those rows directly (deterministic for a given
        // elapsed time, and far cheaper than copying 7680 cells) so
        // `self.last_frame` stays byte-identical to what the terminal now shows.
        if let Some(last) = self.last_frame.as_mut() {
            crate::tui::ui::render_idle_animation_into(
                last,
                area,
                crate::tui::TuiState::animation_elapsed(app),
            );
        }
        // Without this the animation-only path was invisible in `draw-stats`:
        // `partial_repaints` stayed at 0 while this path served ~60 repaints a
        // second, which reads as "the cheap path never runs" and sends anyone
        // debugging redraw cost down the wrong branch.
        crate::tui::ui::note_idle_animation_partial_repaint();
        Ok(true)
    }

    pub(super) fn draw_full(
        &mut self,
        app: &mut App,
        terminal: &mut DefaultTerminal,
    ) -> Result<()> {
        // Painting a frame is progress, including during long streaming turns.
        crate::logging::watchdog::beat("tui.draw");
        let invalidation = full_frame_invalidation(app.force_full_redraw, app.force_full_repaint);
        let force_full_redraw = invalidation != FullFrameInvalidation::None;
        // Wrap the whole frame (optional clear + diff flush) in a synchronized update so the
        // terminal applies every cell change atomically. Without this, ratatui's crossterm
        // backend streams cells one-by-one and eagerly-repainting terminals (and slow/remote or
        // multiplexed sessions) show visible flicker. See issue #282.
        let sync = crossterm::execute!(terminal.backend_mut(), BeginSynchronizedUpdate).is_ok();
        match invalidation {
            FullFrameInvalidation::HardClear => {
                terminal.clear()?;
                self.invalidate();
            }
            FullFrameInvalidation::SoftRepaint => {
                invalidate_previous_terminal_buffer(terminal);
                self.invalidate();
            }
            FullFrameInvalidation::None => {}
        }
        app.force_full_redraw = false;
        app.force_full_repaint = false;

        let previous_frame = self.last_frame.as_ref();
        let draw_start = Instant::now();
        let mut render_elapsed = Duration::ZERO;
        let completed = terminal.draw(|frame| {
            let render_start = Instant::now();
            crate::tui::ui::draw(frame, app);
            render_elapsed = render_start.elapsed();
        })?;
        let total_elapsed = draw_start.elapsed();
        let changed_cells = previous_frame
            .filter(|previous| previous.area == completed.buffer.area)
            .map(|previous| {
                previous
                    .content
                    .iter()
                    .zip(completed.buffer.content.iter())
                    .filter(|(left, right)| left != right)
                    .count()
            });
        let total_cells = Some(completed.buffer.content.len());
        let completed_buffer = completed.buffer.clone();
        // `completed` borrows the terminal; it is unused past this point, so the
        // borrow ends here (NLL) before we touch the backend again below.
        if sync {
            let _ = crossterm::execute!(terminal.backend_mut(), EndSynchronizedUpdate);
        }
        crate::tui::ui::record_draw_call_attribution(crate::tui::ui::DrawCallAttribution {
            timestamp_ms: crate::tui::ui::wall_clock_ms(),
            total_ms: total_elapsed.as_secs_f64() * 1000.0,
            render_ms: render_elapsed.as_secs_f64() * 1000.0,
            backend_flush_ms: total_elapsed.saturating_sub(render_elapsed).as_secs_f64() * 1000.0,
            changed_cells,
            total_cells,
            force_full_redraw,
            input: crate::tui::ui::frame_input_attribution_snapshot(),
        });
        if crate::tui::ui::last_idle_animation_area().is_some() {
            crate::tui::ui::note_idle_animation_full_repaint();
        }
        self.last_frame = Some(completed_buffer);
        self.last_full_frame_at = Some(Instant::now());
        // A full frame rewrote the whole surface, so ratatui's working buffer no
        // longer matches `last_frame` outside the animated rows. Force the next
        // animation-only repaint to re-seed before it trusts that again.
        self.seeded_animation_area = None;
        // This frame drew the composer as it is now, so the animation-only path
        // may reuse it again until the input changes.
        if self.last_full_frame_input != app.input {
            self.last_full_frame_input.clear();
            self.last_full_frame_input.push_str(&app.input);
        }
        // Close the key-to-paint clock here rather than at render time: the user
        // sees the keystroke when the frame reaches the terminal, so anything
        // before the flush would understate the latency they feel.
        crate::tui::ui::note_frame_painted();
        Ok(())
    }

    pub(super) fn draw_status_spinner_only(
        &mut self,
        app: &App,
        terminal: &mut DefaultTerminal,
    ) -> Result<bool> {
        let status_symbol = status_spinner_only_symbol(app);
        if status_symbol.is_none() {
            return Ok(false);
        }
        let Some(previous_frame) = self.last_frame.as_ref() else {
            return Ok(false);
        };
        let status_area = crate::tui::ui::last_status_area();
        let status_patchable = status_symbol
            .zip(status_area)
            .is_some_and(|(symbol, area)| {
                render_status_spinner_into_buffer(previous_frame, area, symbol)
            });
        if !status_patchable {
            return Ok(false);
        }

        let next_frame = {
            let current_buffer = terminal.current_buffer_mut();
            current_buffer.clone_from(previous_frame);
            if let Some((symbol, area)) = status_symbol.zip(status_area)
                && status_patchable
            {
                render_status_spinner_into_buffer_mut(current_buffer, area, symbol);
            }
            current_buffer.clone()
        };

        // Keep ratatui's virtual buffers authoritative while preserving the user's cursor position.
        // The only terminal mutation outside ratatui here is cursor save/restore; cell contents still
        // go through Terminal::flush so the next full-frame diff remains synchronized. Wrap the
        // single-cell update in a synchronized update so it applies atomically (see issue #282).
        crossterm::queue!(
            terminal.backend_mut(),
            BeginSynchronizedUpdate,
            SavePosition
        )?;
        terminal.flush()?;
        crossterm::queue!(
            terminal.backend_mut(),
            RestorePosition,
            EndSynchronizedUpdate
        )?;
        terminal.swap_buffers();
        terminal.backend_mut().flush()?;
        self.last_frame = Some(next_frame);
        // This path `clone_from`s the whole previous frame and patches one cell,
        // so the animation-only repaint's "everything outside the animated rows
        // is already seeded" assumption no longer holds. Make it re-seed once.
        self.seeded_animation_area = None;
        crate::tui::ui::note_idle_animation_partial_repaint();
        Ok(true)
    }
}

fn render_status_spinner_into_buffer(buffer: &Buffer, area: Rect, symbol: &str) -> bool {
    area.width > 0
        && area.height > 0
        && buffer
            .cell((area.x, area.y))
            .is_some_and(|cell| jcode_tui_style::theme::is_activity_indicator_frame(cell.symbol()))
        && !symbol.is_empty()
}

fn render_status_spinner_into_buffer_mut(buffer: &mut Buffer, area: Rect, symbol: &str) {
    buffer.set_stringn(
        area.x,
        area.y,
        symbol,
        1,
        // The spinner cell is patched outside the full-frame draw, so apply
        // light-theme adaptation here explicitly (no-op on dark themes).
        Style::default().fg(jcode_tui_style::adapt_color_for_theme(
            jcode_tui_style::theme::ai_color(),
        )),
    );
}

impl App {
    /// Run the TUI application
    /// Returns Some(session_id) if hot-reload was requested
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<RunResult> {
        super::terminal_liveness::capture_initial_tty();
        let mut event_stream = EventStream::new();
        let mut redraw_period = crate::tui::redraw_interval(&self);
        let mut redraw_interval = redraw_timer(redraw_period);
        let mut status_spinner_interval = status_spinner_interval();
        let mut status_spinner_renderer = StatusSpinnerRenderer::default();
        let mut needs_redraw = true;
        let mut first_frame_reported = false;
        let mut handterm_native_scroll =
            super::handterm_native_scroll::HandtermNativeScrollClient::connect_from_env();
        // Subscribe to bus for background task completion notifications
        let mut bus_receiver = Bus::global().subscribe();
        if let Some(status) = Bus::global().latest_update_status() {
            self.handle_update_status(status);
        }

        loop {
            self.sync_sleep_guard();
            let desired_redraw = crate::tui::redraw_interval(&self);
            if desired_redraw != redraw_period {
                redraw_period = desired_redraw;
                redraw_interval = redraw_timer(redraw_period);
            }

            if needs_redraw {
                // On an idle animated screen, repaint just the animation rows
                // when nothing else is due. This is the single draw site, so
                // gating here covers every redraw source (ticks, input, bus
                // events), not just the animation tick.
                if status_spinner_renderer.idle_animation_only_available(&self)
                    && status_spinner_renderer.draw_idle_animation_only(&self, &mut terminal)?
                {
                    needs_redraw = false;
                } else {
                    status_spinner_renderer.draw_full(&mut self, &mut terminal)?;
                    if !first_frame_reported {
                        first_frame_reported = true;
                        report_reload_interaction_gap();
                    }
                    reset_status_spinner_interval(&mut status_spinner_interval, &self);
                    if let Some(native) = handterm_native_scroll.as_mut() {
                        native.sync_from_app(&self);
                    }
                    needs_redraw = false;
                }
            }

            if self.should_quit {
                break;
            }

            // Process pending turn OR wait for input/redraw
            if self.pending_turn {
                self.pending_turn = false;
                // Process turn while still handling input
                self.process_turn_with_input(&mut terminal, &mut event_stream, &mut bus_receiver)
                    .await;
                needs_redraw = true;
            } else if self.pending_queued_dispatch {
                self.pending_queued_dispatch = false;
                self.process_queued_messages(&mut terminal, &mut event_stream)
                    .await;
                local::finish_turn(&mut self);
                needs_redraw = true;
            } else {
                // Wait for input or redraw tick
                tokio::select! {
                    // Declaration-order polling: user input outranks timers and
                    // bus chatter (see the remote loop for the rationale).
                    biased;
                    event = event_stream.next() => {
                        if event.is_some() {
                            needs_redraw |= local::handle_terminal_event(&mut self, &mut terminal, event)?;
                        } else if super::terminal_liveness::terminal_abandoned() {
                            // Input EOF and the controlling terminal is gone:
                            // this client is an orphan (window died without a
                            // deliverable SIGHUP). Exit instead of looping
                            // forever holding ~100 MB. The session persists
                            // and can be resumed.
                            crate::logging::warn(
                                "Terminal input closed and controlling terminal is gone; exiting orphaned client",
                            );
                            self.should_quit = true;
                        } else {
                            tokio::time::sleep(redraw_period).await;
                        }
                    }
                    _ = status_spinner_interval.tick(), if status_spinner_renderer.spinner_only_available(&self) => {
                        if !status_spinner_renderer.draw_status_spinner_only(&self, &mut terminal)? {
                            needs_redraw = true;
                        }
                    }
                    _ = redraw_interval.tick() => {
                        needs_redraw |= local::handle_tick(&mut self);
                    }
                    command = async {
                        match handterm_native_scroll.as_mut() {
                            Some(native) => native.recv().await,
                            None => futures::future::pending::<Option<super::handterm_native_scroll::HostToApp>>().await,
                        }
                    } => {
                        if let Some(command) = command {
                            self.apply_handterm_native_scroll(command);
                            self.request_full_redraw();
                            needs_redraw = true;
                        } else {
                            handterm_native_scroll = None;
                        }
                    }
                    // Handle background task completion notifications
                    bus_event = bus_receiver.recv() => {
                        needs_redraw |= local::handle_bus_event(&mut self, bus_event);
                    }
                }
            }
        }

        self.extract_session_memories().await;

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            restart_session: self.restart_requested.take(),
            exit_code: self.requested_exit_code,
            session_id: Some(self.session.id.clone()),
        })
    }

    /// Run the TUI in remote mode, connecting to a server
    pub async fn run_remote(
        mut self,
        mut terminal: DefaultTerminal,
        remote_working_dir: Option<String>,
    ) -> Result<RunResult> {
        super::terminal_liveness::capture_initial_tty();
        let mut event_stream = EventStream::new();
        let mut redraw_period = crate::tui::redraw_interval(&self);
        let mut redraw_interval = redraw_timer(redraw_period);
        let mut status_spinner_interval = status_spinner_interval();
        let mut status_spinner_renderer = StatusSpinnerRenderer::default();
        let mut needs_redraw = true;
        let mut first_frame_reported = false;
        // While unfocused and idle, redraws are throttled to this interval so a
        // backgrounded session does not repaint at full rate on shared-server bus
        // chatter. `None` means "no throttled frame drawn yet since losing focus".
        const UNFOCUSED_IDLE_REDRAW_MIN_INTERVAL: std::time::Duration =
            std::time::Duration::from_millis(1000);
        let mut last_unfocused_draw: Option<std::time::Instant> = None;
        let mut handterm_native_scroll =
            super::handterm_native_scroll::HandtermNativeScrollClient::connect_from_env();
        let mut remote_state = remote::RemoteRunState::default();

        'outer: loop {
            if self.display_messages.is_empty() {
                if self.server_spawning {
                    self.set_remote_startup_phase(super::RemoteStartupPhase::StartingServer);
                } else {
                    self.set_remote_startup_phase(super::RemoteStartupPhase::Connecting);
                }
            }
            if needs_redraw {
                status_spinner_renderer.draw_full(&mut self, &mut terminal)?;
                if !first_frame_reported {
                    first_frame_reported = true;
                    report_reload_interaction_gap();
                }
                // Close the startup-profile gap: `pre_run_remote` is the last
                // pre-loop mark, so the first completed paint here is the real
                // process-to-first-frame point. Logged once via a static guard so
                // the end-to-end launch cost (including the ~5ms first draw) is
                // visible in the startup profile without re-marking every frame.
                {
                    use std::sync::atomic::{AtomicBool, Ordering};
                    static FIRST_FRAME_MARKED: AtomicBool = AtomicBool::new(false);
                    if !FIRST_FRAME_MARKED.swap(true, Ordering::Relaxed) {
                        crate::startup_profile::mark("first_frame");
                        crate::startup_profile::report_to_log();
                    }
                }
                reset_status_spinner_interval(&mut status_spinner_interval, &self);
                needs_redraw = false;
            }

            let session_to_resume = self.reconnect_target_session_id();

            let mut remote_conn = match remote::connect_with_retry(
                &mut self,
                &mut terminal,
                &mut event_stream,
                &mut remote_state,
                session_to_resume.as_deref(),
                remote_working_dir.as_deref(),
            )
            .await?
            {
                remote::ConnectOutcome::Connected(remote) => remote,
                remote::ConnectOutcome::Retry => continue,
                remote::ConnectOutcome::Quit => break 'outer,
            };
            status_spinner_renderer.invalidate();

            match remote::handle_post_connect(
                &mut self,
                &mut terminal,
                &mut remote_conn,
                &mut remote_state,
                session_to_resume.as_deref(),
            )
            .await?
            {
                remote::PostConnectOutcome::Ready => {}
                remote::PostConnectOutcome::Quit => break 'outer,
            }
            status_spinner_renderer.invalidate();
            needs_redraw = true;

            let mut bus_receiver_remote = Bus::global().subscribe();
            if let Some(status) = Bus::global().latest_update_status() {
                self.handle_update_status(status);
                needs_redraw = true;
            }

            // Main event loop
            loop {
                self.sync_sleep_guard();
                let desired_redraw = crate::tui::redraw_interval(&self);
                if desired_redraw != redraw_period {
                    redraw_period = desired_redraw;
                    redraw_interval = redraw_timer(redraw_period);
                }

                if needs_redraw {
                    // Throttle idle full-frame renders while the terminal is
                    // backgrounded (FocusLost). An unfocused, idle session has
                    // nothing changing worth a 60fps repaint, so it should not
                    // repaint at full rate just because other sessions on the
                    // shared server broadcast bus updates -- that is what made a
                    // swarm of background windows saturate the CPU. We keep full-
                    // rate redraws while streaming/processing so visible-but-
                    // unfocused windows in a tiling WM still show live progress,
                    // and set_client_focused(true) forces a full repaint on refocus.
                    let allow_redraw = self.client_focused()
                        || self.unfocused_redraw_warranted()
                        || last_unfocused_draw
                            .map(|t| t.elapsed() >= UNFOCUSED_IDLE_REDRAW_MIN_INTERVAL)
                            .unwrap_or(true);
                    if allow_redraw {
                        // Idle animated screen: repaint only the animation rows
                        // when nothing else is due (see the local loop).
                        if status_spinner_renderer.idle_animation_only_available(&self)
                            && status_spinner_renderer
                                .draw_idle_animation_only(&self, &mut terminal)?
                        {
                            needs_redraw = false;
                        } else {
                            status_spinner_renderer.draw_full(&mut self, &mut terminal)?;
                            reset_status_spinner_interval(&mut status_spinner_interval, &self);
                            if let Some(native) = handterm_native_scroll.as_mut() {
                                native.sync_from_app(&self);
                            }
                            last_unfocused_draw =
                                (!self.client_focused()).then(std::time::Instant::now);
                            needs_redraw = false;
                        }
                    }
                    // When unfocused and throttled, leave needs_redraw set so the
                    // pending update is coalesced into the next allowed frame.
                }

                if self.should_quit {
                    break 'outer;
                }

                if self.pending_queued_dispatch {
                    self.pending_queued_dispatch = false;
                    remote::process_remote_followups(&mut self, &mut remote_conn).await;
                    needs_redraw = true;
                    continue;
                }

                tokio::select! {
                    // Poll in declaration order so user input always wins the
                    // race against server/bus chatter. During heavy streaming
                    // the remote event branch is almost always ready; with the
                    // default random polling it repeatedly outcompetes buffered
                    // keystrokes, which shows up as a laggy, stuttering input
                    // line while a turn is running.
                    biased;
                    event = event_stream.next() => {
                        if event.is_some() {
                            needs_redraw |= remote::handle_terminal_event(&mut self, &mut terminal, &mut remote_conn, event).await?;
                        } else if super::terminal_liveness::terminal_abandoned() {
                            // Input EOF with the controlling terminal gone:
                            // orphaned client (see local loop). Exit; the
                            // server-side session keeps running and can be
                            // reattached with --resume.
                            crate::logging::warn(
                                "Terminal input closed and controlling terminal is gone; exiting orphaned client",
                            );
                            self.should_quit = true;
                        } else {
                            tokio::time::sleep(redraw_period).await;
                        }
                    }
                    _ = status_spinner_interval.tick(), if status_spinner_renderer.spinner_only_available(&self) => {
                        if !status_spinner_renderer.draw_status_spinner_only(&self, &mut terminal)? {
                            needs_redraw = true;
                        }
                    }
                    _ = redraw_interval.tick() => {
                        needs_redraw |= remote::handle_tick(&mut self, &mut remote_conn).await;
                    }
                    event = remote_conn.next_event() => {
                        let (outcome, event_redraw) = remote::handle_remote_event(
                            &mut self,
                            &mut terminal,
                            &mut remote_conn,
                            &mut remote_state,
                            event,
                        )
                        .await?;
                        needs_redraw |= event_redraw;
                        match outcome {
                            remote::RemoteEventOutcome::Continue => {}
                            remote::RemoteEventOutcome::Reconnect => continue 'outer,
                            remote::RemoteEventOutcome::Quit => break 'outer,
                        }
                    }
                    command = async {
                        match handterm_native_scroll.as_mut() {
                            Some(native) => native.recv().await,
                            None => futures::future::pending::<Option<super::handterm_native_scroll::HostToApp>>().await,
                        }
                    } => {
                        if let Some(command) = command {
                            self.apply_handterm_native_scroll(command);
                            self.request_full_redraw();
                            needs_redraw = true;
                        } else {
                            handterm_native_scroll = None;
                        }
                    }
                    bus_event = bus_receiver_remote.recv() => {
                        needs_redraw |= remote::handle_bus_event(&mut self, &mut remote_conn, bus_event).await;
                    }
                }
            }
        }

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            restart_session: self.restart_requested.take(),
            exit_code: self.requested_exit_code,
            session_id: if self.is_remote {
                self.remote_session_id.clone()
            } else {
                Some(self.session.id.clone())
            },
        })
    }

    /// Run the TUI in replay mode, playing back a timeline of events.
    pub async fn run_replay(
        self,
        terminal: DefaultTerminal,
        timeline: Vec<crate::replay::TimelineEvent>,
        speed: f64,
    ) -> Result<RunResult> {
        replay::run_replay(self, terminal, timeline, speed).await
    }

    /// Run an interactive swarm replay, rendering multiple sessions in tiled panes.
    pub async fn run_swarm_replay(
        terminal: DefaultTerminal,
        panes: Vec<crate::replay::PaneReplayInput>,
        speed: f64,
        centered_override: Option<bool>,
    ) -> Result<()> {
        replay::run_swarm_replay(terminal, panes, speed, centered_override).await
    }

    /// Run replay headlessly, rendering each frame to an in-memory buffer.
    /// Returns a list of (timestamp_secs, Buffer) pairs for video export.
    pub async fn run_headless_replay(
        mut self,
        timeline: &[crate::replay::TimelineEvent],
        speed: f64,
        width: u16,
        height: u16,
        fps: u32,
    ) -> Result<Vec<(f64, ratatui::buffer::Buffer)>> {
        use crate::replay::ReplayEvent;
        use ratatui::backend::TestBackend;

        let replay_events = crate::replay::timeline_to_replay_events(timeline);
        if replay_events.is_empty() {
            anyhow::bail!("No replay events to export");
        }

        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let mut remote = crate::tui::backend::ReplayRemoteState::default();

        let frame_duration_ms: f64 = 1000.0 / fps as f64;
        let mut frames: Vec<(f64, ratatui::buffer::Buffer)> = Vec::new();
        let mut sim_time_ms: f64 = 0.0;
        let mut next_frame_at: f64 = 0.0;

        let total_duration_ms: f64 = replay_events.iter().map(|(d, _)| *d as f64 / speed).sum();

        let mut event_schedule: Vec<(f64, &ReplayEvent)> = Vec::new();
        {
            let mut abs_time: f64 = 0.0;
            for (delay_ms, evt) in &replay_events {
                abs_time += *delay_ms as f64 / speed;
                event_schedule.push((abs_time, evt));
            }
        }

        let mut event_cursor: usize = 0;
        let mut replay_turn_id: u64 = 0;

        terminal.draw(|f| crate::tui::render_frame(f, &self))?;
        frames.push((0.0, terminal.backend().buffer().clone()));

        let progress_interval = (total_duration_ms / 20.0).max(1000.0);
        let mut next_progress = progress_interval;

        while sim_time_ms <= total_duration_ms + frame_duration_ms {
            while event_cursor < event_schedule.len()
                && event_schedule[event_cursor].0 <= sim_time_ms
            {
                let (_t, event) = event_schedule[event_cursor];
                replay::apply_replay_event(
                    &mut self,
                    &mut remote,
                    event,
                    &mut replay_turn_id,
                    Some(sim_time_ms),
                );
                event_cursor += 1;
            }

            if sim_time_ms >= next_frame_at {
                replay::update_replay_elapsed_override(&mut self, sim_time_ms);
                terminal.draw(|f| crate::tui::render_frame(f, &self))?;
                frames.push((sim_time_ms / 1000.0, terminal.backend().buffer().clone()));
                next_frame_at = sim_time_ms + frame_duration_ms;
            }

            if sim_time_ms >= next_progress {
                let pct = (sim_time_ms / total_duration_ms * 100.0).min(100.0);
                eprint!("\r  Rendering... {:.0}%", pct);
                next_progress += progress_interval;
            }

            sim_time_ms += frame_duration_ms;
        }

        eprintln!("\r  Rendering... 100%  ({} frames captured)", frames.len());

        Ok(frames)
    }
}

#[cfg(test)]
mod tests {

    /// The regression that made the animation lag: an idle screen almost always
    /// has some slow-moving chrome up (notification line, status notice, cache
    /// countdown). If that chrome wins every tick, the animation runs at full
    /// frame cost. It must instead get a full frame only at its own cadence,
    /// with the animation frames in between served cheaply.
    /// The animation-only path reuses the previous full frame for everything
    /// outside the decorative rows, so it can never show a newer input line. It
    /// must therefore refuse to run while the composer differs from what the last
    /// full frame drew.
    ///
    /// Without this, a keystroke's redraw request was "satisfied" by an animation
    /// frame that did not contain the character, and the glyph waited for a later
    /// full frame. Measured against a live client, typing echoed to the terminal
    /// in ~7ms but only reached the screen ~500ms later; with the guard the same
    /// keystroke paints in ~6ms.
    ///
    /// Every existing test missed this because they render single frames and
    /// compare pixels: the partial frame is *correct*, it is simply the wrong
    /// frame to have drawn, which is only visible across frames over time.
    #[test]
    fn the_animation_only_path_refuses_to_run_when_the_composer_changed() {
        // Pretend a full frame drew an empty composer.
        // Struct update so a new renderer field cannot break this test.
        let renderer = StatusSpinnerRenderer {
            last_frame: Some(Buffer::empty(Rect::new(0, 0, 10, 3))),
            ..Default::default()
        };

        // Same input: the guard must not object (other predicates may still
        // block, which is why this asserts the reason rather than the outcome).
        assert!(
            !renderer.composer_changed_since_last_full_frame(""),
            "an unchanged composer must not block the cheap path"
        );

        // A keystroke landed. The path cannot render it, so it must be blocked.
        assert!(
            renderer.composer_changed_since_last_full_frame("/"),
            "a typed character must force a full frame"
        );
    }

    /// Invalidation must clear the remembered input too. A stale value would let
    /// the animation-only path run against a frame that no longer exists.
    #[test]
    fn invalidating_the_renderer_forgets_the_drawn_composer() {
        let mut renderer = StatusSpinnerRenderer {
            last_full_frame_input: "draft".to_string(),
            ..Default::default()
        };
        renderer.invalidate();
        assert!(
            renderer.composer_changed_since_last_full_frame("draft"),
            "after invalidation nothing is known to be drawn, so any composer \
             contents must force a full frame"
        );
    }

    use super::*;
    use ratatui::style::Color;

    #[tokio::test]
    async fn redraw_timer_waits_one_period_and_skips_missed_ticks() {
        let mut timer = redraw_timer(Duration::from_millis(250));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), timer.tick())
                .await
                .is_err(),
            "the first redraw tick must not fire immediately"
        );
        assert_eq!(
            timer.missed_tick_behavior(),
            tokio::time::MissedTickBehavior::Skip
        );
    }

    fn assert_duration_close(actual: Duration, expected: Duration) {
        let actual_ms = actual.as_millis() as i128;
        let expected_ms = expected.as_millis() as i128;
        assert!(
            (actual_ms - expected_ms).abs() <= 1,
            "expected {actual:?} to be within 1ms of {expected:?}"
        );
    }

    #[test]
    fn status_spinner_fast_path_uses_status_elapsed_clock() {
        let full_status_elapsed = 0.0;
        let app_lifetime_elapsed = 0.24;

        let full_status_symbol = jcode_tui_style::theme::activity_indicator(
            full_status_elapsed,
            STATUS_SPINNER_FPS,
            true,
        );
        let old_app_lifetime_symbol = jcode_tui_style::theme::activity_indicator(
            app_lifetime_elapsed,
            STATUS_SPINNER_FPS,
            true,
        );
        let fast_path_symbol = jcode_tui_style::theme::activity_indicator(
            status_spinner_elapsed_for_sources(Some(full_status_elapsed)),
            STATUS_SPINNER_FPS,
            true,
        );

        assert_ne!(
            old_app_lifetime_symbol, full_status_symbol,
            "the app lifetime clock can be on a different spinner frame than the status clock"
        );
        assert_eq!(fast_path_symbol, full_status_symbol);
    }

    #[test]
    fn primary_spinner_statuses_are_explicit() {
        assert!(status_uses_primary_spinner(&ProcessingStatus::Sending));
        assert!(status_uses_primary_spinner(&ProcessingStatus::Streaming));
        assert!(!status_uses_primary_spinner(
            &ProcessingStatus::RunningTool("bash".to_string())
        ));
        assert!(!status_uses_primary_spinner(&ProcessingStatus::Idle));
        assert!(!status_uses_primary_spinner(
            &ProcessingStatus::WaitingForNetwork {
                listener: "network".to_string(),
            }
        ));
    }

    #[test]
    fn slash_command_palette_suspends_spinner_fast_path() {
        assert!(is_slash_command_input("/"));
        assert!(is_slash_command_input("  /help"));
        assert!(!is_slash_command_input("normal prompt"));
    }

    /// The palette predicate must short-circuit on the input prefix so hot
    /// paths never build the suggestion list for ordinary prompts.
    #[test]
    fn palette_visibility_checks_the_cheap_prefix_before_building_suggestions() {
        let mut suggestions_built = false;
        assert!(!slash_command_palette_may_be_visible("hello", || {
            suggestions_built = true;
            true
        }));
        assert!(
            !suggestions_built,
            "non-slash input must not pay for the suggestion list"
        );

        assert!(slash_command_palette_may_be_visible("/", || true));
        assert!(!slash_command_palette_may_be_visible("/", || false));
    }

    /// The bug this pins: on a fresh idle screen (donut spinning), pressing `/`
    /// opens the command palette, which floats over the animation rows. Full
    /// frames painted the palette, then the very next animation-only repaint
    /// reset those rows and erased it, so the menu blinked in and out at the
    /// chrome full-frame cadence (~4 Hz), captured frame-by-frame from a live
    /// tester PTY. While the palette may be visible, animation ticks must take
    /// the full-frame path, which re-renders the overlay.
    #[test]
    fn the_animation_only_path_refuses_to_run_while_the_command_palette_is_up() {
        let clean_idle_frame = IdleAnimationFastPathInputs {
            has_previous_frame: true,
            animation_active: true,
            has_animation_area: true,
            force_full_redraw: false,
            force_full_repaint: false,
            composer_changed: false,
            command_palette_visible: false,
        };
        assert_eq!(
            idle_animation_fast_path_blocked_reason(&clean_idle_frame),
            None,
            "a clean idle frame must keep the cheap path available"
        );

        let palette_open = IdleAnimationFastPathInputs {
            command_palette_visible: true,
            ..clean_idle_frame
        };
        assert_eq!(
            idle_animation_fast_path_blocked_reason(&palette_open),
            Some("command_palette_visible"),
            "an open palette must force full frames so the overlay survives"
        );

        // A keystroke that both changes the composer and opens the palette
        // reports the keystroke: it is the more urgent of the two reasons.
        let typing_into_palette = IdleAnimationFastPathInputs {
            composer_changed: true,
            command_palette_visible: true,
            ..clean_idle_frame
        };
        assert_eq!(
            idle_animation_fast_path_blocked_reason(&typing_into_palette),
            Some("input_changed"),
        );
    }

    #[test]
    fn status_spinner_reset_targets_next_frame_boundary() {
        assert_duration_close(
            status_spinner_delay_until_next_frame(0.0),
            STATUS_SPINNER_ONLY_INTERVAL,
        );
        assert_duration_close(
            status_spinner_delay_until_next_frame(0.040),
            Duration::from_millis(40),
        );
        assert_duration_close(
            status_spinner_delay_until_next_frame(1.0),
            Duration::from_millis(40),
        );
        assert_duration_close(
            status_spinner_delay_until_next_frame(f32::NAN),
            STATUS_SPINNER_ONLY_INTERVAL,
        );
    }

    #[test]
    fn status_spinner_partial_mutates_only_status_cell() {
        let area = Rect::new(0, 0, 8, 2);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "abcdefgh", Style::default().fg(Color::White));
        buffer.set_string(0, 1, "ABCDEFGH", Style::default().fg(Color::Blue));
        buffer
            .cell_mut((2, 1))
            .expect("status cell")
            .set_symbol("⠋");
        let before = buffer.clone();

        let status_area = Rect::new(2, 1, 6, 1);
        assert!(render_status_spinner_into_buffer(&buffer, status_area, "⠙"));
        render_status_spinner_into_buffer_mut(&mut buffer, status_area, "⠙");

        for y in 0..2 {
            for x in 0..8 {
                if (x, y) == (2, 1) {
                    assert_eq!(buffer.cell((x, y)).unwrap().symbol(), "⠙");
                    assert_eq!(
                        buffer.cell((x, y)).unwrap().fg,
                        jcode_tui_style::theme::ai_color()
                    );
                } else {
                    assert_eq!(buffer.cell((x, y)), before.cell((x, y)));
                }
            }
        }
    }

    #[test]
    fn status_spinner_partial_does_not_overwrite_slash_palette_cell() {
        let area = Rect::new(0, 0, 12, 1);
        let mut buffer = Buffer::empty(area);
        buffer.set_string(0, 0, "/help  show help", Style::default().fg(Color::Yellow));

        assert!(
            !render_status_spinner_into_buffer(&buffer, area, "⠙"),
            "late overlays own the status cell until the next full frame"
        );
    }
}
