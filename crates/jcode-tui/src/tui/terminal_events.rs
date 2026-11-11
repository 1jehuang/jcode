//! Runtime terminal-background queries using the existing crossterm reader.
//!
//! Never run colorsaurus once an event reader exists: two stdin readers race
//! and a color report can become composer input. Crossterm 0.29 decodes OSC
//! reports as Alt+] / ordinary characters / Alt+\\ (or Ctrl+G). Intercept them
//! before *any* picker, shortcut, or composer sees them. The async reader and
//! synchronous burst drains share this filter, just like crossterm's reader.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures::Stream;
use jcode_tui_style::ThemeMode;
use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

const QUERY_INTERVAL: Duration = Duration::from_secs(5);
/// Query cadence once the terminal reported FocusLost. Regaining focus queries
/// immediately, so a background pane only needs an occasional refresh. Dozens
/// of parked clients each asking the terminal every five seconds also woke the
/// terminal emulator for every one of them.
const UNFOCUSED_QUERY_INTERVAL: Duration = Duration::from_secs(60);
/// A client with no user input for this long is treated like a background
/// pane for query cadence. Focus reports alone are not enough: a terminal may
/// not send them, and a client that missed FocusLost believes it is focused
/// forever. Each query also costs a reply that wakes the input thread and the
/// event loop, which measured about one wakeup a second per parked client.
const INPUT_IDLE_AFTER: Duration = Duration::from_secs(60);
/// How often to re-check whether runtime queries became enabled (config
/// change) when nothing else needs the timer.
const DISABLED_RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const PREFIX_TIMEOUT: Duration = Duration::from_millis(250);
const REPORT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPORT_LEN: usize = 40;
const CSI_TAIL_HOLD_TIMEOUT: Duration = Duration::from_millis(50);
const CSI_TAIL_INTER_BYTE: Duration = Duration::from_millis(50);
const CSI_TAIL_MAX_LEN: usize = 16;

static THEME_REDRAW_PENDING: AtomicBool = AtomicBool::new(false);

pub(super) fn theme_redraw_pending() -> bool {
    THEME_REDRAW_PENDING.load(Ordering::Relaxed)
}

pub(super) fn take_theme_redraw() -> bool {
    THEME_REDRAW_PENDING.swap(false, Ordering::Relaxed)
}

#[derive(Default)]
struct ColorReports {
    held: Vec<Event>,
    body: String,
    started: Option<Instant>,
    ready: VecDeque<Event>,
    mode: Option<ThemeMode>,
    expecting_until: Option<Instant>,
    split_escape: bool,
}

impl ColorReports {
    fn recognized(&self) -> bool {
        self.body.starts_with("11;rgb:") || self.body.starts_with("11;rgba:")
    }

    fn reset(&mut self, replay: bool) {
        if replay {
            self.ready.extend(self.held.drain(..));
        } else {
            self.held.clear();
        }
        self.body.clear();
        self.started = None;
        self.split_escape = false;
    }

    /// When the in-flight report (if any) must be expired.
    fn deadline(&self) -> Option<Instant> {
        let timeout = if self.recognized() {
            REPORT_TIMEOUT
        } else {
            PREFIX_TIMEOUT
        };
        self.started.map(|started| started + timeout)
    }

    fn expire(&mut self, now: Instant) {
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            // Replay an ordinary Alt+] chord, but never replay half a report.
            self.reset(!self.recognized());
        }
    }

    fn push(&mut self, event: Event, now: Instant) {
        self.expire(now);
        let Event::Key(key) = &event else {
            if !self.recognized() {
                self.reset(true);
            }
            self.ready.push_back(event);
            return;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            self.ready.push_back(event);
            return;
        }
        // A read ending exactly after ESC makes crossterm emit a bare Escape
        // key rather than an Alt chord. Reassemble only an expected query's
        // introducer, or the terminator of a report already being consumed.
        if self.split_escape {
            let plain = key.modifiers.is_empty();
            if self.body.is_empty() && key.code == KeyCode::Char(']') && plain {
                self.split_escape = false;
                self.held.push(event);
                return;
            }
            if self.recognized() && key.code == KeyCode::Char('\\') && plain {
                self.mode = parse_background_report(&self.body);
                self.expecting_until = None;
                self.reset(false);
                return;
            }
            if self.recognized() {
                let escape = self.held.pop();
                self.reset(false);
                if let Some(escape) = escape {
                    self.ready.push_back(escape);
                }
            } else {
                self.reset(true);
            }
        }
        if key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && (self.recognized()
                || (self.started.is_none()
                    && self.expecting_until.is_some_and(|until| now < until)))
        {
            self.started.get_or_insert(now);
            self.split_escape = true;
            self.held.push(event);
            return;
        }
        if key.code == KeyCode::Char(']') && key.modifiers == KeyModifiers::ALT {
            self.reset(!self.recognized());
            self.started = Some(now);
            self.held.push(event);
            return;
        }
        if self.started.is_none() {
            self.ready.push_back(event);
            return;
        }
        let terminator = (key.code == KeyCode::Char('\\') && key.modifiers == KeyModifiers::ALT)
            || (key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::CONTROL);
        if terminator && self.recognized() {
            self.mode = parse_background_report(&self.body);
            self.expecting_until = None;
            self.reset(false);
            return;
        }
        if let KeyCode::Char(ch) = key.code
            && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
            && ch.is_ascii()
        {
            self.body.push(ch);
            self.held.push(event);
            let prefix = "11;rgb:".starts_with(&self.body) || "11;rgba:".starts_with(&self.body);
            if self.body.len() > MAX_REPORT_LEN || (!prefix && !self.recognized()) {
                self.reset(!self.recognized());
            }
        } else {
            self.reset(!self.recognized());
            self.ready.push_back(event);
        }
    }
}

fn parse_background_report(body: &str) -> Option<ThemeMode> {
    let (rgb, count) = if let Some(rgb) = body.strip_prefix("11;rgb:") {
        (rgb, 3)
    } else {
        (body.strip_prefix("11;rgba:")?, 4)
    };
    let components: Vec<_> = rgb.split('/').collect();
    if components.len() != count {
        return None;
    }
    let mut channels = Vec::with_capacity(count);
    for component in components {
        if !(1..=4).contains(&component.len()) || !component.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        let value = match u32::from_str_radix(component, 16) {
            Ok(value) => value,
            Err(error) => {
                crate::logging::debug(&format!("Invalid terminal color component: {error}"));
                return None;
            }
        };
        let max = (1u32 << (component.len() * 4)) - 1;
        channels.push((value * u32::from(u16::MAX) / max) as u16);
    }
    let background = terminal_colorsaurus::Color::rgb(channels[0], channels[1], channels[2]);
    Some(if background.perceived_lightness() > 0.5 {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    })
}

