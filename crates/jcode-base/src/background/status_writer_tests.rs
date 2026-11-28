use super::BackgroundTaskManager;
use crate::bus::BackgroundTaskStatus;
use anyhow::Result;
use tempfile::tempdir;

#[test]
fn adopted_task_status_writer_replaces_open_reader() -> Result<()> {
    use std::io::Read;

    let tmp = tempdir()?;
    let path = tmp.path().join("open-reader.status.json");
    let mut status = BackgroundTaskStatus::Running;
    BackgroundTaskManager::persist_status_file(&path, &status)?;
    let mut reader = std::fs::File::open(&path)?;

    status = BackgroundTaskStatus::Completed;
    BackgroundTaskManager::persist_status_file(&path, &status)?;

    let mut original = String::new();
    reader.read_to_string(&mut original)?;
    let original: BackgroundTaskStatus = serde_json::from_str(&original)?;
    assert_eq!(original, BackgroundTaskStatus::Running);
    let current: BackgroundTaskStatus = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(current, BackgroundTaskStatus::Completed);
    assert_eq!(std::fs::read_dir(tmp.path())?.count(), 1);
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        assert_eq!(std::fs::metadata(path)?.file_attributes() & 0x100, 0);
    }
    Ok(())
}

#[test]
fn adopted_task_status_writer_cleans_failed_replacement() -> Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("destination");
    std::fs::create_dir(&path)?;
    let status = BackgroundTaskStatus::Running;
    assert!(BackgroundTaskManager::persist_status_file(&path, &status).is_err());
    assert!(path.is_dir());
    assert_eq!(std::fs::read_dir(tmp.path())?.count(), 1);
    Ok(())
}
