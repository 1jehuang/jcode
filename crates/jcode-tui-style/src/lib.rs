pub mod color;
pub mod harmony;
pub mod palette;
pub mod theme;
pub mod theme_mode;

pub use color::{ColorCapability, clear_buf, color_capability, has_truecolor, indexed_to_rgb, rgb};
pub use harmony::{Criterion, HarmonyReport, Oklab, analyze as analyze_harmony, analyze_active};
pub use palette::{ALL_ROLES, Palette, Role, palette, role_color, set_palette};
pub use theme_mode::{
    ThemeMode, adapt_buffer, adapt_buffer_for_display, adapt_buffer_for_theme,
    adapt_color_for_theme, adapt_foreground_for_display, adapt_foreground_for_theme,
    is_light_theme, set_theme_mode, theme_mode,
};

/// Final buffer pass shared by every render loop (`ui::draw`, the standalone
/// session picker, swarm replay). Fills `Color::Reset` backgrounds with the
/// theme background when the theme defines one ("dark" on a white terminal,
/// for instance), then runs the terminal-adaptation pass or the configured
/// palette pass. Custom TOML themes opt out of terminal adaptation so their
/// hand-picked colors are not luminance-flipped, while `[display.colors]`
/// overrides still apply through the palette pass.
pub fn finish_frame(buf: &mut ratatui::buffer::Buffer) {
    use ratatui::style::Color;

    let bg = theme::background_color();
    if bg != Color::Reset {
        for cell in buf.content.iter_mut() {
            if cell.bg == Color::Reset {
                cell.bg = bg;
            }
        }
    }
    if theme::active_theme_uses_terminal_adaptation() {
        adapt_buffer_for_display(buf);
    } else {
        palette::adapt_buffer_for_palette(buf);
    }
}

/// The active palette and theme mode are process-global. One lock serializes
/// every test that reads or mutates them, wherever the test lives (palette and
/// theme-mode tests used to carry separate locks and could interleave).
#[cfg(test)]
pub(crate) static STYLE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restore the terminal, logging any failure instead of printing it.
///
/// `ratatui::restore()` reports failures with `eprintln!`. On a dead terminal
/// (closed window, dropped SSH) the restore fails with EIO and stderr is
/// equally dead, so that `eprintln!` itself panics inside
/// `std::io::stdio::print_to`. The panic hook can then record a live session as
/// crashed and clobber its snapshot.
///
/// Always prefer this over `ratatui::restore()` on cleanup and orphan-exit
/// paths. See issue #599 (and #129, the same class via another path).
pub fn restore_terminal_quietly() {
    if let Err(error) = ratatui::try_restore() {
        jcode_logging::warn(&format!("failed to restore terminal: {error}"));
    }
}

thread_local! {
    /// Nesting depth of `catch_recoverable_panic` scopes on this thread.
    static RECOVERABLE_PANIC_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Run `f` and catch a panic that the caller recovers from and keeps running.
///
/// The panic hook runs before unwinding starts, so it cannot tell a fatal
/// panic from one a `catch_unwind` is about to absorb. Without this scope the
/// hook restored the terminal for a draw panic the TUI then recovered from:
/// jcode kept running on the primary screen in cooked mode with mouse
/// reporting still on, so the pane filled with escape garbage and ignored
/// keys. A tmux pane squeezed to one row was enough to trigger it.
pub fn catch_recoverable_panic<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            RECOVERABLE_PANIC_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
    RECOVERABLE_PANIC_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let _scope = Scope;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
}

/// Whether a panic raised on this thread right now will be recovered.
pub fn panic_will_be_recovered() -> bool {
    RECOVERABLE_PANIC_DEPTH.with(|depth| depth.get() > 0)
}

fn install_panic_hook_with(restore: fn()) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if panic_will_be_recovered() {
            // The TUI is still live on the alternate screen. Restoring the
            // terminal or printing a backtrace here would corrupt it.
            jcode_logging::error(&format!("recovered panic: {info}"));
            return;
        }
        restore();
        previous(info);
    }));
}

/// Enter raw mode and the alternate screen, like `ratatui::init`.
///
/// Always use this instead of `ratatui::init`, whose panic hook restores the
/// terminal on every panic, including recovered ones (see
/// [`catch_recoverable_panic`]). The hook is installed once per process, so
/// re-entering the TUI after an editor handoff does not stack hooks.
pub fn init_terminal() -> std::io::Result<ratatui::DefaultTerminal> {
    use ratatui::crossterm::{execute, terminal};

    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| install_panic_hook_with(restore_terminal_quietly));

    terminal::enable_raw_mode()?;
    execute!(std::io::stdout(), terminal::EnterAlternateScreen)?;
    ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))
}

#[cfg(test)]
mod panic_hook_tests {
    use super::*;

    thread_local! {
        static RESTORES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    fn count_restore() {
        RESTORES.with(|restores| restores.set(restores.get() + 1));
    }

    #[test]
    fn recovered_panics_do_not_restore_the_terminal() {
        // The hook runs on the panicking thread, so a thread-local counter
        // isolates this test from panics in tests running in parallel.
        install_panic_hook_with(count_restore);

        assert!(!panic_will_be_recovered());
        let recovered = catch_recoverable_panic(|| {
            assert!(panic_will_be_recovered());
            panic!("draw outside buffer");
        });
        assert!(recovered.is_err());
        assert!(!panic_will_be_recovered(), "scope must end on unwind");
        assert_eq!(RESTORES.with(std::cell::Cell::get), 0);

        assert_eq!(catch_recoverable_panic(|| 7).ok(), Some(7));
        assert!(!panic_will_be_recovered());

        let fatal = std::panic::catch_unwind(|| panic!("fatal"));
        assert!(fatal.is_err());
        assert_eq!(RESTORES.with(std::cell::Cell::get), 1);
    }
}