/// Swallow keyboard/mouse/focus reports that arrive ESC-split from terminals
/// with a partial kitty-keyboard implementation (#2140).
///
/// iTerm2 accepts the kitty push but writes report byte-chunks separately, so
/// a read can land exactly after the `ESC` byte. Crossterm then emits a bare
/// Escape press for the head and every tail byte (`[98;1:3u`, `[O`) as an
/// insertable character, and the fragments land in the composer as garbage.
/// A doubled escape in one write (`ESC ESC [;23M`) leaks the tail with no
/// head at all, and a report following a completed one (`ESC[O35;49;28M`)
/// leaks its continuation mid-sequence. Terminals that encode a report in one
/// write never split, so this guard stays dormant for them.
///
/// The guard runs only while jcode owns a kitty push, and only swallows
/// bursts that keep a 50ms inter-arrival pace: real report fragments arrive
/// in one burst while human typing cannot sustain that cadence. After a bare
/// Escape press it holds the event briefly, awaiting a continuation:
///
/// * `[` at burst pace: the tail of a split CSI report, matched against the
///   report grammar (parameters `0-9 ; : < ?`, then a final byte in
///   `0x40..=0x7e`). Completion reconstructs what the report carried: a key
///   event from its press half, `Esc` from the release of the escape that
///   armed the watch, a focus event from `[I`/`[O`, an SGR mouse event, or
///   nothing for reply noise. The held escape head was part of the report
///   and is consumed with it.
/// * `]`: a split OSC reply head; the guard stands down and lets the report
///   reassembler own the stream.
/// * anything else, or the 50ms deadline: everything held is replayed
///   untouched in arrival order, so a human pressing Escape and typing loses
///   nothing. Terminals that report event types disarm the hold with the
///   burst-paced release report of the same keypress.
///
/// A completed report briefly keeps watching (`Post`): a follow-up burst of
/// grammar bytes is the headless continuation of another report whose head
/// was consumed with the previous one. Incomplete matches always replay: the
/// guard drops report noise only on a complete grammar match.
#[derive(Default)]
struct CsiTailGuard {
    held: Vec<Event>,
    body: Vec<u8>,
    started: Option<Instant>,
    last_at: Option<Instant>,
    phase: CsiTailPhase,
}

#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
enum CsiTailPhase {
    /// No watch running.
    #[default]
    Idle,
    /// A bare Escape press is held, awaiting a burst-pace continuation.
    Armed,
    /// Report bytes are being matched against the grammar.
    Tail,
    /// A report just completed; a burst-paced follower is a headless report.
    Post,
}

/// What the guard decided about one raw event.
#[derive(Debug)]
enum GuardOutcome {
    /// Deliver these events to the app, in arrival order.
    Pass(Vec<Event>),
    /// Report noise was consumed silently.
    Drop,
    /// Event held; nothing to deliver yet.
    Held,
}

impl CsiTailGuard {
    fn enabled(&self) -> bool {
        super::KITTY_KEYBOARD_PUSH_ACTIVE.load(Ordering::Relaxed)
    }

    fn disarm(&mut self) {
        self.started = None;
        self.last_at = None;
        self.phase = CsiTailPhase::Idle;
        self.body.clear();
    }

    fn replay(&mut self) -> Vec<Event> {
        self.disarm();
        std::mem::take(&mut self.held)
    }

    fn replay_with(&mut self, event: Event) -> Vec<Event> {
        let mut replayed = self.replay();
        replayed.push(event);
        replayed
    }

    /// Deliver the current event, flushing anything held first so arrival
    /// order holds. A mid-watch violation means the held events were human
    /// input.
    fn pass(&mut self, event: Event) -> GuardOutcome {
        if self.phase == CsiTailPhase::Idle {
            return GuardOutcome::Pass(vec![event]);
        }
        GuardOutcome::Pass(self.replay_with(event))
    }

    fn hold(&mut self, event: Event, now: Instant) -> GuardOutcome {
        self.held.push(event);
        self.last_at = Some(now);
        GuardOutcome::Held
    }

    /// Absolute deadline of the running watch, if any. Armed and Post holds
    /// are short: their continuation arrives within the same burst or not at
    /// all. A Tail dies with the burst pace: report bytes never pause
    /// between arrivals.
    fn pending_deadline(&self) -> Option<Instant> {
        match self.phase {
            CsiTailPhase::Idle => None,
            CsiTailPhase::Armed | CsiTailPhase::Post => {
                self.started.map(|started| started + CSI_TAIL_HOLD_TIMEOUT)
            }
            CsiTailPhase::Tail => self.last_at.map(|last| last + CSI_TAIL_INTER_BYTE * 2),
        }
    }

    fn expired(&self, now: Instant) -> bool {
        self.pending_deadline()
            .is_some_and(|deadline| now >= deadline)
    }

    fn plain_ascii_char(code: &KeyCode, modifiers: &KeyModifiers) -> Option<char> {
        let KeyCode::Char(ch) = *code else {
            return None;
        };
        (modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT)
            .then_some(ch)
            .filter(|ch| ch.is_ascii())
    }

    fn tail_param(byte: u8) -> bool {
        matches!(byte, b'0'..=b'9' | b';' | b':' | b'<' | b'?')
    }

    fn burst_paced(&self, now: Instant) -> bool {
        self.last_at
            .is_some_and(|last| now.duration_since(last) < CSI_TAIL_INTER_BYTE)
    }

    /// Offer one event to the guard.
    fn push(&mut self, event: Event, now: Instant) -> GuardOutcome {
        if !self.enabled() {
            if self.phase != CsiTailPhase::Idle {
                return GuardOutcome::Pass(self.replay_with(event));
            }
            return GuardOutcome::Pass(vec![event]);
        }
        if self.expired(now) {
            // The deadline passed without a completion: everything held was
            // human input, replay it in arrival order.
            return GuardOutcome::Pass(self.replay_with(event));
        }
        let Event::Key(key) = event else {
            return self.pass(event);
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return self.pass(Event::Key(key));
        }
        match self.phase {
            CsiTailPhase::Idle => self.push_idle(key, now),
            CsiTailPhase::Armed => self.push_armed(key, now),
            CsiTailPhase::Tail => self.push_tail(key, now),
            CsiTailPhase::Post => self.push_post(key, now),
        }
    }

