//! Simple undo/redo manager using session message snapshots.
//! Snapshots stored at `~/.jcode/undo/<session_id>/`.

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

static UNDO_MANAGER: std::sync::LazyLock<Mutex<UndoManager>> =
    std::sync::LazyLock::new(|| Mutex::new(UndoManager::new()));

struct SessionUndoStack {
    undo_stack: Vec<Vec<u8>>,
    redo_stack: Vec<Vec<u8>>,
    max_depth: usize,
}

impl SessionUndoStack {
    fn new(max_depth: usize) -> Self {
        Self { undo_stack: Vec::new(), redo_stack: Vec::new(), max_depth }
    }
}

pub struct UndoManager {
    sessions: HashMap<String, SessionUndoStack>,
    undo_dir: PathBuf,
}

impl UndoManager {
    fn new() -> Self {
        let dir = crate::storage::jcode_dir()
            .map(|d| d.join("undo"))
            .unwrap_or_else(|_| PathBuf::from("./.jcode/undo"));
        Self { sessions: HashMap::new(), undo_dir: dir }
    }

    fn ensure_session(&mut self, session_id: &str, max_depth: usize) -> &mut SessionUndoStack {
        self.sessions.entry(session_id.to_string())
            .or_insert_with(|| SessionUndoStack::new(max_depth))
    }

    pub fn save_checkpoint(session_id: &str, data: Vec<u8>) {
        if let Ok(mut mgr) = UNDO_MANAGER.lock() {
            let undo_dir = mgr.undo_dir.clone();
            let snapshot_data;
            let stack_len;
            {
                let stack = mgr.ensure_session(session_id, 20);
                stack.redo_stack.clear();
                stack.undo_stack.push(data.clone());
                while stack.undo_stack.len() > stack.max_depth {
                    stack.undo_stack.remove(0);
                }
                let idx = stack.undo_stack.len();
                stack_len = idx;
                snapshot_data = stack.undo_stack[idx - 1].clone();
            }
            let _ = std::fs::create_dir_all(&undo_dir);
            let _ = std::fs::write(undo_dir.join(format!("{}.snap", snapshot_data.len())), &data);
        }
    }

    pub fn undo(session_id: &str) -> Option<Vec<u8>> {
        let mut mgr = UNDO_MANAGER.lock().ok()?;
        let stack = mgr.ensure_session(session_id, 20);
        let state = stack.undo_stack.pop()?;
        stack.redo_stack.push(state);
        stack.undo_stack.last().cloned()
    }

    pub fn redo(session_id: &str) -> Option<Vec<u8>> {
        let mut mgr = UNDO_MANAGER.lock().ok()?;
        let stack = mgr.ensure_session(session_id, 20);
        let state = stack.redo_stack.pop()?;
        stack.undo_stack.push(state.clone());
        Some(state)
    }

    pub fn can_undo(session_id: &str) -> bool {
        UNDO_MANAGER.lock().ok().map_or(false, |mut m| m.ensure_session(session_id, 20).undo_stack.len() > 1)
    }

    pub fn can_redo(session_id: &str) -> bool {
        UNDO_MANAGER.lock().ok().map_or(false, |mut m| !m.ensure_session(session_id, 20).redo_stack.is_empty())
    }

    pub fn snapshot_session(session_id: &str) -> Result<()> {
        if let Ok(session) = crate::session::Session::load(session_id) {
            let data = serde_json::to_vec(&session.messages)?;
            Self::save_checkpoint(session_id, data);
        }
        Ok(())
    }
}
