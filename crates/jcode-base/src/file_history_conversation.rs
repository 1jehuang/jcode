//! Transcript targets are independent of optional, bounded file snapshots.
use super::*;
use crate::message::ContentBlock;
use crate::session::{
    Session, StoredMessage, is_attached_image_label_text, is_authentic_user_prompt,
};
use jcode_session_types::{RewindMode, RewindTargetKind};

const MESSAGE_TARGET_PREFIX: &str = "message:";
const ASSISTANT_TARGET_PREFIX: &str = "assistant:";

/// User targets remove their input; assistant targets keep their entire stored
/// message. An assistant boundary is never a fabricated human/file checkpoint.
pub(super) enum ResolvedTarget {
    UserPrompt {
        checkpoint: Checkpoint,
        images: Vec<(String, String)>,
    },
    AssistantMessage {
        info: RewindCheckpoint,
        boundary_message_id: String,
    },
}

impl ResolvedTarget {
    pub(super) fn info(&self) -> &RewindCheckpoint {
        match self {
            Self::UserPrompt { checkpoint, .. } => &checkpoint.info,
            Self::AssistantMessage { info, .. } => info,
        }
    }

    pub(super) fn boundary_message_id(&self) -> Option<&str> {
        match self {
            Self::UserPrompt { checkpoint, .. } => checkpoint.boundary_message_id.as_deref(),
            Self::AssistantMessage {
                boundary_message_id,
                ..
            } => Some(boundary_message_id),
        }
    }

    pub(super) fn into_result(
        self,
        mode: RewindMode,
        removed_messages: usize,
        restored_files: usize,
    ) -> jcode_session_types::RewindResult {
        let (target_kind, prompt, images) = match self {
            Self::UserPrompt { checkpoint, images } => {
                (RewindTargetKind::UserPrompt, checkpoint.info.prompt, images)
            }
            Self::AssistantMessage { .. } => (
                RewindTargetKind::AssistantMessage,
                String::new(),
                Vec::new(),
            ),
        };
        jcode_session_types::RewindResult {
            mode,
            target_kind,
            prompt,
            images,
            removed_messages,
            restored_files,
            skipped_files: Vec::new(),
        }
    }
}

fn assistant_info(message: &StoredMessage, keep_len: usize) -> RewindCheckpoint {
    RewindCheckpoint {
        id: format!("{ASSISTANT_TARGET_PREFIX}{}", message.id),
        target_kind: RewindTargetKind::AssistantMessage,
        message_index: keep_len,
        prompt: prompt_text(message),
        timestamp: message.timestamp.map(|timestamp| timestamp.to_rfc3339()),
        can_restore_files: false,
        changed_file_count: 0,
    }
}

fn prompt_text(message: &StoredMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } if !is_attached_image_label_text(text) => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn transcript_checkpoint(session: &Session, history: &History, index: usize) -> Checkpoint {
    let message = &session.messages[index];
    let boundary = history.conversation_boundaries.get(&message.id);
    Checkpoint {
        info: RewindCheckpoint {
            id: format!("{MESSAGE_TARGET_PREFIX}{}", message.id),
            target_kind: RewindTargetKind::UserPrompt,
            message_index: boundary.map_or(index, |boundary| boundary.message_index),
            prompt: prompt_text(message),
            timestamp: message.timestamp.map(|timestamp| timestamp.to_rfc3339()),
            can_restore_files: false,
            changed_file_count: 0,
        },
        before: BTreeMap::new(),
        blocked: BTreeSet::new(),
        boundary_message_id: match boundary {
            Some(boundary) => boundary.boundary_message_id.clone(),
            None => index
                .checked_sub(1)
                .map(|previous| session.messages[previous].id.clone()),
        },
        input_message_id: Some(message.id.clone()),
    }
}

/// Observe current state without capturing external contents into history.
fn observed_state(path: &Path) -> Result<FileState> {
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
                metadata: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let data = fs::read(path)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let mode = Some(u32::from(metadata.permissions().readonly()));
    #[cfg(unix)]
    let metadata_state = {
        use std::os::unix::fs::MetadataExt;
        Some(super::FileMetadata {
            uid: metadata.uid(),
            gid: metadata.gid(),
            xattrs: super::read_xattrs(path)?,
        })
    };
    #[cfg(not(unix))]
    let metadata_state = None;
    Ok(FileState {
        blob: Some(hex::encode(Sha256::digest(data))),
        mode,
        metadata: metadata_state,
    })
}

