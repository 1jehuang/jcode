//! Per-path locks for file-editing tools.
//!
//! `batch` runs subcalls concurrently, so two edits to the same file could
//! both read the original and then race their writes, losing one edit and
//! leaving a torn tail. Every read-modify-write tool holds the lock for each
//! path it touches for the whole read-modify-write cycle.

pub(crate) use crate::file_history::{lock, lock_all};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_edits_to_one_file_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "").unwrap();
        let tasks = (0..32).map(|index| {
            let path = path.clone();
            tokio::spawn(async move {
                let _guard = lock(&path).await;
                let mut content = tokio::fs::read_to_string(&path).await.unwrap();
                tokio::task::yield_now().await;
                content.push_str(&format!("{index}\n"));
                tokio::fs::write(&path, content).await.unwrap();
            })
        });
        for task in tasks.collect::<Vec<_>>() {
            task.await.unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 32);
    }
}
