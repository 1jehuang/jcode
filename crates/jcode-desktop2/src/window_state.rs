//! Persisted window geometry.
//!
//! Reopening the app to a different size than you left it is a small but
//! constant annoyance, so size and position are remembered. Parsing and
//! validation are pure and separate from disk I/O, which keeps the sanity
//! rules (no zero-sized or off-screen windows) testable.

use std::path::PathBuf;

/// Default window size, used on first run and when a saved value is unusable.
pub const DEFAULT_SIZE: (f64, f64) = (1100.0, 720.0);
/// Smallest window we will restore. Anything smaller is treated as corrupt:
/// restoring it would produce an unusable window.
pub const MIN_SIZE: (f64, f64) = (480.0, 360.0);
/// Largest size we will restore, guarding against absurd saved values.
const MAX_SIZE: f64 = 30_000.0;

/// Minimum gap between writes, so dragging a window edge does not write the
/// file on every resize event.
pub const SAVE_THROTTLE: std::time::Duration = std::time::Duration::from_millis(750);

/// Remembered window geometry, in logical units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub width: f64,
    pub height: f64,
    /// Position, when the platform reported one.
    pub position: Option<(f64, f64)>,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            width: DEFAULT_SIZE.0,
            height: DEFAULT_SIZE.1,
            position: None,
        }
    }
}

impl Geometry {
    /// Clamp to something usable. A saved geometry that is tiny, inverted, or
    /// absurd falls back to the default rather than opening a broken window.
    pub fn sanitized(self) -> Self {
        let usable = |value: f64, min: f64, default: f64| {
            if value.is_finite() && (min..=MAX_SIZE).contains(&value) {
                value
            } else {
                default
            }
        };
        let position = self.position.filter(|(x, y)| {
            // Negative coordinates are legal on multi-monitor setups, but
            // non-finite or absurd ones are not.
            x.is_finite() && y.is_finite() && x.abs() <= MAX_SIZE && y.abs() <= MAX_SIZE
        });
        Self {
            width: usable(self.width, MIN_SIZE.0, DEFAULT_SIZE.0),
            height: usable(self.height, MIN_SIZE.1, DEFAULT_SIZE.1),
            position,
        }
    }

    /// Serialize as a tiny line-oriented format: no dependency, and a
    /// corrupt file degrades to defaults rather than failing to start.
    pub fn serialize(&self) -> String {
        let mut out = format!("width={}\nheight={}\n", self.width, self.height);
        if let Some((x, y)) = self.position {
            out.push_str(&format!("x={x}\ny={y}\n"));
        }
        out
    }

