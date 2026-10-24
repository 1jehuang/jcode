use super::*;

static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(super) struct StoreGuard {
    _thread: std::sync::MutexGuard<'static, ()>,
    _file: std::fs::File,
}

/// Lock both client/server processes and threads before reading or mutating inventory.
pub(super) fn store_lock() -> Result<StoreGuard> {
    let thread = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = crate::storage::jcode_dir()?.join("side_panel");
    crate::storage::ensure_dir(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".lock"))?;
    file.lock()?;
    Ok(StoreGuard {
        _thread: thread,
        _file: file,
    })
}
