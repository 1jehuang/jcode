//! Atomic publication of background task status files.
//!
//! `bg status`/`bg wait`, the reconciliation sweeps, and other jcode processes
//! read these paths while the owning task keeps rewriting them, so every write
//! goes through a sibling temp file plus a rename.

use super::*;

/// Write a task status file so concurrent readers never observe a torn file.
///
/// A plain truncating write is observable as an empty or partial file, and a
/// reader that hits that window reports the task as missing (`wait` then returns
/// `None` instead of a result). Writing a sibling temp file and renaming it into
/// place makes every read see either the previous file or the complete new one.
/// A failed publish can leave one uniquely named temp file behind; the reconcile
/// sweep and `bg cleanup` both key off the `.json` extension, so it is inert.
pub(super) fn write_status_file_atomic(
    path: &std::path::Path,
    contents: &str,
) -> std::io::Result<()> {
    static WRITE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = WRITE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    std::fs::write(&temp, contents)?;
    #[cfg(test)]
    super::status_write_tests::before_status_file_rename(path, temp.clone());
    std::fs::rename(&temp, path)
}

/// Publish one serialized status file while holding the status gate, keeping the
/// owned guard so the caller can extend the same critical section.
pub(super) async fn publish_status_file(
    status_updates: &Arc<Mutex<()>>,
    guard: OwnedMutexGuard<()>,
    path: &std::path::Path,
    status: &TaskStatusFile,
) -> OwnedMutexGuard<()> {
    let Ok(json) = serde_json::to_string_pretty(status) else {
        crate::logging::warn("background status serialization failed");
        return guard;
    };
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let result = write_status_file_atomic(&path, &json);
        (guard, result)
    })
    .await
    {
        Ok((guard, result)) => {
            if let Err(error) = result {
                crate::logging::warn(&format!("background status write failed: {error}"));
            }
            guard
        }
        Err(error) => {
            crate::logging::warn(&format!("background status writer failed: {error}"));
            status_updates.clone().lock_owned().await
        }
    }
}
