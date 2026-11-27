//! Remote stderr is untrusted. Drain it without retaining content or trusting labels.
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_LINE: usize = 192;
const SCAN_BUDGET: usize = 64 * 1024;
const RECORD_BUDGET: usize = 128;

fn timing_record(line: &[u8]) -> Option<String> {
    let Ok(text) = std::str::from_utf8(line) else {
        return None;
    };
    let rest = text.strip_prefix("[CLOUD_TIMING] stage=")?;
    let (stage, rest) = rest.split_once(" outcome=")?;
    let stage = match stage {
        "projection" => "projection",
        "remote_request" => "remote_request",
        "inventory_entry" => "inventory_entry",
        "remote_preview" => "remote_preview",
        _ => return None,
    };
    let (outcome, duration) = rest.split_once(" duration_ms=")?;
    let outcome = match outcome {
        "success" => "success",
        "error" => "error",
        "skipped_invalid_target" => "skipped_invalid_target",
        "skipped_away" => "skipped_away",
        _ => return None,
    };
    if duration.is_empty() || !duration.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let Ok(duration) = duration.parse::<u64>() else {
        return None;
    };
    Some(format!(
        "[CLOUD_TIMING] stage={stage} outcome={outcome} duration_ms={duration}"
    ))
}

pub(super) async fn drain(reader: impl AsyncRead + Unpin, mut emit: impl FnMut(String)) {
    let mut reader = reader;
    let mut chunk = [0u8; 4096];
    let mut line = Vec::with_capacity(MAX_LINE);
    let mut oversized = false;
    let mut scanned = 0usize;
    let mut emitted = 0usize;
    loop {
        let Ok(count) = reader.read(&mut chunk).await else {
            return;
        };
        if count == 0 {
            return;
        }
        for &byte in &chunk[..count] {
            if scanned >= SCAN_BUDGET || emitted >= RECORD_BUDGET {
                // Still drain the pipe, otherwise a noisy child can deadlock stdout.
                break;
            }
            scanned += 1;
            if byte == b'\n' {
                if !oversized && let Some(record) = timing_record(&line) {
                    emit(record);
                    emitted += 1;
                }
                line.clear();
                oversized = false;
            } else if line.len() < MAX_LINE && !oversized {
                line.push(byte);
            } else {
                line.clear();
                oversized = true;
            }
        }
    }
}

/// Unlike kill_on_drop, this also kills pipe-holding descendants on cancellation.
pub(super) struct ProcessGroup(pub(super) Option<u32>);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
#[path = "cloud_git_stderr_tests.rs"]
mod tests;
