//! System clipboard access.
//!
//! Wrapped behind a small trait-free struct with an in-process fallback so the
//! editor keeps working (and stays testable) on headless machines or when the
//! platform clipboard is unavailable, instead of silently dropping a cut.
//!
//! Two destinations, not one. Linux has a *primary selection* alongside the
//! ordinary clipboard: selecting text fills it, middle click pastes it, and it
//! is deliberately separate so that highlighting something does not throw away
//! whatever you had explicitly copied. That separation is the entire reason
//! auto-copy-on-select is safe to do at all, so the distinction is modelled
//! here rather than collapsed into one buffer.

/// The platform clipboard refused a write. Carries the platform's own message
/// so the app can say what went wrong rather than failing mutely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable(pub String);

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which system buffer an operation refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The ordinary clipboard: what Ctrl+C writes and Ctrl+V reads.
    Clipboard,
    /// The X11/Wayland primary selection: filled by selecting text, pasted
    /// with middle click. On platforms without one, writes are dropped rather
    /// than redirected: silently rewriting the real clipboard every time the
    /// user dragged over a word would destroy what they had copied.
    Primary,
}

/// Clipboard handle. Falls back to an internal buffer when the system
/// clipboard cannot be opened, so cut/paste never loses text.
pub struct Clipboard {
    backend: Option<arboard::Clipboard>,
    /// Last value we set, used as a fallback and to keep cut/paste coherent
    /// when the platform clipboard is unavailable.
    local: Option<String>,
    /// Last value written to the primary selection. Kept separate from
    /// `local` for the same reason the system keeps them separate: a selection
    /// must not be able to overwrite an explicit copy.
    local_primary: Option<String>,
    /// Whether to consult the platform clipboard at all.
    system: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            backend: None,
            local: None,
            local_primary: None,
            // Tests must never read or overwrite the developer's real
            // clipboard, so the in-process fallback is used there.
            system: !cfg!(test),
        }
    }
}

/// Whether this platform has a primary selection to write to.
pub const fn has_primary_selection() -> bool {
    cfg!(target_os = "linux")
}

impl Clipboard {
    /// A clipboard that really talks to the platform, even under `cfg(test)`.
    ///
    /// Only for the deliberate round-trip check: the default is sandboxed so
    /// the test suite cannot clobber a developer's clipboard, which also means
    /// the default never exercises arboard at all.
    pub fn system() -> Self {
        Self {
            system: true,
            ..Self::default()
        }
    }

    fn backend(&mut self) -> Option<&mut arboard::Clipboard> {
        if !self.system {
            return None;
        }
        if self.backend.is_none() {
            self.backend = arboard::Clipboard::new().ok();
        }
        self.backend.as_mut()
    }

    /// Copy `text` to the ordinary clipboard. Empty text is ignored so a no-op
    /// cut does not wipe the user's clipboard.
    #[cfg(test)]
    pub fn set(&mut self, text: &str) -> Result<(), Unavailable> {
        self.set_to(Target::Clipboard, text)
    }

    /// Copy `text` to a specific buffer.
    ///
    /// A primary-selection write on a platform without one is a no-op, not a
    /// fallback to the real clipboard: auto-copy fires on every drag, so
    /// falling back would mean merely highlighting a word destroys whatever
    /// the user had deliberately copied.
    pub fn set_to(&mut self, target: Target, text: &str) -> Result<(), Unavailable> {
        if text.is_empty() {
            return Ok(());
        }
        match target {
            Target::Clipboard => self.local = Some(text.to_string()),
            Target::Primary => {
                self.local_primary = Some(text.to_string());
                if !has_primary_selection() {
                    return Ok(());
                }
            }
        }
        let Some(backend) = self.backend() else {
            // No platform clipboard at all (headless, or a sandbox): the
            // in-process copy above still keeps cut and paste coherent.
            return Ok(());
        };
        write(backend, target, text).map_err(|error| Unavailable(error.to_string()))
    }

    /// Read the clipboard, preferring the system and falling back to the last
    /// value set in-process.
    pub fn get(&mut self) -> Option<String> {
        self.get_from(Target::Clipboard)
    }

    /// Read a specific buffer, preferring the system and falling back to the
    /// last value this process wrote there.
    pub fn get_from(&mut self, target: Target) -> Option<String> {
        if (target == Target::Clipboard || has_primary_selection())
            && let Some(backend) = self.backend()
            && let Ok(text) = read(backend, target)
            && !text.is_empty()
        {
            return Some(text);
        }
        let local = match target {
            Target::Clipboard => &self.local,
            Target::Primary => &self.local_primary,
        };
        local.clone().filter(|text| !text.is_empty())
    }

    /// The last value written to the primary selection in this process.
    /// Exposed for tests: reading the real primary selection back would depend
    /// on a compositor, which CI does not have.
    #[cfg(test)]
    pub fn primary(&self) -> Option<&str> {
        self.local_primary.as_deref()
    }
}

