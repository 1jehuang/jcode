//! Runtime terminal-background queries using the existing crossterm reader.
//!
//! Never run colorsaurus once an event reader exists: two stdin readers race
//! and a color report can become composer input. Crossterm 0.29 decodes OSC
//! reports as Alt+] / ordinary characters / Alt+\\ (or Ctrl+G). Intercept them
//! before *any* picker, shortcut, or composer sees them. The async reader and
//! synchronous burst drains share this filter, just like crossterm's reader.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
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
const PREFIX_TIMEOUT: Duration = Duration::from_millis(250);
const REPORT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPORT_LEN: usize = 40;

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

    fn expire(&mut self, now: Instant) {
        let timeout = if self.recognized() {
            REPORT_TIMEOUT
        } else {
            PREFIX_TIMEOUT
        };
        if self
            .started
            .is_some_and(|started| now.duration_since(started) >= timeout)
        {
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
                let escape = self.held.pop().expect("held split Escape");
                self.reset(false);
                self.ready.push_back(escape);
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
        let value = u32::from_str_radix(component, 16).ok()?;
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

#[derive(Default)]
struct RuntimeTheme {
    reports: ColorReports,
    last_query: Option<Instant>,
    cache_query_at: Option<Instant>,
}

fn runtime() -> &'static Mutex<RuntimeTheme> {
    static RUNTIME: OnceLock<Mutex<RuntimeTheme>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(RuntimeTheme::default()))
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
        let term = std::env::var("TERM").unwrap_or_default();
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
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    })
}

fn query_if_due(state: &mut RuntimeTheme, now: Instant, focused: bool) {
    if !super::theme_detect::runtime_queries_enabled() {
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
        QUERY_INTERVAL
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
    state.reports.ready.pop_front()
}

/// Nonblocking burst drain with the same report filter as [`EventStream`].
/// Returning None means there is currently no user event, even if report bytes
/// were consumed. The async reader will deliver any remaining report/replay.
pub(super) fn try_read() -> io::Result<Option<Event>> {
    let mut state = runtime().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(event) = take_event(&mut state) {
        return Ok(Some(event));
    }
    for _ in 0..64 {
        if !crossterm::event::poll(Duration::ZERO)? {
            return Ok(None);
        }
        let event = crossterm::event::read()?;
        let now = Instant::now();
        if event == Event::FocusGained {
            query_if_due(&mut state, now, true);
        }
        state.reports.push(event, now);
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
}

impl EventStream {
    pub(super) fn new() -> Self {
        Self {
            inner: crossterm::event::EventStream::new(),
            tick: Box::pin(tokio::time::sleep(Duration::ZERO)),
        }
    }
}

impl Stream for EventStream {
    type Item = io::Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = runtime().lock().unwrap_or_else(|error| error.into_inner());
        // Poll the timer first so a continuously-ready stream of input cannot
        // starve a theme refresh. It only writes a tiny query every five seconds.
        if self.tick.as_mut().poll(cx).is_ready() {
            let now = Instant::now();
            state.reports.expire(now);
            query_if_due(&mut state, now, false);
            self.tick
                .as_mut()
                .reset(tokio::time::Instant::now() + PREFIX_TIMEOUT);
            let _ = self.tick.as_mut().poll(cx);
        }
        if let Some(event) = take_event(&mut state) {
            return Poll::Ready(Some(Ok(event)));
        }
        // Bound the work per poll while draining the key-shaped color report.
        for _ in 0..64 {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    let now = Instant::now();
                    if event == Event::FocusGained {
                        query_if_due(&mut state, now, true);
                    }
                    state.reports.push(event, now);
                    if let Some(event) = take_event(&mut state) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
                other => return other,
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
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

    fn key(ch: char, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(ch), modifiers))
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