    fn push_idle(&mut self, key: KeyEvent, now: Instant) -> GuardOutcome {
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            self.phase = CsiTailPhase::Armed;
            self.started = Some(now);
            return self.hold(Event::Key(key), now);
        }
        GuardOutcome::Pass(vec![Event::Key(key)])
    }

    fn push_armed(&mut self, key: KeyEvent, now: Instant) -> GuardOutcome {
        let event = Event::Key(key.clone());
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            // A doubled escape head (`ESC ESC [ ...`) holds and keeps waiting.
            return self.hold(event, now);
        }
        let Some(ch) = Self::plain_ascii_char(&key.code, &key.modifiers) else {
            return self.pass(event);
        };
        if ch == ']' {
            // Split OSC reply head: the report reassembler owns the stream.
            return self.pass(event);
        }
        // A split CSI report always continues with its introducer (`ESC [`).
        if ch == '[' && self.burst_paced(now) {
            self.phase = CsiTailPhase::Tail;
            self.body.push(b'[');
            return self.hold(event, now);
        }
        // A human pressed Escape and typed. A split legacy chord cannot
        // occur here: while jcode owns a kitty push, chords arrive as
        // complete CSI reports, and without the push this guard is off.
        self.pass(event)
    }

    fn push_tail(&mut self, key: KeyEvent, now: Instant) -> GuardOutcome {
        let event = Event::Key(key.clone());
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            // Reports never embed a bare escape; this is fresh input.
            return self.pass(event);
        }
        let Some(ch) = Self::plain_ascii_char(&key.code, &key.modifiers) else {
            return self.pass(event);
        };
        if !self.burst_paced(now) {
            // Real report bytes arrive in one burst; human typing cannot
            // keep this pace.
            return self.pass(event);
        }
        if self.body.len() >= CSI_TAIL_MAX_LEN {
            return self.pass(event);
        }
        let byte = ch as u8;
        if Self::tail_param(byte) {
            self.body.push(byte);
            return self.hold(event, now);
        }
        if (0x40..=0x7e).contains(&byte) {
            self.body.push(byte);
            return self.complete(now);
        }
        self.pass(event)
    }

    /// The grammar completed: reconstruct what the report carried.
    fn complete(&mut self, now: Instant) -> GuardOutcome {
        let body = std::mem::take(&mut self.body);
        self.disarm();
        self.held.clear();
        let outcome = match Self::parse_report(&body) {
            Some(event) => GuardOutcome::Pass(vec![event]),
            None => GuardOutcome::Drop,
        };
        // Keep watching briefly: another report may follow with its head
        // consumed, leaking only grammar bytes.
        self.phase = CsiTailPhase::Post;
        self.started = Some(now);
        self.last_at = Some(now);
        outcome
    }

    fn push_post(&mut self, key: KeyEvent, now: Instant) -> GuardOutcome {
        let event = Event::Key(key.clone());
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            // The next report's split head: arm on it.
            self.phase = CsiTailPhase::Armed;
            self.started = Some(now);
            return self.hold(event, now);
        }
        let Some(ch) = Self::plain_ascii_char(&key.code, &key.modifiers) else {
            return self.pass(event);
        };
        // A report that lost its escape head leaks grammar bytes right after
        // the previous report completed (`[O35;49;28M`).
        if Self::tail_param(ch as u8) && self.burst_paced(now) {
            self.phase = CsiTailPhase::Tail;
            self.body.push(ch as u8);
            return self.hold(event, now);
        }
        self.pass(event)
    }

    /// Reconstruct one report from its matched tail bytes.
    fn parse_report(body: &[u8]) -> Option<Event> {
        let body = body.strip_prefix(b"[")?;
        let final_byte = *body.last()?;
        let params = std::str::from_utf8(&body[..body.len() - 1]).ok()?;
        match final_byte {
            b'I' if params.is_empty() => Some(Event::FocusGained),
            b'O' if params.is_empty() => Some(Event::FocusLost),
            b'M' => Self::parse_sgr_mouse(params, false),
            b'm' => Self::parse_sgr_mouse(params, true),
            b'u' => Self::parse_csi_u(params),
            // Other replies (device attributes, keyboard flags) and noise
            // from a split head: consumed with no reconstruction.
            _ => None,
        }
    }

    /// SGR mouse report (`CSI < cb ; cx ; cy M/m`), mirroring crossterm's
    /// decoder so a split report still delivers the click.
    fn parse_sgr_mouse(params: &str, released: bool) -> Option<Event> {
        let params = params.strip_prefix('<')?;
        let mut parts = params.split(';');
        let cb: u8 = parts.next()?.parse().ok()?;
        let cx: u16 = parts.next()?.parse().ok()?;
        let cy: u16 = parts.next()?.parse().ok()?;
        // Extra parameters (a trailing `;` before the final byte) are
        // ignored, exactly like crossterm's decoder.
        let button = (cb & 0b0000_0011) | ((cb & 0b1100_0000) >> 4);
        let dragging = cb & 0b0010_0000 == 0b0010_0000;
        let mut modifiers = KeyModifiers::NONE;
        if cb & 0b0000_0100 != 0 {
            modifiers |= KeyModifiers::SHIFT;
        }
        if cb & 0b0000_1000 != 0 {
            modifiers |= KeyModifiers::ALT;
        }
        if cb & 0b0001_0000 != 0 {
            modifiers |= KeyModifiers::CONTROL;
        }
        let kind = match (button, dragging) {
            (0, false) => MouseEventKind::Down(MouseButton::Left),
            (1, false) => MouseEventKind::Down(MouseButton::Middle),
            (2, false) => MouseEventKind::Down(MouseButton::Right),
            (0, true) => MouseEventKind::Drag(MouseButton::Left),
            (1, true) => MouseEventKind::Drag(MouseButton::Middle),
            (2, true) => MouseEventKind::Drag(MouseButton::Right),
            (3, false) => MouseEventKind::Up(MouseButton::Left),
            (3, true) | (4, true) | (5, true) => MouseEventKind::Moved,
            (4, false) => MouseEventKind::ScrollUp,
            (5, false) => MouseEventKind::ScrollDown,
            (6, false) => MouseEventKind::ScrollLeft,
            (7, false) => MouseEventKind::ScrollRight,
            _ => return None,
        };
        let kind = if released {
            match kind {
                MouseEventKind::Down(button) => MouseEventKind::Up(button),
                other => other,
            }
        } else {
            kind
        };
        Some(Event::Mouse(MouseEvent {
            kind,
            // SGR reports 1-based coordinates; crossterm delivers 0-based.
            column: cx.saturating_sub(1),
            row: cy.saturating_sub(1),
            modifiers,
        }))
    }

    /// Kitty `CSI u` key report, mirroring crossterm's decoder for the keys
    /// that can arrive split. Functional-key codepoints (0xE000 and up) only
    /// ever arrive intact, so they are dropped rather than mis-decoded.
    fn parse_csi_u(params: &str) -> Option<Event> {
        let mut parts = params.split(';');
        let codepoints = parts.next()?;
        // REPORT_ALTERNATE_KEYS appends `:alternates` after the key code.
        let code: u32 = codepoints.split(':').next()?.parse().ok()?;
        let mods_and_kind = parts.next().unwrap_or("");
        let (mods, kind) = mods_and_kind
            .split_once(':')
            .unwrap_or((mods_and_kind, "1"));
        let mods: u8 = if mods.is_empty() {
            1
        } else {
            mods.parse().ok()?
        };
        let kind: u8 = kind.parse().ok()?;
        // A third parameter (text-as-codepoints) is ignored, like crossterm.
        if kind == 3 {
            // A split release report: its press half was the bare escape
            // that armed this watch. Only an unmodified Esc press arrives
            // that way; every other press was delivered intact already.
            return (code == 27 && mods == 1)
                .then(|| Self::key_event(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press));
        }
        let kind = match kind {
            1 => KeyEventKind::Press,
            2 => KeyEventKind::Repeat,
            _ => return None,
        };
        let modifiers = Self::kitty_modifiers(mods);
        let key = match char::from_u32(code)? {
            '\x1b' => KeyCode::Esc,
            '\r' => KeyCode::Enter,
            '\t' => {
                if modifiers.contains(KeyModifiers::SHIFT) {
                    KeyCode::BackTab
                } else {
                    KeyCode::Tab
                }
            }
            '\x7f' => KeyCode::Backspace,
            ch if code < 0xE000 && !ch.is_control() => KeyCode::Char(ch),
            _ => return None,
        };
        Some(Self::key_event(key, modifiers, kind))
    }

    fn kitty_modifiers(mods: u8) -> KeyModifiers {
        let mask = mods.saturating_sub(1);
        let mut modifiers = KeyModifiers::NONE;
        if mask & 1 != 0 {
            modifiers |= KeyModifiers::SHIFT;
        }
        if mask & 2 != 0 {
            modifiers |= KeyModifiers::ALT;
        }
        if mask & 4 != 0 {
            modifiers |= KeyModifiers::CONTROL;
        }
        if mask & 8 != 0 {
            modifiers |= KeyModifiers::SUPER;
        }
        if mask & 16 != 0 {
            modifiers |= KeyModifiers::HYPER;
        }
        if mask & 32 != 0 {
            modifiers |= KeyModifiers::META;
        }
        modifiers
    }

    fn key_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent::new_with_kind(code, modifiers, kind))
    }

    /// Deadline expiry for the shared readers: replay whatever was held
    /// straight to the ready queue. The result bypasses the guard (it would
    /// re-arm on the same escape) but still flows through the report
    /// reassembler, which may legitimately hold an expected OSC head.
    fn tick_expire(&mut self, now: Instant) -> Option<Vec<Event>> {
        let expired = self.expired(now);
        expired.then(|| self.replay())
    }
}

