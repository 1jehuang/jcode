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
    struct Name(String, bool);
    impl std::fmt::Write for Name {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if !(c.is_alphanumeric() || c == '_') || self.0.len() >= 48 {
                    self.1 = true;
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut name = Name(String::new(), false);
    // Our writer uses an error to stop after the variant name. Other formatter
    // failures are unexpected and should remain visible in diagnostics.
    if std::fmt::write(&mut name, format_args!("{value:?}")).is_err() && !name.1 {
        crate::logging::debug("Failed to format wake source name");
    }
    note(&format!("{prefix}:{}", name.0));
}

pub(crate) fn snapshot() -> serde_json::Value {
    let counts = match COUNTS.lock() {
        Ok(counts) => counts.clone(),
        Err(error) => {
            crate::logging::warn(&format!("Wake counters lock poisoned: {error}"));
            BTreeMap::new()
        }
    };
    match serde_json::to_value(counts) {
        Ok(value) => value,
        Err(error) => {
            crate::logging::warn(&format!("Failed to serialize wake counters: {error}"));
            serde_json::Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_counter_stops_at_variant_name() {
        super::note_debug("wake-test", &Some("payload"));
        assert!(
            super::snapshot()["wake-test:Some"]
                .as_u64()
                .is_some_and(|n| n > 0)
        );
        super::note_debug("wake-test", &None::<()>);
        assert!(
            super::snapshot()["wake-test:None"]
                .as_u64()
                .is_some_and(|n| n > 0)
        );
    }
}