pub(super) fn checkpoint_info(history: &History, checkpoint: &Checkpoint) -> RewindCheckpoint {
    let mut info = checkpoint.info.clone();
    info.can_restore_files = checkpoint.blocked.is_empty();
    info.changed_file_count = 0;
    for (path, desired) in &checkpoint.before {
        match observed_state(path) {
            Ok(actual) if desired.matches(&actual) => (),
            Ok(actual) => {
                info.changed_file_count += 1;
                if !history
                    .expected
                    .get(path)
                    .is_some_and(|expected| expected.matches(&actual))
                {
                    info.can_restore_files = false;
                }
            }
            Err(_) => {
                info.changed_file_count += 1;
                info.can_restore_files = false;
            }
        }
    }
    info
}

pub(super) fn checkpoints(session: &Session) -> Result<Vec<RewindCheckpoint>> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let history = load(&directory(&session.id)?)?;
    let assistants: BTreeSet<_> = session
        .rewind_assistant_target_stored_indices()
        .into_iter()
        .collect();
    Ok(session
        .messages
        .iter()
        .enumerate()
        .filter(|(index, message)| is_authentic_user_prompt(message) || assistants.contains(index))
        .map(|(index, message)| {
            if assistants.contains(&index) {
                return assistant_info(message, index + 1);
            }
            let mut info = history
                .checkpoints
                .iter()
                .rev()
                .find(|checkpoint| {
                    checkpoint.input_message_id.as_ref() == Some(&message.id)
                        && reachable(session, checkpoint)
                })
                .map(|checkpoint| checkpoint_info(&history, checkpoint))
                .unwrap_or_else(|| transcript_checkpoint(session, &history, index).info);
            // File snapshot eviction must not change the conversation target ID.
            info.id = format!("{MESSAGE_TARGET_PREFIX}{}", message.id);
            info
        })
        .collect())
}

pub(super) fn resolve_checkpoint(
    session: &Session,
    target_id: &str,
    mode: RewindMode,
) -> Result<ResolvedTarget> {
    if let Some(message_id) = target_id.strip_prefix(ASSISTANT_TARGET_PREFIX) {
        ensure!(
            !mode.restores_files(),
            "Assistant messages support conversation-only rewind"
        );
        let keep_len = session
            .rewind_stored_len_for_assistant(message_id)
            .context("Rewind assistant message is not present in this conversation branch")?;
        let message = &session.messages[keep_len - 1];
        return Ok(ResolvedTarget::AssistantMessage {
            info: assistant_info(message, keep_len),
            boundary_message_id: message.id.clone(),
        });
    }
    let (checkpoint, images) = resolve_user_checkpoint(session, target_id, mode)?;
    Ok(ResolvedTarget::UserPrompt { checkpoint, images })
}

fn resolve_user_checkpoint(
    session: &Session,
    target_id: &str,
    mode: RewindMode,
) -> Result<(Checkpoint, Vec<(String, String)>)> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let history = load(&directory(&session.id)?)?;
    let checkpoint = if let Some(message_id) = target_id.strip_prefix(MESSAGE_TARGET_PREFIX) {
        let (index, message) = session
            .messages
            .iter()
            .enumerate()
            .find(|(_, message)| message.id == message_id && is_authentic_user_prompt(message))
            .context("Rewind prompt is not present in this conversation branch")?;
        match history.checkpoints.iter().rev().find(|checkpoint| {
            checkpoint.input_message_id.as_ref() == Some(&message.id)
                && reachable(session, checkpoint)
        }) {
            Some(checkpoint) => checkpoint.clone(),
            None => {
                ensure!(
                    !mode.restores_files(),
                    "No file snapshot is available for this prompt"
                );
                transcript_checkpoint(session, &history, index)
            }
        }
    } else {
        // Keep existing checkpoint UUIDs valid for already-open clients.
        history
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.info.id == target_id)
            .cloned()
            .context("Checkpoint not found")?
    };
    ensure!(
        reachable(session, &checkpoint),
        "Checkpoint belongs to a removed conversation branch"
    );
    let input = session
        .messages
        .iter()
        .skip(checkpoint.info.message_index)
        .find(|message| {
            checkpoint.input_message_id.as_ref() == Some(&message.id)
                && is_authentic_user_prompt(message)
        })
        .context("Checkpoint input is not an authentic user message")?;
    let images = if mode.restores_conversation() {
        input
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Image { media_type, data } => {
                    Some((media_type.clone(), data.clone()))
                }
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok((checkpoint, images))
}

pub(super) fn retain_before(history: &mut History, boundary: usize) {
    history
        .checkpoints
        .retain(|checkpoint| checkpoint.info.message_index < boundary);
    history
        .conversation_boundaries
        .retain(|_, saved| saved.message_index < boundary);
}