#[derive(Default)]
struct RuntimeTheme {
    reports: ColorReports,
    csi_guard: CsiTailGuard,
    last_query: Option<Instant>,
    cache_query_at: Option<Instant>,
    /// Result of the last `runtime_queries_enabled` check, so computing the
    /// next wake does not repeat its isatty/env probes on every poll.
    queries_enabled: bool,
    /// True once the terminal reported FocusLost. Terminals without focus
    /// reporting never send it, so they keep the focused cadence.
    unfocused: bool,
    /// When the last user event was delivered. Color reports never count.
    last_input: Option<Instant>,
}

impl RuntimeTheme {
    fn note_focus(&mut self, event: &Event) {
        match event {
            Event::FocusGained => self.unfocused = false,
            Event::FocusLost => self.unfocused = true,
            _ => {}
        }
    }

    fn note_input(&mut self, event: &Event, now: Instant) {
        if *event != Event::FocusLost {
            self.last_input = Some(now);
        }
    }

    fn timer_query_interval(&self, now: Instant) -> Duration {
        let input_idle = self
            .last_input
            .is_none_or(|at| now.saturating_duration_since(at) >= INPUT_IDLE_AFTER);
        if self.unfocused || input_idle {
            UNFOCUSED_QUERY_INTERVAL
        } else {
            QUERY_INTERVAL
        }
    }

    /// Earliest instant the timer has work to do: expire a held report, read
    /// the tmux cache, or send the next periodic query. An idle client used to
    /// tick every 250ms forever just in case one of these was pending.
    fn next_wake(&self, now: Instant) -> Instant {
        let mut at = now + DISABLED_RECHECK_INTERVAL;
        if let Some(deadline) = self.reports.deadline() {
            at = at.min(deadline);
        }
        if let Some(cache_query_at) = self.cache_query_at {
            at = at.min(cache_query_at);
        }
        if self.queries_enabled {
            at = at.min(
                self.last_query
                    .map_or(now, |last| last + self.timer_query_interval(now)),
            );
        }
        at
    }
}

fn runtime() -> &'static Mutex<RuntimeTheme> {
    static RUNTIME: OnceLock<Mutex<RuntimeTheme>> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Mutex::new(RuntimeTheme {
            // A re-executed background client stays on the slow query cadence.
            unfocused: !crate::tui::focus_carryover::initial_focus(),
            ..RuntimeTheme::default()
        })
    })
}

fn background_query(passthrough: bool) -> &'static [u8] {
    if passthrough {
        // tmux caches ordinary OSC 11 replies at attachment. Ask the outer
        // terminal directly, doubling ESC bytes inside its DCS envelope.
        b"\x1bPtmux;\x1b\x1b]11;?\x1b\x1b\\\x1b\\"
    } else {
        b"\x1b]11;?\x1b\\"
    }
}

