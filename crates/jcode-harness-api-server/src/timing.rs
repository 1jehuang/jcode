//! Always-on, low-volume request timing for the API bridge.
//!
//! One line per client request when its reply is written, plus slow event
//! translations, appended to `$JCODE_HOME/logs/api-timing.log`. Bridges run
//! over SSH stdio have no useful stderr, so this goes to a file. Never logs
//! session content, only request kinds, ids, durations and byte counts.
use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Translations slower than this are logged even without a request.
pub const SLOW_TRANSLATE: Duration = Duration::from_millis(20);

fn path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("JCODE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".jcode")))?;
    Some(home.join("logs").join("api-timing.log"))
}

pub fn log(line: &str) {
    static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
    let Some(path) = path() else { return };
    let Ok(mut guard) = FILE.lock() else { return };
    if guard.is_none() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Keep one previous generation instead of growing forever.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
        *guard = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    }
    if let Some(file) = guard.as_mut() {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "{ms} pid={} {line}", std::process::id());
    }
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Requests awaiting their reply on one connection.
#[derive(Default)]
pub struct Pending {
    started: Instant_,
    inflight: HashMap<u64, (String, Instant, Duration)>,
}

struct Instant_(Instant);
impl Default for Instant_ {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Pending {
    pub fn connection_age(&self) -> Duration {
        self.started.0.elapsed()
    }

    /// Note a request; `translate` is the synchronous bridge time spent on it.
    pub fn begin(&mut self, id: u64, kind: &str, received: Instant, translate: Duration) {
        if self.inflight.len() > 1024 {
            self.inflight.clear();
        }
        self.inflight.insert(id, (kind.to_string(), received, translate));
    }

    /// A reply was written. `source` is "bridge" (answered locally) or
    /// "daemon" (came back over the legacy socket).
    pub fn finish(&mut self, reply_to: u64, event: &str, bytes: usize, source: &str) {
        if let Some((kind, received, translate)) = self.inflight.remove(&reply_to) {
            log(&format!(
                "req {kind} id={reply_to} total={:.1}ms translate={:.1}ms reply={event} bytes={bytes} via={source} conn_age={:.0}ms",
                ms(received.elapsed()),
                ms(translate),
                ms(self.connection_age()),
            ));
        }
    }
}