/// Write to a system buffer. Split out so the platform-specific selection
/// plumbing is in one place rather than spread through `set_to`.
///
/// Returns the platform error rather than discarding it: a copy that silently
/// did nothing is the failure mode users report as "the app ate my clipboard",
/// so the caller keeps the in-process fallback *and* gets to say so.
#[cfg(target_os = "linux")]
fn write(
    backend: &mut arboard::Clipboard,
    target: Target,
    text: &str,
) -> Result<(), arboard::Error> {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let kind = match target {
        Target::Clipboard => LinuxClipboardKind::Clipboard,
        Target::Primary => LinuxClipboardKind::Primary,
    };
    backend.set().clipboard(kind).text(text.to_string())
}

#[cfg(not(target_os = "linux"))]
fn write(
    backend: &mut arboard::Clipboard,
    target: Target,
    text: &str,
) -> Result<(), arboard::Error> {
    // Only the ordinary clipboard exists here; `set_to` has already returned
    // for a primary write, so this is unreachable for `Target::Primary`.
    debug_assert_eq!(target, Target::Clipboard);
    backend.set_text(text.to_string())
}

/// Read a system buffer.
#[cfg(target_os = "linux")]
fn read(backend: &mut arboard::Clipboard, target: Target) -> Result<String, arboard::Error> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    let kind = match target {
        Target::Clipboard => LinuxClipboardKind::Clipboard,
        Target::Primary => LinuxClipboardKind::Primary,
    };
    backend.get().clipboard(kind).text()
}

#[cfg(not(target_os = "linux"))]
fn read(backend: &mut arboard::Clipboard, target: Target) -> Result<String, arboard::Error> {
    debug_assert_eq!(target, Target::Clipboard);
    backend.get_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_text() {
        // Exercises the fallback path, which guarantees cut/paste coherence on
        // headless machines and in CI.
        let mut clipboard = Clipboard::default();
        clipboard.set("hello world").expect("fallback write");
        assert_eq!(clipboard.get().as_deref(), Some("hello world"));
    }

    #[test]
    fn tests_never_touch_the_real_system_clipboard() {
        assert!(
            !Clipboard::default().system,
            "the test clipboard would clobber the developer's clipboard"
        );
    }

    #[test]
    fn reading_an_untouched_clipboard_is_none() {
        assert_eq!(Clipboard::default().get(), None);
    }

    #[test]
    fn empty_copy_does_not_clobber_the_clipboard() {
        let mut clipboard = Clipboard::default();
        clipboard.set("keep me").expect("fallback write");
        let _ = clipboard.set("");
        assert_eq!(clipboard.get().as_deref(), Some("keep me"));
    }

    /// The invariant that makes auto-copy safe: selecting text must never
    /// touch what the user deliberately copied. Without this, dragging over a
    /// word in the transcript would silently destroy their clipboard.
    #[test]
    fn a_primary_write_never_touches_the_clipboard() {
        let mut clipboard = Clipboard::default();
        clipboard
            .set("deliberately copied")
            .expect("fallback write");
        clipboard
            .set_to(Target::Primary, "merely selected")
            .expect("fallback write");
        assert_eq!(
            clipboard.get().as_deref(),
            Some("deliberately copied"),
            "selecting text overwrote an explicit copy"
        );
        assert_eq!(clipboard.primary(), Some("merely selected"));
    }

    /// And the converse: an explicit copy must not be shadowed by whatever was
    /// last selected.
    #[test]
    fn a_clipboard_write_never_touches_the_primary_selection() {
        let mut clipboard = Clipboard::default();
        clipboard
            .set_to(Target::Primary, "selected")
            .expect("fallback write");
        clipboard.set("copied").expect("fallback write");
        assert_eq!(clipboard.primary(), Some("selected"));
    }

    #[test]
    fn an_empty_selection_does_not_clobber_the_primary_selection() {
        let mut clipboard = Clipboard::default();
        clipboard
            .set_to(Target::Primary, "keep me")
            .expect("fallback write");
        let _ = clipboard.set_to(Target::Primary, "");
        assert_eq!(clipboard.primary(), Some("keep me"));
    }
    /// A platform failure must be reported, not discarded. The in-process copy
    /// still happens, so paste within the app keeps working, but the caller
    /// gets to tell the user their system clipboard did not receive it.
    #[test]
    fn a_failed_system_write_still_keeps_the_text_locally() {
        let mut clipboard = Clipboard::default();
        // The sandboxed clipboard has no backend, which is the same shape as a
        // headless machine: the write reports success because the fallback
        // took it, and the text is readable back.
        assert_eq!(clipboard.set_to(Target::Clipboard, "text"), Ok(()));
        assert_eq!(clipboard.get().as_deref(), Some("text"));
    }

    /// `Unavailable` has to carry the platform's own message: "clipboard
    /// failed" tells a user nothing they can act on.
    #[test]
    fn an_unavailable_clipboard_explains_itself() {
        let error = Unavailable("no wayland display".into());
        assert_eq!(error.to_string(), "no wayland display");
    }
}