    /// Parse the format written by [`Self::serialize`]. Unknown keys and
    /// malformed lines are ignored.
    pub fn parse(text: &str) -> Self {
        let mut geometry = Self::default();
        let (mut x, mut y) = (None, None);
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<f64>() else {
                continue;
            };
            match key.trim() {
                "width" => geometry.width = value,
                "height" => geometry.height = value,
                "x" => x = Some(value),
                "y" => y = Some(value),
                _ => {}
            }
        }
        geometry.position = x.zip(y);
        geometry.sanitized()
    }

    /// Where the geometry is stored.
    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".jcode")
                .join("desktop2-window.conf"),
        )
    }

    /// Load the saved geometry, falling back to defaults. A missing file is
    /// normal on first run; any other failure is reported rather than hidden.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("window geometry: cannot read {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Whether this geometry should be written yet, given the last write time
    /// and what was last written. Pure, so the throttling rule is testable.
    pub fn should_save(
        &self,
        last_saved: Option<(std::time::Instant, Geometry)>,
        now: std::time::Instant,
    ) -> bool {
        match last_saved {
            // Unchanged geometry never needs rewriting.
            Some((_, previous)) if previous == self.sanitized() => false,
            Some((at, _)) => now.saturating_duration_since(at) >= SAVE_THROTTLE,
            None => true,
        }
    }

    /// Persist the geometry. A failure must never break the app or block exit,
    /// but it is reported: silently forgetting the window size looks like a bug
    /// with no way to diagnose it.
    pub fn save(&self) {
        if let Err(error) = self.try_save() {
            eprintln!("window geometry: not saved: {error}");
        }
    }

    fn try_save(&self) -> std::io::Result<()> {
        let path = Self::path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME to store geometry in")
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.sanitized().serialize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_saved_format() {
        let geometry = Geometry {
            width: 1234.0,
            height: 900.5,
            position: Some((42.0, -17.0)),
        };
        assert_eq!(Geometry::parse(&geometry.serialize()), geometry);
    }

    #[test]
    fn round_trips_without_a_position() {
        let geometry = Geometry {
            width: 800.0,
            height: 600.0,
            position: None,
        };
        assert_eq!(Geometry::parse(&geometry.serialize()), geometry);
    }

    #[test]
    fn a_missing_or_empty_file_gives_the_default_size() {
        let geometry = Geometry::parse("");
        assert_eq!((geometry.width, geometry.height), DEFAULT_SIZE);
        assert_eq!(geometry.position, None);
    }

    #[test]
    fn corrupt_content_never_panics_and_falls_back() {
        for text in [
            "garbage",
            "width=",
            "width=abc\nheight=def",
            "=5",
            "width=1\nheight=1",
            "\0\0\0",
            "width=NaN\nheight=NaN",
            "width=inf",
        ] {
            let geometry = Geometry::parse(text);
            assert!(
                geometry.width >= MIN_SIZE.0 && geometry.height >= MIN_SIZE.1,
                "parsed {text:?} into an unusable window {geometry:?}"
            );
        }
    }

    #[test]
    fn a_tiny_saved_window_is_rejected() {
        // Restoring a 1x1 window would leave the user with nothing to click.
        let geometry = Geometry {
            width: 4.0,
            height: 4.0,
            position: None,
        }
        .sanitized();
        assert_eq!((geometry.width, geometry.height), DEFAULT_SIZE);
    }

    #[test]
    fn an_absurd_saved_window_is_rejected() {
        let geometry = Geometry {
            width: 5_000_000.0,
            height: 900.0,
            position: None,
        }
        .sanitized();
        assert_eq!(geometry.width, DEFAULT_SIZE.0);
        assert_eq!(geometry.height, 900.0, "a valid dimension was discarded");
    }

    #[test]
    fn negative_positions_are_kept_for_multi_monitor_setups() {
        // A monitor left of the primary has negative logical coordinates.
        let geometry = Geometry {
            width: 1100.0,
            height: 720.0,
            position: Some((-1920.0, 0.0)),
        }
        .sanitized();
        assert_eq!(geometry.position, Some((-1920.0, 0.0)));
    }

    #[test]
    fn non_finite_positions_are_dropped() {
        let geometry = Geometry {
            width: 1100.0,
            height: 720.0,
            position: Some((f64::NAN, 0.0)),
        }
        .sanitized();
        assert_eq!(geometry.position, None);
    }

    #[test]
    fn a_half_written_position_is_ignored() {
        // Only x was persisted, so there is no usable position.
        let geometry = Geometry::parse("width=800\nheight=600\nx=10");
        assert_eq!(geometry.position, None);
        assert_eq!((geometry.width, geometry.height), (800.0, 600.0));
    }

    #[test]
    fn sanitizing_is_idempotent() {
        let geometry = Geometry {
            width: 3.0,
            height: 900.0,
            position: Some((f64::INFINITY, 1.0)),
        };
        let once = geometry.sanitized();
        assert_eq!(once, once.sanitized());
    }

    #[test]
    fn saving_reports_failure_instead_of_silently_dropping_it() {
        // A save that cannot happen must surface an error rather than leaving
        // the user wondering why the window size is never remembered.
        let previous = std::env::var_os("HOME");
        // SAFETY: single-threaded test; restored below.
        unsafe { std::env::remove_var("HOME") };
        let result = Geometry::default().try_save();
        if let Some(previous) = previous {
            unsafe { std::env::set_var("HOME", previous) };
        }
        assert!(
            result.is_err(),
            "a save with nowhere to write reported success"
        );
    }

    #[test]
    fn geometry_is_saved_promptly_the_first_time() {
        // Saving only on a clean exit loses the size if the app is killed.
        let geometry = Geometry::default();
        assert!(geometry.should_save(None, std::time::Instant::now()));
    }

    #[test]
    fn repeated_saves_are_throttled_while_dragging() {
        let now = std::time::Instant::now();
        let saved = Geometry {
            width: 900.0,
            height: 700.0,
            position: None,
        };
        let changed = Geometry {
            width: 901.0,
            ..saved
        };
        assert!(
            !changed.should_save(Some((now, saved)), now),
            "wrote the file on every resize event"
        );
        assert!(
            changed.should_save(Some((now, saved)), now + SAVE_THROTTLE),
            "never wrote after the throttle window"
        );
    }

    #[test]
    fn unchanged_geometry_is_not_rewritten() {
        let now = std::time::Instant::now();
        let geometry = Geometry {
            width: 900.0,
            height: 700.0,
            position: Some((5.0, 5.0)),
        };
        assert!(
            !geometry.should_save(Some((now - SAVE_THROTTLE * 10, geometry)), now),
            "rewrote an unchanged geometry"
        );
    }

    #[test]
    fn the_saved_path_lives_under_the_jcode_directory() {
        // Guards against writing config into the working directory.
        if let Some(path) = Geometry::path() {
            assert!(path.to_string_lossy().contains("/.jcode/"));
        }
    }
}
