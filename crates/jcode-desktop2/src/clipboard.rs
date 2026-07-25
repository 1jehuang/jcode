//! System clipboard access.
//!
//! Wrapped behind a small trait-free struct with an in-process fallback so the
//! editor keeps working (and stays testable) on headless machines or when the
//! platform clipboard is unavailable, instead of silently dropping a cut.

/// Clipboard handle. Falls back to an internal buffer when the system
/// clipboard cannot be opened, so cut/paste never loses text.
pub struct Clipboard {
    backend: Option<arboard::Clipboard>,
    /// Last value we set, used as a fallback and to keep cut/paste coherent
    /// when the platform clipboard is unavailable.
    local: Option<String>,
    /// Whether to consult the platform clipboard at all.
    system: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            backend: None,
            local: None,
            // Tests must never read or overwrite the developer's real
            // clipboard, so the in-process fallback is used there.
            system: !cfg!(test),
        }
    }
}

impl Clipboard {
    fn backend(&mut self) -> Option<&mut arboard::Clipboard> {
        if !self.system {
            return None;
        }
        if self.backend.is_none() {
            self.backend = arboard::Clipboard::new().ok();
        }
        self.backend.as_mut()
    }

    /// Copy `text` to the clipboard. Empty text is ignored so a no-op cut does
    /// not wipe the user's clipboard.
    pub fn set(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.local = Some(text.to_string());
        if let Some(backend) = self.backend() {
            let _ = backend.set_text(text.to_string());
        }
    }

    /// Read the clipboard, preferring the system and falling back to the last
    /// value set in-process.
    pub fn get(&mut self) -> Option<String> {
        if let Some(backend) = self.backend()
            && let Ok(text) = backend.get_text()
            && !text.is_empty()
        {
            return Some(text);
        }
        self.local.clone().filter(|text| !text.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_text() {
        // Exercises the fallback path, which guarantees cut/paste coherence on
        // headless machines and in CI.
        let mut clipboard = Clipboard::default();
        clipboard.set("hello world");
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
        clipboard.set("keep me");
        clipboard.set("");
        assert_eq!(clipboard.get().as_deref(), Some("keep me"));
    }
}
