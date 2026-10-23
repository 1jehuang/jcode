//! Durable, tool-scoped file checkpoints. Never snapshots or resets a worktree.
//!
//! Before-images and intended outcomes are durable before a tracked mutation.
//! Restores compare bytes, existence and permissions, and refuse external changes.
use anyhow::{Context, Result, ensure};
use jcode_session_types::RewindCheckpoint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[path = "file_history_conversation.rs"]
mod conversation;

const MAX_CHECKPOINTS: usize = 100;
static HISTORY_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_FINALIZE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileState {
    blob: Option<String>,
    mode: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    info: RewindCheckpoint,
    before: BTreeMap<PathBuf, FileState>,
    #[serde(default)]
    blocked: BTreeSet<PathBuf>,
    #[serde(default)]
    boundary_message_id: Option<String>,
    #[serde(default)]
    input_message_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ConversationBoundary {
    message_index: usize,
    boundary_message_id: Option<String>,
}

type PathLocks = Mutex<BTreeMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>;

/// Shared with the editing tools, including batch subcalls and local clients.
pub async fn lock(path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: OnceLock<PathLocks> = OnceLock::new();
    let mutex = {
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, mutex| Arc::strong_count(mutex) > 1);
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        locks.entry(key).or_default().clone()
    };
    mutex.lock_owned().await
}

pub async fn lock_all(
    paths: impl IntoIterator<Item = PathBuf>,
) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
    let mut paths: Vec<_> = paths
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect();
    paths.sort();
    paths.dedup();
    let mut guards = Vec::with_capacity(paths.len());
    for path in paths {
        guards.push(lock(&path).await);
    }
    guards
}

#[derive(Clone, Serialize, Deserialize)]
struct Change {
    path: PathBuf,
    before: FileState,
    after: FileState,
}

#[derive(Clone, Serialize, Deserialize)]
struct ConversationCommit {
    session_id: String,
    message_index: usize,
    boundary_message_id: Option<String>,
}

fn conversation_saved(commit: &ConversationCommit) -> bool {
    crate::session::Session::load(&commit.session_id).is_ok_and(|session| {
        session.messages.len() == commit.message_index
            && session.messages.last().map(|message| message.id.clone())
                == commit.boundary_message_id
            && session.provider_session_id.is_none()
            && session.compaction.is_none()
    })
}

#[derive(Default, Serialize, Deserialize)]
struct History {
    checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    conversation_boundaries: BTreeMap<String, ConversationBoundary>,
    expected: BTreeMap<PathBuf, FileState>,
    #[serde(default)]
    pending: Option<Change>,
    #[serde(default)]
    restoring: Vec<Change>,
    #[serde(default)]
    conversation_commit: Option<ConversationCommit>,
}

fn directory(session_id: &str) -> Result<PathBuf> {
    // Hash the ID rather than allowing a session identifier to escape the sidecar.
    let key = hex::encode(Sha256::digest(session_id.as_bytes()));
    Ok(crate::storage::jcode_dir()?
        .join("sessions/file-history")
        .join(key))
}

fn save(dir: &Path, history: &History) -> Result<()> {
    #[cfg(test)]
    if history.restoring.is_empty() && FAIL_NEXT_FINALIZE.with(|flag| flag.replace(false)) {
        anyhow::bail!("injected checkpoint finalization failure");
    }
    crate::storage::write_json_secret(&dir.join("history.json"), history)
}

/// Resolve the nearest existing parent to a physical identity. Parent aliases
/// such as macOS /var are harmless; the destination itself must be a regular
/// non-aliased file so restoration cannot overwrite a symlink target.
fn safe_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute.parent().context("File has no parent")?;
    let name = absolute.file_name().context("File has no name")?;
    fn physical_parent(path: &Path) -> Result<PathBuf> {
        match fs::canonicalize(path) {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = path.parent().context("Missing file parent")?;
                Ok(physical_parent(parent)?.join(path.file_name().context("Invalid parent")?))
            }
            Err(error) => Err(error.into()),
        }
    }
    let path = physical_parent(parent)?.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            ensure!(
                !meta.file_type().is_symlink(),
                "Cannot checkpoint symlink: {}",
                path.display()
            );
            ensure!(
                meta.is_file(),
                "Cannot checkpoint non-regular file: {}",
                path.display()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    meta.nlink() == 1,
                    "Cannot checkpoint hardlinked file: {}",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

fn aliased_target(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        if metadata.file_type().is_symlink() {
            return true;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.is_file() && metadata.nlink() > 1 {
                return true;
            }
        }
        false
    })
}