fn tmux_passthrough_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let term = std::env::var_os("TERM")
            .map_or_else(String::new, |value| value.to_string_lossy().into_owned());
        if std::env::var_os("TMUX").is_none()
            || !(term.starts_with("tmux") || term.starts_with("screen"))
        {
            return false;
        }
        use std::process::{Command, Stdio};
        let Ok(mut child) = Command::new("tmux")
            .args(["show-options", "-Apv", "allow-passthrough"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        // Only once per TUI, bounded so a broken/missing mux cannot stall input.
        let deadline = Instant::now() + Duration::from_millis(150);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    return child
                        .wait_with_output()
                        .is_ok_and(|output| matches!(output.stdout.trim_ascii(), b"on" | b"all"));
                }
                Ok(Some(_)) => return false,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                _ => {
                    if let Err(error) = child.kill() {
                        crate::logging::debug(&format!("Failed to stop tmux probe: {error}"));
                    }
                    if let Err(error) = child.wait() {
                        crate::logging::debug(&format!("Failed to reap tmux probe: {error}"));
                    }
                    return false;
                }
            }
        }
    })
}

fn query_if_due(state: &mut RuntimeTheme, now: Instant, focused: bool) {
    state.queries_enabled = super::theme_detect::runtime_queries_enabled();
    if !state.queries_enabled {
        return;
    }
    if state.cache_query_at.is_some_and(|at| now >= at) {
        state.cache_query_at = None;
        // tmux consumes outer-terminal color replies to update its cache.
        // Read that refreshed cache on the next timer tick, without blocking
        // the input stream or competing for stdin.
        write_background_query(state, now, false);
        return;
    }
    let interval = if focused {
        Duration::from_secs(1)
    } else {
        state.timer_query_interval(now)
    };
    if state
        .last_query
        .is_some_and(|last| now.duration_since(last) < interval)
    {
        return;
    }
    state.last_query = Some(now);
    let passthrough = tmux_passthrough_enabled();
    if passthrough {
        state.cache_query_at = Some(now + PREFIX_TIMEOUT);
    }
    write_background_query(state, now, passthrough);
}

fn write_background_query(state: &mut RuntimeTheme, now: Instant, passthrough: bool) {
    // Query only the background. No stdin read and no blocking response wait.
    let query = background_query(passthrough);
    let mut stdout = io::stdout().lock();
    if stdout
        .write_all(query)
        .and_then(|()| stdout.flush())
        .is_ok()
    {
        state.reports.expecting_until = Some(now + REPORT_TIMEOUT);
    }
}

fn take_event(state: &mut RuntimeTheme) -> Option<Event> {
    if let Some(mode) = state.reports.mode.take()
        && super::theme_detect::auto_theme_enabled()
        && mode != jcode_tui_style::theme_mode()
    {
        jcode_tui_style::set_theme_mode(mode);
        THEME_REDRAW_PENDING.store(true, Ordering::Relaxed);
        // Reuse the existing resize invalidation/redraw path without inventing
        // a keypress or changing focus state. The actual dimensions stay intact.
        if let Ok((width, height)) = crossterm::terminal::size() {
            return Some(Event::Resize(width, height));
        }
    }
    let event = state.reports.ready.pop_front()?;
    state.note_input(&event, Instant::now());
    Some(event)
}

/// Feed one raw crossterm event through the CSI-tail guard, then the color
/// report filter. Guard-held events reach neither; replayed bursts and
/// everything the guard passes through land in `reports.ready` in arrival
/// order, so both the async reader and the burst drains see one stream.
fn push_raw_event(state: &mut RuntimeTheme, event: Event, now: Instant) {
    match state.csi_guard.push(event, now) {
        GuardOutcome::Pass(events) => {
            for event in events {
                if event == Event::FocusGained {
                    query_if_due(state, now, true);
                }
                state.reports.push(event, now);
            }
        }
        GuardOutcome::Drop => {}
        GuardOutcome::Held => {}
    }
}

/// Nonblocking burst drain with the same report filter as [`EventStream`].
/// Returning None means there is currently no user event, even if report bytes
/// were consumed. The async reader will deliver any remaining report/replay.
pub(super) fn try_read() -> io::Result<Option<Event>> {
    let mut state = runtime().lock().unwrap_or_else(|error| error.into_inner());
    expire_guard_into(&mut state);
    if let Some(event) = take_event(&mut state) {
        return Ok(Some(event));
    }
    for _ in 0..64 {
        if !crossterm::event::poll(Duration::ZERO)? {
            return Ok(None);
        }
        let event = crossterm::event::read()?;
        let now = Instant::now();
        state.note_focus(&event);
        push_raw_event(&mut state, event, now);
        if let Some(event) = take_event(&mut state) {
            return Ok(Some(event));
        }
    }
    Ok(None)
}

/// Crossterm's event stream, with nonblocking periodic OSC 11 queries and report
/// filtering. The timer is polled even during LLM output and tool execution.
pub(super) struct EventStream {
    inner: crossterm::event::EventStream,
    tick: Pin<Box<tokio::time::Sleep>>,
    /// Deadline `tick` is currently armed for.
    armed: Instant,
}

impl EventStream {
    pub(super) fn new() -> Self {
        Self {
            inner: crossterm::event::EventStream::new(),
            tick: Box::pin(tokio::time::sleep(Duration::ZERO)),
            armed: Instant::now(),
        }
    }

    /// Fire the periodic tick early enough to expire a guard hold on time.
    fn shorten_tick(&mut self, state: &RuntimeTheme) {
        let Some(deadline) = state.csi_guard.pending_deadline() else {
            return;
        };
        let deadline = tokio::time::Instant::from_std(deadline);
        let normal = tokio::time::Instant::now() + PREFIX_TIMEOUT;
        self.tick.as_mut().reset(deadline.min(normal));
    }

    /// Arm the timer for the next instant it has work. `fired` re-arms
    /// unconditionally. Otherwise the timer only moves earlier (a report just
    /// started being held), so an armed deadline is not reset on every poll.
    fn rearm(&mut self, state: &RuntimeTheme, fired: bool, cx: &mut Context<'_>) {
        let now = Instant::now();
        let want = state.next_wake(now);
        if !fired && want >= self.armed {
            return;
        }
        self.armed = want;
        self.tick
            .as_mut()
            .reset(tokio::time::Instant::from_std(want));
        if self.tick.as_mut().poll(cx).is_ready() {
            cx.waker().wake_by_ref();
        }
    }

