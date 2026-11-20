use crate::TodoItem;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoCancellation {
    /// Snapshot as it was immediately before cancellation.
    pub todo: TodoItem,
    pub reason: String,
    pub at: DateTime<Utc>,
}

impl TodoCancellation {
    pub fn new(todo: TodoItem, reason: String) -> Self {
        Self {
            todo,
            reason,
            at: Utc::now(),
        }
    }
}

fn directory(home: &Path) -> PathBuf {
    home.join("todos").join("cancellations")
}

fn path(home: &Path, session: &str) -> io::Result<PathBuf> {
    if !super::is_safe_session_id(session) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid todo session ID",
        ));
    }
    Ok(directory(home).join(format!("{session}.json")))
}

pub fn load_cancellations(home: &Path, session_id: &str) -> io::Result<Vec<TodoCancellation>> {
    match super::read_optional(&path(home, session_id)?)? {
        Some(history) => Ok(history),
        None => Ok(Vec::new()),
    }
}

/// Append the entire batch atomically. Reject unreadable history rather than replacing it.
pub fn append_cancellations(
    home: &Path,
    session_id: &str,
    entries: &[TodoCancellation],
) -> io::Result<()> {
    let path = path(home, session_id)?;
    if entries.is_empty() {
        return Ok(());
    }
    if entries.iter().any(|entry| entry.reason.trim().is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cancellation reason must not be blank",
        ));
    }
    fs::create_dir_all(directory(home))?;
    // A stable lock inode protects the read/modify/rename sequence across processes.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut history: Vec<TodoCancellation> = load_cancellations(home, session_id)?;
    history.extend_from_slice(entries);
    jcode_storage::write_json_secret(&path, &history).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_independent_atomic_and_session_scoped() {
        let home = std::env::temp_dir().join(format!(
            "todo-history-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let entry = |n: usize| {
            TodoCancellation::new(
                TodoItem {
                    id: format!("id-{n}"),
                    status: "in_progress".into(),
                    ..Default::default()
                },
                format!("reason {n}"),
            )
        };
        assert!(load_cancellations(&home, "s").unwrap().is_empty());
        assert!(append_cancellations(&home, "../s", &[entry(0)]).is_err());
        let first = entry(0);
        append_cancellations(&home, "s", std::slice::from_ref(&first)).unwrap();
        std::thread::scope(|scope| {
            for n in 1..5 {
                let home = &home;
                scope.spawn(move || append_cancellations(home, "s", &[entry(n)]).unwrap());
            }
        });
        let history = load_cancellations(&home, "s").unwrap();
        assert_eq!(history[0], first);
        assert_eq!(history.len(), 5);
        assert!(load_cancellations(&home, "other").unwrap().is_empty());
        assert!(super::super::load_snapshot(&home, "s").todos.is_empty());
        let path = path(&home, "s").unwrap();
        fs::write(&path, b"corrupt").unwrap();
        assert!(append_cancellations(&home, "s", &[entry(5)]).is_err());
        assert_eq!(fs::read(path).unwrap(), b"corrupt");
        fs::remove_dir_all(home).unwrap();
    }
}