fn state(dir: &Path, path: &Path) -> Result<FileState> {
    ensure!(
        safe_path(path)? == path,
        "Tracked path identity changed: {}",
        path.display()
    );
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileState {
                blob: None,
                mode: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let bytes = fs::read(path).with_context(|| format!("Capturing {}", path.display()))?;
    let blob = store_blob(dir, &bytes)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let mode = Some(u32::from(metadata.permissions().readonly()));
    Ok(FileState {
        blob: Some(blob),
        mode,
    })
}

fn store_blob(dir: &Path, bytes: &[u8]) -> Result<String> {
    let hash = hex::encode(Sha256::digest(bytes));
    let path = dir.join("blobs").join(&hash);
    if !path.exists() {
        crate::storage::ensure_dir(path.parent().unwrap())?;
        crate::storage::write_bytes(&path, bytes)?;
        crate::storage::harden_secret_file_permissions(&path);
    }
    Ok(hash)
}

fn bytes(dir: &Path, state: &FileState) -> Result<Option<Vec<u8>>> {
    let Some(hash) = &state.blob else {
        return Ok(None);
    };
    let data =
        fs::read(dir.join("blobs").join(hash)).context("Missing checkpoint file contents")?;
    ensure!(
        hex::encode(Sha256::digest(&data)) == *hash,
        "Corrupt checkpoint file contents"
    );
    Ok(Some(data))
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn apply(dir: &Path, change: &Change, reverse: bool) -> Result<()> {
    let (expected, desired) = if reverse {
        (&change.after, &change.before)
    } else {
        (&change.before, &change.after)
    };
    ensure!(
        state(dir, &change.path)? == *expected,
        "File changed outside tracked tools: {}",
        change.path.display()
    );
    let Some(data) = bytes(dir, desired)? else {
        if expected.blob.is_some() {
            fs::remove_file(&change.path)?;
            sync_parent(&change.path)?;
        }
        return Ok(());
    };
    let parent = change.path.parent().context("File has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".jcode-rewind-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&data)?;
        #[cfg(unix)]
        if let Some(mode) = desired.mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        if let Some(mode) = desired.mode {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(mode != 0);
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        // Recheck after staging, rather than overwriting a newer external edit.
        ensure!(
            state(dir, &change.path)? == *expected,
            "File changed during restore: {}",
            change.path.display()
        );
        fs::rename(&temporary, &change.path)?;
        sync_parent(&change.path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn load(dir: &Path) -> Result<History> {
    let path = dir.join("history.json");
    let mut history: History = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid file checkpoint history")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => History::default(),
        Err(error) => return Err(error.into()),
    };
    if let Some(pending) = history.pending.clone() {
        let actual = state(dir, &pending.path)?;
        let completed_new_file = pending.before.blob.is_none() && actual.blob == pending.after.blob;
        ensure!(
            actual == pending.before || actual == pending.after || completed_new_file,
            "Interrupted file mutation conflicts with external changes: {}",
            pending.path.display()
        );
        history.expected.insert(pending.path, actual);
        history.pending = None;
        save(dir, &history)?;
    }
    if !history.restoring.is_empty() {
        if history
            .conversation_commit
            .as_ref()
            .is_some_and(conversation_saved)
        {
            for change in &history.restoring {
                ensure!(
                    state(dir, &change.path)? == change.after,
                    "Committed rewind conflicts with external changes: {}",
                    change.path.display()
                );
                history
                    .expected
                    .insert(change.path.clone(), change.after.clone());
            }
            history.restoring.clear();
            if let Some(commit) = &history.conversation_commit {
                let boundary = commit.message_index;
                conversation::retain_before(&mut history, boundary);
            }
            history.conversation_commit = None;
            save(dir, &history)?;
            return Ok(history);
        }
        // Crash recovery rolls files back to the pre-restore state. Transcript
        // persistence is owned by Agent and must not observe half a restore.
        for change in history.restoring.iter().rev() {
            let actual = state(dir, &change.path)?;
            if actual == change.before {
                continue;
            }
            ensure!(
                actual == change.after,
                "Interrupted restore conflicts with external changes: {}",
                change.path.display()
            );
            apply(dir, change, true)?;
        }
        history.restoring.clear();
        history.conversation_commit = None;
        save(dir, &history)?;
    }
    Ok(history)
}

fn note_external(history: &mut History, path: &Path, actual: &FileState) {
    if history
        .expected
        .get(path)
        .is_some_and(|expected| expected != actual)
    {
        for checkpoint in &mut history.checkpoints {
            checkpoint.blocked.insert(path.to_owned());
        }
    }
}

pub fn checkpoint(
    session_id: &str,
    message_index: usize,
    prompt: &str,
) -> Result<RewindCheckpoint> {
    checkpoint_with_anchor(session_id, message_index, prompt, None)
}

fn checkpoint_with_anchor(
    session_id: &str,
    message_index: usize,
    prompt: &str,
    boundary_message_id: Option<String>,
) -> Result<RewindCheckpoint> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let mut history = load(&dir)?;
    let mut before = BTreeMap::new();
    for path in history.expected.keys().cloned().collect::<Vec<_>>() {
        let actual = match state(&dir, &path) {
            Ok(actual) => actual,
            Err(error) => {
                for checkpoint in &mut history.checkpoints {
                    checkpoint.blocked.insert(path.clone());
                }
                history.expected.remove(&path);
                crate::logging::warn(&format!(
                    "File checkpoints disabled for {}: {error:#}",
                    path.display()
                ));
                continue;
            }
        };
        note_external(&mut history, &path, &actual);
        history.expected.insert(path.clone(), actual.clone());
        before.insert(path, actual);
    }
    let info = RewindCheckpoint {
        id: uuid::Uuid::new_v4().to_string(),
        message_index,
        prompt: prompt.to_owned(),
        timestamp: Some(chrono::Utc::now().to_rfc3339()),
        can_restore_files: true,
        changed_file_count: 0,
    };
    history.checkpoints.push(Checkpoint {
        info: info.clone(),
        before,
        blocked: BTreeSet::new(),
        boundary_message_id,
        input_message_id: None,
    });
    let evicted = history.checkpoints.len() > MAX_CHECKPOINTS;
    if evicted {
        let checkpoint = history.checkpoints.remove(0);
        // Also migrate older sidecars that predate independent boundaries.
        if let Some(input_id) = checkpoint.input_message_id {
            history
                .conversation_boundaries
                .entry(input_id)
                .or_insert(ConversationBoundary {
                    message_index: checkpoint.info.message_index,
                    boundary_message_id: checkpoint.boundary_message_id,
                });
        }
    }
    save(&dir, &history)?;
    if evicted {
        collect_blobs_best_effort(&dir, &history);
    }
    Ok(info)
}

pub fn checkpoints(session_id: &str) -> Result<Vec<RewindCheckpoint>> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let history = load(&dir)?;
    Ok(history
        .checkpoints
        .iter()
        .map(|checkpoint| conversation::checkpoint_info(&history, checkpoint))
        .collect())
}

/// Paths are exposed for the caller to take the same locks as editing tools.
/// Restore still performs a complete fresh preflight under the history lock.
pub fn restore_paths(session_id: &str, checkpoint_id: &str) -> Result<Vec<PathBuf>> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let history = load(&directory(session_id)?)?;
    let checkpoint = history
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.info.id == checkpoint_id)
        .context("Checkpoint not found")?;
    Ok(checkpoint.before.keys().cloned().collect())
}