    fn poll_events(
        &mut self,
        state: &mut RuntimeTheme,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<Event>>> {
        if let Some(event) = take_event(state) {
            return Poll::Ready(Some(Ok(event)));
        }
        // Bound the work per poll while draining the key-shaped color report.
        for _ in 0..64 {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    let now = Instant::now();
                    state.note_focus(&event);
                    push_raw_event(&mut state, event, now);
                    if let Some(event) = take_event(&mut state) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
                other => {
                    if other.is_pending() {
                        self.shorten_tick(&state);
                    }
                    return other;
                }
            }
        }
        self.shorten_tick(&state);
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl Stream for EventStream {
    type Item = io::Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let mut state = runtime().lock().unwrap_or_else(|error| error.into_inner());
        // Poll the timer first so a continuously-ready stream of input cannot
        // starve a theme refresh. It only writes a tiny query every five seconds.
        let fired = this.tick.as_mut().poll(cx).is_ready();
        if fired {
            let now = Instant::now();
            state.reports.expire(now);
            expire_guard_into(&mut state);
            query_if_due(&mut state, now, false);
        }
        let result = this.poll_events(&mut state, cx);
        // Arm after draining input: a report that just started being held needs
        // its expiry timer, and a query just sent moves the next one out.
        this.rearm(&state, fired, cx);
        result
    }
}

/// Expire an overdue guard hold, replaying human input into the report
/// reassembler. Replays must bypass the guard itself: feeding them through
/// would arm on the same bare escape all over again.
fn expire_guard_into(state: &mut RuntimeTheme) {
    let now = Instant::now();
    if let Some(events) = state.csi_guard.tick_expire(now) {
        for event in events {
            state.reports.push(event, now);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn background_queries_bypass_tmux_cache_only_with_passthrough() {
        assert_eq!(super::background_query(false), b"\x1b]11;?\x1b\\");
        assert_eq!(
            super::background_query(true),
            b"\x1bPtmux;\x1b\x1b]11;?\x1b\x1b\\\x1b\\"
        );
    }
    use super::*;
    use crossterm::event::KeyEvent;

    #[test]
    fn idle_timer_sleeps_until_the_next_query_instead_of_polling() {
        let now = Instant::now();
        let mut state = RuntimeTheme::default();
        // Queries disabled and nothing held: only the slow enablement recheck.
        assert_eq!(state.next_wake(now), now + DISABLED_RECHECK_INTERVAL);

        state.queries_enabled = true;
        assert_eq!(state.next_wake(now), now, "first query is due at once");
        state.last_query = Some(now);
        // No user input yet: a parked client keeps the slow cadence even if it
        // believes it is focused.
        assert_eq!(state.next_wake(now), now + UNFOCUSED_QUERY_INTERVAL);
        state.note_input(&key('x', KeyModifiers::NONE), now);
        assert_eq!(state.next_wake(now), now + QUERY_INTERVAL);
        assert!(QUERY_INTERVAL > PREFIX_TIMEOUT * 4);
        // Input went quiet: back to the slow cadence. Typing again makes the
        // overdue query due at once.
        let later = now + INPUT_IDLE_AFTER;
        assert_eq!(state.next_wake(later), now + UNFOCUSED_QUERY_INTERVAL);
        state.note_input(&key('x', KeyModifiers::NONE), later);
        assert_eq!(state.next_wake(later), now + QUERY_INTERVAL);
        state.note_input(&Event::FocusLost, later + INPUT_IDLE_AFTER);
        assert_eq!(state.last_input, Some(later), "losing focus is not input");
        state.note_input(&key('x', KeyModifiers::NONE), now);

        state.note_focus(&Event::FocusLost);
        assert_eq!(state.next_wake(now), now + UNFOCUSED_QUERY_INTERVAL);
        state.note_focus(&Event::FocusGained);
        assert_eq!(state.next_wake(now), now + QUERY_INTERVAL);
    }

    #[test]
    fn held_report_and_tmux_cache_read_pull_the_timer_in() {
        let now = Instant::now();
        let mut state = RuntimeTheme {
            queries_enabled: true,
            last_query: Some(now),
            last_input: Some(now),
            ..RuntimeTheme::default()
        };
        // An Alt+] chord is held as a possible report prefix and must be
        // replayed after PREFIX_TIMEOUT even if no further input arrives.
        state.reports.push(key(']', KeyModifiers::ALT), now);
        assert_eq!(state.next_wake(now), now + PREFIX_TIMEOUT);
        state.reports.expire(now + PREFIX_TIMEOUT);
        assert_eq!(state.reports.ready.len(), 1);
        assert_eq!(state.next_wake(now), now + QUERY_INTERVAL);

        state.cache_query_at = Some(now + Duration::from_millis(100));
        assert_eq!(state.next_wake(now), now + Duration::from_millis(100));
    }

    fn key(ch: char, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(ch), modifiers))
    }

    fn escape() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    fn press(ch: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
    }

    /// The guard reads the process-global kitty-push flag; guard tests flip
    /// it, so only one may run at a time.
    static GUARD_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard_test_lock() -> std::sync::MutexGuard<'static, ()> {
        GUARD_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Enable the guard and feed `bytes` as one burst of plain character
    /// presses (the events crossterm emits for a report tail whose escape
    /// head was consumed by an earlier event).
    fn feed_guard_split_tail(
        guard: &mut CsiTailGuard,
        head: Event,
        tail: &[u8],
        now: Instant,
    ) -> Vec<Event> {
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        let mut delivered = Vec::new();
        let mut at = now;
        let mut outcome = guard.push(head, at);
        for byte in tail {
            at += Duration::from_millis(1);
            match outcome {
                GuardOutcome::Pass(events) => delivered.extend(events),
                GuardOutcome::Drop => {}
                GuardOutcome::Held => {}
            }
            outcome = guard.push(press(*byte as char), at);
        }
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        match outcome {
            GuardOutcome::Pass(events) => delivered.extend(events),
            GuardOutcome::Drop => {}
            GuardOutcome::Held => {}
        }
        delivered
    }

    fn collect_guard_held(guard: &mut CsiTailGuard, now: Instant) -> Vec<Event> {
        guard
            .tick_expire(now + CSI_TAIL_HOLD_TIMEOUT)
            .unwrap_or_default()
    }

    #[test]
    fn split_release_report_is_swallowed_not_leaked() {
        let _lock = guard_test_lock();
        // The literal shape from #2140: Alt+b's release report arrives as a
        // bare Escape press followed by `[98;1:3u` as insertable characters.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let delivered = feed_guard_split_tail(&mut guard, escape(), b"[98;1:3u", now);
        assert!(delivered.is_empty());
        assert_eq!(guard.phase, CsiTailPhase::Post);
    }

    #[test]
    fn doubled_escape_head_swallows_headless_modifier_noise() {
        let _lock = guard_test_lock();
        // `ESC ESC [;23M` in one write: crossterm emits one Escape press for
        // the doubled escape, then `[;23M` as plain characters.
        let mut guard = CsiTailGuard::default();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        let now = Instant::now();
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        assert!(matches!(
            guard.push(escape(), now + Duration::from_millis(1)),
            GuardOutcome::Held
        ));
        let mut at = now + Duration::from_millis(2);
        let mut outcome = guard.push(press('['), at);
        for ch in ";23M".chars() {
            at += Duration::from_millis(1);
            outcome = guard.push(press(ch), at);
        }
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        assert!(matches!(outcome, GuardOutcome::Drop));
        assert!(guard.held.is_empty());
    }

    #[test]
    fn focus_split_after_escape_synthesizes_focus_event() {
        let _lock = guard_test_lock();
        // `ESC [O` focus-lost split after the escape: the guard must deliver
        // a real FocusLost, and then keep watching (`Post`) for the mouse
        // report that followed it in the user's paste: `35;49;28M`.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let mut delivered = feed_guard_split_tail(&mut guard, escape(), b"[O", now);
        assert_eq!(delivered, vec![Event::FocusLost]);
        assert_eq!(guard.phase, CsiTailPhase::Post);
        // The headless mouse continuation follows within the burst window.
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        let mut at = now + Duration::from_millis(3);
        let mut outcome = guard.push(press('3'), at);
        for ch in "5;49;28M".chars() {
            at += Duration::from_millis(1);
            outcome = guard.push(press(ch), at);
        }
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        match outcome {
            GuardOutcome::Drop => {}
            other => panic!("expected headless mouse tail to drop, got {other:?}"),
        }
        delivered.extend(collect_guard_held(&mut guard, at));
        assert_eq!(delivered, vec![Event::FocusLost]);
    }

    #[test]
    fn split_press_report_reconstructs_the_keypress() {
        let _lock = guard_test_lock();
        // Alt+b's press report split after the escape: `ESC [98;3u`
        // (modifier value 3 = 1 + alt bit). The guard must synthesize the
        // Alt+b press the user made.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let delivered = feed_guard_split_tail(&mut guard, escape(), b"[98;3u", now);
        assert_eq!(
            delivered,
            vec![Event::Key(KeyEvent::new(
                KeyCode::Char('b'),
                KeyModifiers::ALT
            ))]
        );
    }

    #[test]
    fn split_escape_release_report_synthesizes_escape_press() {
        let _lock = guard_test_lock();
        // A real Esc keypress whose release report (`ESC [27;1:3u`) split:
        // the held press is replaced by the reconstructed one.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let delivered = feed_guard_split_tail(&mut guard, escape(), b"[27;1:3u", now);
        assert_eq!(delivered, vec![escape()]);
    }

    #[test]
    fn split_mouse_report_reconstructs_the_click() {
        let _lock = guard_test_lock();
        // A split SGR click report (`ESC [ < 0 ; 13 ; 35 M`): crossterm
        // decodes cb 0 as a plain left-button press at cell 13;35.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        let delivered = feed_guard_split_tail(&mut guard, escape(), b"[<0;13;35M", now);
        assert_eq!(
            delivered,
            vec![Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 12,
                row: 34,
                modifiers: KeyModifiers::NONE,
            })]
        );
    }

    #[test]
    fn human_typing_after_escape_is_replayed_in_order() {
        let _lock = guard_test_lock();
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        // A human presses Escape then types `x`, slower than a burst.
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        let outcome = guard.push(press('x'), now + Duration::from_millis(120));
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        assert!(matches!(outcome, GuardOutcome::Pass(_)));
        let delivered = match outcome {
            GuardOutcome::Pass(events) => events,
            _ => unreachable!(),
        };
        assert_eq!(delivered, vec![escape(), press('x')]);
    }

    #[test]
    fn lone_escape_replays_on_expiry() {
        let _lock = guard_test_lock();
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        assert!(guard.tick_expire(now).is_none());
        let replayed = guard.tick_expire(now + CSI_TAIL_HOLD_TIMEOUT).unwrap();
        assert_eq!(replayed, vec![escape()]);
        assert_eq!(guard.phase, CsiTailPhase::Idle);
    }

    #[test]
    fn incomplete_tail_replays_rather_than_dropping() {
        let _lock = guard_test_lock();
        // A grammar prefix that never completes (a human typed `[1` after a
        // paste, or a truncated report): replay everything held.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        for (i, ch) in "[1".chars().enumerate() {
            let at = now + Duration::from_millis(1 + i as u64);
            assert!(matches!(guard.push(press(ch), at), GuardOutcome::Held));
        }
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        let replayed = guard.tick_expire(now + Duration::from_millis(200)).unwrap();
        assert_eq!(replayed, vec![escape(), press('['), press('1')]);
    }

    #[test]
    fn split_osc_head_stands_down_to_the_report_reassembler() {
        let _lock = guard_test_lock();
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        let outcome = guard.push(press(']'), now + Duration::from_millis(1));
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        let delivered = match outcome {
            GuardOutcome::Pass(events) => events,
            other => panic!("expected pass, got {other:?}"),
        };
        assert_eq!(delivered, vec![escape(), press(']')]);
        assert_eq!(guard.phase, CsiTailPhase::Idle);
    }

    #[test]
    fn guard_stays_idle_without_a_kitty_push() {
        let _lock = guard_test_lock();
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        let outcome = guard.push(escape(), now);
        assert!(matches!(outcome, GuardOutcome::Pass(_)));
        let outcome = guard.push(press('['), now + Duration::from_millis(1));
        assert!(matches!(outcome, GuardOutcome::Pass(_)));
        assert_eq!(guard.phase, CsiTailPhase::Idle);
    }

    #[test]
    fn pastes_and_non_key_events_pass_through_mid_watch() {
        let _lock = guard_test_lock();
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        let paste = Event::Paste("[98;1:3u".into());
        let outcome = guard.push(paste.clone(), now + Duration::from_millis(1));
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        let delivered = match outcome {
            GuardOutcome::Pass(events) => events,
            other => panic!("expected pass, got {other:?}"),
        };
        assert_eq!(delivered, vec![escape(), paste]);
    }

    #[test]
    fn slow_tail_bytes_are_not_report_noise() {
        let _lock = guard_test_lock();
        // The same tail spread over 100ms per byte is a fast human, not a
        // terminal burst: nothing may be held.
        let mut guard = CsiTailGuard::default();
        let now = Instant::now();
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(true, Ordering::Relaxed);
        assert!(matches!(guard.push(escape(), now), GuardOutcome::Held));
        let outcome = guard.push(press('['), now + Duration::from_millis(60));
        super::super::KITTY_KEYBOARD_PUSH_ACTIVE.store(false, Ordering::Relaxed);
        let delivered = match outcome {
            GuardOutcome::Pass(events) => events,
            other => panic!("expected pass, got {other:?}"),
        };
        assert_eq!(delivered, vec![escape(), press('[')]);
    }

    fn report(filter: &mut ColorReports, body: &str, bel: bool, now: Instant) {
        filter.push(key(']', KeyModifiers::ALT), now);
        for ch in body.chars() {
            filter.push(key(ch, KeyModifiers::NONE), now);
        }
        filter.push(
            if bel {
                key('g', KeyModifiers::CONTROL)
            } else {
                key('\\', KeyModifiers::ALT)
            },
            now,
        );
    }

    #[test]
    fn osc_reports_switch_both_directions_without_becoming_input() {
        let mut filter = ColorReports::default();
        let now = Instant::now();
        report(&mut filter, "11;rgb:ffff/ffff/ffff", false, now);
        assert_eq!(filter.mode.take(), Some(ThemeMode::Light));
        assert!(filter.ready.is_empty());
        report(&mut filter, "11;rgb:00/00/00", true, now);
        assert_eq!(filter.mode.take(), Some(ThemeMode::Dark));
        assert!(filter.ready.is_empty());
    }

    #[test]
    fn ordinary_alt_chords_and_pastes_survive() {
        let mut filter = ColorReports::default();
        let now = Instant::now();
        let events = [
            key(']', KeyModifiers::ALT),
            key('x', KeyModifiers::NONE),
            Event::Paste("11;rgb:ff/ff/ff".into()),
        ];
        for event in &events {
            filter.push(event.clone(), now);
        }
        assert_eq!(filter.ready.into_iter().collect::<Vec<_>>(), events);
        assert_eq!(filter.mode, None);
    }

    #[test]
    fn missing_split_escape_recovers_without_losing_next_key() {
        let now = Instant::now();
        let mut filter = ColorReports {
            body: "11;rgb:".into(),
            started: Some(now),
            split_escape: true,
            ..ColorReports::default()
        };
        let input = key('x', KeyModifiers::NONE);
        filter.push(input.clone(), now);
        assert_eq!(filter.ready.into_iter().collect::<Vec<_>>(), vec![input]);
        assert_eq!(filter.mode, None);
    }

    #[test]
    fn fragmented_report_is_held_until_terminator() {
        let mut filter = ColorReports::default();
        let now = Instant::now();
        filter.push(key(']', KeyModifiers::ALT), now);
        for (i, ch) in "11;rgb:ffff/ffff/ffff".chars().enumerate() {
            filter.push(
                key(ch, KeyModifiers::NONE),
                now + Duration::from_millis(i as u64 * 10),
            );
            assert!(filter.ready.is_empty());
            assert_eq!(filter.mode, None);
        }
        filter.push(
            key('\\', KeyModifiers::ALT),
            now + Duration::from_millis(230),
        );
        assert_eq!(filter.mode, Some(ThemeMode::Light));
    }

    #[test]
    fn reassembles_split_escape_introducer_and_terminator() {
        let now = Instant::now();
        let mut filter = ColorReports {
            expecting_until: Some(now + REPORT_TIMEOUT),
            ..ColorReports::default()
        };
        let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        filter.push(escape.clone(), now);
        filter.push(
            key(']', KeyModifiers::NONE),
            now + Duration::from_millis(20),
        );
        for ch in "11;rgb:ffff/ffff/ffff".chars() {
            filter.push(key(ch, KeyModifiers::NONE), now + Duration::from_millis(40));
        }
        filter.push(escape, now + Duration::from_millis(60));
        filter.push(
            key('\\', KeyModifiers::NONE),
            now + Duration::from_millis(80),
        );
        assert_eq!(filter.mode, Some(ThemeMode::Light));
        assert!(filter.ready.is_empty());
        assert!(filter.expecting_until.is_none());
    }

    #[test]
    fn genuine_escape_is_replayed_and_is_not_delayed_outside_a_query() {
        let now = Instant::now();
        let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let mut filter = ColorReports::default();
        filter.push(escape.clone(), now);
        assert_eq!(filter.ready.pop_front(), Some(escape.clone()));
        filter.expecting_until = Some(now + REPORT_TIMEOUT);
        filter.push(escape.clone(), now);
        filter.expire(now + PREFIX_TIMEOUT);
        assert_eq!(filter.ready.pop_front(), Some(escape.clone()));
        filter.push(escape.clone(), now);
        filter.push(
            key('x', KeyModifiers::NONE),
            now + Duration::from_millis(20),
        );
        assert_eq!(filter.ready.pop_front(), Some(escape));
        assert_eq!(filter.ready.pop_front(), Some(key('x', KeyModifiers::NONE)));
    }

    #[test]
    fn timeout_replays_unrecognized_chord_but_drops_partial_report() {
        let now = Instant::now();
        let mut filter = ColorReports::default();
        filter.push(key(']', KeyModifiers::ALT), now);
        filter.expire(now + PREFIX_TIMEOUT);
        assert_eq!(filter.ready.pop_front(), Some(key(']', KeyModifiers::ALT)));
        filter.push(key(']', KeyModifiers::ALT), now);
        for ch in "11;rgb:ff/".chars() {
            filter.push(key(ch, KeyModifiers::NONE), now);
        }
        filter.expire(now + REPORT_TIMEOUT);
        assert!(filter.ready.is_empty());
        filter.push(key('x', KeyModifiers::NONE), now + REPORT_TIMEOUT);
        assert_eq!(filter.ready.pop_front(), Some(key('x', KeyModifiers::NONE)));
    }

    #[test]
    fn validates_color_components_and_ignores_alpha() {
        assert_eq!(
            parse_background_report("11;rgba:ffff/ffff/ffff/0000"),
            Some(ThemeMode::Light)
        );
        assert_eq!(
            parse_background_report("11;rgb:f/f/f"),
            Some(ThemeMode::Light)
        );
        for body in [
            "10;rgb:ff/ff/ff",
            "11;rgb:/ff/ff",
            "11;rgb:fffff/ff/ff",
            "11;rgb:gg/ff/ff",
            "11;rgb:ff/ff",
            "11;rgb:ff/ff/ff/ff",
        ] {
            assert_eq!(parse_background_report(body), None, "{body}");
        }
    }
}

#[cfg(all(test, unix))]
#[path = "terminal_events_pty_tests.rs"]
mod pty_tests;
