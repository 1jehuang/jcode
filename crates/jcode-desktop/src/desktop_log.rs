use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_LOG_MESSAGE_CHARS: usize = 8 * 1024;

static DESKTOP_LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

pub(crate) fn init() {
    let _ = log_file();
}

pub(crate) fn info(args: fmt::Arguments<'_>) {
    write("INFO", args, false);
}

pub(crate) fn warn(args: fmt::Arguments<'_>) {
    write("WARN", args, true);
}

pub(crate) fn error(args: fmt::Arguments<'_>) {
    write("ERROR", args, true);
}

pub(crate) fn truncate_for_log(value: &str, max_chars: usize) -> String {
    let sanitized = value
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    truncate_chars(&sanitized, max_chars)
}

fn write(level: &str, args: fmt::Arguments<'_>, mirror_to_stderr: bool) {
    let message = truncate_for_log(&args.to_string(), DEFAULT_MAX_LOG_MESSAGE_CHARS);
    if mirror_to_stderr {
        eprintln!("{message}");
    }

    let timestamp_ms = unix_timestamp_ms();
    let line = format!("[{timestamp_ms}] [{level}] {message}\n");
    let log_file = log_file();
    let mut guard = match log_file.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    if let Err(error) = file.write_all(line.as_bytes()) {
        eprintln!("jcode-desktop: failed to write desktop log: {error}");
        *guard = None;
        return;
    }
    if let Err(error) = file.flush() {
        eprintln!("jcode-desktop: failed to flush desktop log: {error}");
        *guard = None;
    }
}

fn log_file() -> &'static Mutex<Option<File>> {
    DESKTOP_LOG_FILE.get_or_init(|| Mutex::new(open_log_file()))
}

fn open_log_file() -> Option<File> {
    let path = desktop_log_path()?;
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!(
            "jcode-desktop: failed to create desktop log directory {}: {error}",
            parent.display()
        );
        return None;
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!(
                "jcode-desktop: failed to open desktop log {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn desktop_log_path() -> Option<PathBuf> {
    if std::env::var_os("JCODE_DESKTOP_LOG").is_some_and(|value| !env_flag_enabled(value)) {
        return None;
    }
    if let Some(path) = std::env::var_os("JCODE_DESKTOP_LOG_PATH") {
        if path.is_empty() {
            return None;
        }
        return Some(PathBuf::from(path));
    }

    let root = std::env::var_os("JCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".jcode")))?;
    Some(
        root.join("logs")
            .join(format!("jcode-desktop-{}.log", utc_date_label())),
    )
}

fn env_flag_enabled(value: std::ffi::OsString) -> bool {
    let value = value.to_string_lossy();
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "off" | "no"
    )
}

fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn utc_date_label() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let (year, month, day) = civil_from_unix_days(seconds.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

// Howard Hinnant's civil-from-days algorithm. The input is whole days since the
// Unix epoch and the output is a proleptic Gregorian UTC date.
fn civil_from_unix_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * mp + 2).div_euclid(5) + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut end = value.len();
    let mut count = 0usize;
    for (index, ch) in value.char_indices() {
        if count == max_chars {
            end = index;
            break;
        }
        count += 1;
        end = index + ch.len_utf8();
    }
    if count <= max_chars && end == value.len() {
        return value.to_string();
    }
    format!("{}… <truncated>", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_from_unix_days_handles_known_dates() {
        assert_eq!(civil_from_unix_days(0), (1970, 1, 1));
        assert_eq!(civil_from_unix_days(20_862), (2027, 2, 13));
    }

    #[test]
    fn truncate_for_log_escapes_control_characters() {
        assert_eq!(truncate_for_log("a\nb\tc", 16), "a\\nb\\tc");
        assert!(truncate_for_log("abcdef", 3).contains("abc… <truncated>"));
    }
}