pub fn restore(session_id: &str, checkpoint_id: &str) -> Result<usize> {
    restore_with_commit(session_id, checkpoint_id, None, || Ok(()))
}

fn restore_with_commit(
    session_id: &str,
    checkpoint_id: &str,
    conversation_commit: Option<ConversationCommit>,
    commit: impl FnOnce() -> Result<()>,
) -> Result<usize> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let mut history = load(&dir)?;
    let checkpoint = history
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.info.id == checkpoint_id)
        .context("Checkpoint not found")?;
    ensure!(
        checkpoint.blocked.is_empty(),
        "Checkpoint conflicts with external changes: {}",
        checkpoint
            .blocked
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut changes = Vec::new();
    for (path, desired) in &checkpoint.before {
        let actual = state(&dir, path)?;
        if &actual == desired {
            continue;
        }
        ensure!(
            history.expected.get(path) == Some(&actual),
            "File changed outside tracked tools: {}",
            path.display()
        );
        // Validate every backup before touching any file.
        bytes(&dir, desired)?;
        if &actual != desired {
            changes.push(Change {
                path: path.clone(),
                before: actual,
                after: desired.clone(),
            });
        }
    }
    history.restoring = changes.clone();
    history.conversation_commit = conversation_commit;
    save(&dir, &history)?;
    for change in &changes {
        if let Err(error) = apply(&dir, change, false) {
            // load() performs conflict-checked rollback, retaining the transaction
            // on disk if an external writer prevents safe recovery.
            let rollback = load(&dir);
            return match rollback {
                Ok(_) => Err(error),
                Err(rollback) => {
                    Err(error.context(format!("Restore rollback requires attention: {rollback:#}")))
                }
            };
        }
    }
    if let Err(error) = commit() {
        if history
            .conversation_commit
            .as_ref()
            .is_some_and(conversation_saved)
        {
            // Completing the pending transcript marker can recover a transient
            // snapshot/journal write failure. Finalize the same durable commit.
            load(&dir)?;
            return Ok(changes.len());
        }
        if history.conversation_commit.as_ref().is_some_and(|commit| {
            crate::session::Session::pending_rewind_exists(&commit.session_id)
        }) {
            return Err(error
                .context("Rewind commit is durable but requires recovery before further changes"));
        }
        // A failed save must not make recovery infer a successful transcript
        // commit from an earlier, coincidentally identical boundary.
        history.conversation_commit = None;
        save(&dir, &history)?;
        let rollback = load(&dir);
        return match rollback {
            Ok(_) => Err(error),
            Err(rollback) => {
                Err(error.context(format!("Restore rollback requires attention: {rollback:#}")))
            }
        };
    }
    for change in &changes {
        history
            .expected
            .insert(change.path.clone(), change.after.clone());
    }
    if let Some(commit) = &history.conversation_commit {
        let boundary = commit.message_index;
        conversation::retain_before(&mut history, boundary);
    }
    history.restoring.clear();
    history.conversation_commit = None;
    save(&dir, &history)?;
    Ok(changes.len())
}

