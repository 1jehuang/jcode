//! Transcript targets are independent of optional, bounded file snapshots.
use super::*;
use crate::message::ContentBlock;
use crate::session::{
    Session, StoredMessage, is_attached_image_label_text, is_authentic_user_prompt,
};
use jcode_session_types::RewindMode;

const MESSAGE_TARGET_PREFIX: &str = "message:";

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
    Ok(FileState {
        blob: Some(hex::encode(Sha256::digest(data))),
        mode,
    })
}

pub(super) fn checkpoint_info(history: &History, checkpoint: &Checkpoint) -> RewindCheckpoint {
    let mut info = checkpoint.info.clone();
    info.can_restore_files = checkpoint.blocked.is_empty();
    info.changed_file_count = 0;
    for (path, desired) in &checkpoint.before {
        match observed_state(path) {
            Ok(actual) if &actual == desired => (),
            Ok(actual) => {
                info.changed_file_count += 1;
                if history.expected.get(path) != Some(&actual) {
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
    Ok(session
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| is_authentic_user_prompt(message))
        .map(|(index, message)| {
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
