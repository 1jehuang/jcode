//! Counts why the client event loop woke up, so idle wakeups can be traced to
//! their source (`client:draw-stats` reports them as `wake_sources`).
use std::collections::BTreeMap;
use std::sync::Mutex;

static COUNTS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

pub(crate) fn note(source: &str) {
    if let Ok(mut counts) = COUNTS.lock() {
        if let Some(n) = counts.get_mut(source) {
            *n += 1;
        } else {
            counts.insert(source.to_string(), 1);
        }
    }
}

/// Variant name of a Debug-printable enum value. Formatting stops at the first
/// non-identifier character, so large payloads (History) cost nothing extra.
pub(crate) fn note_debug<T: std::fmt::Debug>(prefix: &str, value: &T) {
    struct Name(String);
    impl std::fmt::Write for Name {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if !(c.is_alphanumeric() || c == '_') || self.0.len() >= 48 {
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut name = Name(String::new());
    let _ = std::fmt::write(&mut name, format_args!("{value:?}"));
    note(&format!("{prefix}:{}", name.0));
}

pub(crate) fn snapshot() -> serde_json::Value {
    let counts = COUNTS.lock().map(|c| c.clone()).unwrap_or_default();
    serde_json::to_value(counts).unwrap_or_default()
}