impl crate::session::Session {
    pub fn create_rewind_checkpoint(&mut self, prompt: &str) -> Result<RewindCheckpoint> {
        checkpoint_with_anchor(
            &self.id,
            self.messages.len(),
            prompt,
            self.messages.last().map(|message| message.id.clone()),
        )
    }

    pub fn rewind_checkpoints(&self) -> std::result::Result<Vec<RewindCheckpoint>, String> {
        conversation::checkpoints(self).map_err(|error| format!("{error:#}"))
    }

    pub fn bind_rewind_checkpoint(&mut self, checkpoint_id: &str, message_id: &str) -> Result<()> {
        let _guard = HISTORY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let dir = directory(&self.id)?;
        let mut history = load(&dir)?;
        let checkpoint = history
            .checkpoints
            .iter_mut()
            .find(|checkpoint| checkpoint.info.id == checkpoint_id)
            .context("Checkpoint not found")?;
        ensure!(
            self.messages
                .iter()
                .skip(checkpoint.info.message_index)
                .any(|message| message.id == message_id
                    && crate::session::is_authentic_user_prompt(message)),
            "Checkpoint input is not an authentic user message"
        );
        checkpoint.input_message_id = Some(message_id.to_owned());
        let boundary = ConversationBoundary {
            message_index: checkpoint.info.message_index,
            boundary_message_id: checkpoint.boundary_message_id.clone(),
        };
        history
            .conversation_boundaries
            .insert(message_id.to_owned(), boundary);
        save(&dir, &history)
    }

    pub async fn rewind_checkpoint(
        &mut self,
        checkpoint_id: &str,
        mode: jcode_session_types::RewindMode,
    ) -> std::result::Result<jcode_session_types::RewindResult, String> {
        self.rewind_checkpoint_inner(checkpoint_id, mode)
            .await
            .map_err(|error| format!("{error:#}"))
    }

    async fn rewind_checkpoint_inner(
        &mut self,
        checkpoint_id: &str,
        mode: jcode_session_types::RewindMode,
    ) -> Result<jcode_session_types::RewindResult> {
        let (checkpoint, images) = conversation::resolve_checkpoint(self, checkpoint_id, mode)?;
        let boundary = checkpoint.info.message_index;
        ensure!(
            reachable(self, &checkpoint),
            "Checkpoint belongs to a removed conversation branch"
        );
        if mode.restores_conversation() {
            ensure!(
                boundary <= self.messages.len(),
                "Checkpoint belongs to a removed conversation branch"
            );
            let anchor = boundary
                .checked_sub(1)
                .and_then(|index| self.messages.get(index))
                .map(|message| message.id.clone());
            ensure!(
                anchor == checkpoint.boundary_message_id,
                "Checkpoint belongs to a different conversation branch"
            );
            ensure!(
                self.migration_lease_block().is_none(),
                "Session is owned by another machine"
            );
        }
        let paths = if mode.restores_files() {
            restore_paths(&self.id, &checkpoint.info.id)?
        } else {
            Vec::new()
        };
        let _locks = lock_all(paths).await;
        let previous = self.clone();
        let removed_messages = if mode.restores_conversation() {
            self.messages.len() - boundary
        } else {
            0
        };
        let session_id = self.id.clone();
        let conversation_commit = mode.restores_conversation().then(|| ConversationCommit {
            session_id: session_id.clone(),
            message_index: boundary,
            boundary_message_id: checkpoint.boundary_message_id.clone(),
        });
        let mut persist = || -> Result<()> {
            if mode.restores_conversation() {
                self.truncate_messages(boundary);
                self.compaction = None;
                self.provider_session_id = None;
                self.model_usage_turn_id = None;
                self.save_rewind()?;
            }
            Ok(())
        };
        let result = if mode.restores_files() {
            restore_with_commit(
                &session_id,
                &checkpoint.info.id,
                conversation_commit,
                persist,
            )
        } else {
            persist().and_then(|()| {
                let _guard = HISTORY_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let dir = directory(&session_id)?;
                let mut history = load(&dir)?;
                conversation::retain_before(&mut history, boundary);
                save(&dir, &history)?;
                Ok(0)
            })
        };
        match result {
            Ok(restored_files) => Ok(jcode_session_types::RewindResult {
                mode,
                prompt: checkpoint.info.prompt,
                images,
                removed_messages,
                restored_files,
                skipped_files: Vec::new(),
            }),
            Err(error) => {
                // Once a durable transcript commit exists, reverting only the
                // live Session would split memory from disk. Leave the target
                // state installed and retain the recovery marker on failure.
                let committed = mode.restores_conversation()
                    && (self.has_pending_rewind()
                        || conversation_saved(&ConversationCommit {
                            session_id: self.id.clone(),
                            message_index: boundary,
                            boundary_message_id: checkpoint.boundary_message_id.clone(),
                        }));
                if !committed {
                    *self = previous;
                }
                Err(error)
            }
        }
    }
}

fn reachable(session: &crate::session::Session, checkpoint: &Checkpoint) -> bool {
    let boundary = checkpoint.info.message_index;
    let anchor = boundary
        .checked_sub(1)
        .and_then(|index| session.messages.get(index))
        .map(|message| &message.id);
    boundary < session.messages.len()
        && anchor == checkpoint.boundary_message_id.as_ref()
        && checkpoint.input_message_id.as_ref().is_some_and(|id| {
            session.messages.iter().skip(boundary).any(|message| {
                &message.id == id && crate::session::is_authentic_user_prompt(message)
            })
        })
}

fn mutate(session_id: &str, path: &Path, content: Option<&[u8]>) -> Result<()> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let mut history = load(&dir)?;
    if history.checkpoints.is_empty() {
        return match content {
            Some(bytes) => fs::write(path, bytes).map_err(Into::into),
            None => fs::remove_file(path).map_err(Into::into),
        };
    }
    if aliased_target(path) {
        // Existing tools deliberately permit links. Preserve that behavior,
        // but never advertise an incomplete checkpoint as safely restorable.
        for checkpoint in &mut history.checkpoints {
            checkpoint.blocked.insert(path.to_owned());
        }
        save(&dir, &history)?;
        return match content {
            Some(bytes) => fs::write(path, bytes).map_err(Into::into),
            None => fs::remove_file(path).map_err(Into::into),
        };
    }
    let path = safe_path(path)?;
    let before = state(&dir, &path)?;
    // Atomic rename must not grant editing rights that an ordinary write-open
    // would deny (read-only mode, ACLs). This open never truncates or writes.
    if content.is_some() && before.blob.is_some() {
        OpenOptions::new().write(true).open(&path)?;
    }
    let after = FileState {
        blob: content.map(|bytes| store_blob(&dir, bytes)).transpose()?,
        mode: if content.is_some() {
            Some(match before.mode {
                Some(mode) => mode,
                None => new_file_mode(&dir)?,
            })
        } else {
            None
        },
    };
    note_external(&mut history, &path, &before);
    for checkpoint in &mut history.checkpoints {
        checkpoint
            .before
            .entry(path.clone())
            .or_insert_with(|| before.clone());
    }
    let change = Change {
        path: path.clone(),
        before,
        after,
    };
    history.pending = Some(change.clone());
    save(&dir, &history)?;
    // Prepared regular-file writes replace atomically: death during staging
    // leaves the original intact, never a torn in-place truncation.
    let result = apply(&dir, &change, false);
    let actual = state(&dir, &path)?;
    if (result.is_ok() && actual != change.after)
        || (actual != change.before && actual != change.after)
    {
        for checkpoint in &mut history.checkpoints {
            checkpoint.blocked.insert(path.clone());
        }
    }
    history.expected.insert(path, actual);
    history.pending = None;
    save(&dir, &history)?;
    collect_blobs_best_effort(&dir, &history);
    result.map_err(Into::into)
}

fn collect_blobs_best_effort(dir: &Path, history: &History) {
    if let Err(error) = collect_blobs(dir, history) {
        crate::logging::warn(&format!("Checkpoint blob cleanup failed: {error:#}"));
    }
}

fn new_file_mode(dir: &Path) -> Result<u32> {
    // Observe the process umask without changing its process-global value.
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let probe = dir.join(format!(".mode-{}", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o666)
            .open(&probe)?;
        let mode = file.metadata()?.permissions().mode();
        fs::remove_file(probe)?;
        Ok(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(0)
    }
}

fn collect_blobs(dir: &Path, history: &History) -> Result<()> {
    let mut used = BTreeSet::new();
    for state in history.expected.values().chain(
        history
            .checkpoints
            .iter()
            .flat_map(|checkpoint| checkpoint.before.values()),
    ) {
        if let Some(blob) = &state.blob {
            used.insert(blob.clone());
        }
    }
    for change in history.pending.iter().chain(history.restoring.iter()) {
        used.extend(
            change
                .before
                .blob
                .iter()
                .chain(change.after.blob.iter())
                .cloned(),
        );
    }
    let entries = match fs::read_dir(dir.join("blobs")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !used.contains(&entry.file_name().to_string_lossy().into_owned())
            && entry.file_type()?.is_file()
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub async fn tracked_write(session_id: &str, path: &Path, content: &[u8]) -> Result<()> {
    let (session_id, path, content) = (session_id.to_owned(), path.to_owned(), content.to_owned());
    tokio::task::spawn_blocking(move || mutate(&session_id, &path, Some(&content))).await?
}

pub async fn tracked_remove(session_id: &str, path: &Path) -> Result<()> {
    let (session_id, path) = (session_id.to_owned(), path.to_owned());
    tokio::task::spawn_blocking(move || mutate(&session_id, &path, None)).await?
}

#[cfg(test)]
#[path = "file_history_tests.rs"]
mod tests;
