//! Auto-dream integration for the ambient runner.
//!
//! After each ambient cycle, evaluates the three-gate `DreamCoordinator`
//! (time, session count, file lock) and triggers background memory
//! consolidation when all gates pass.

use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use jcode_auto_dream::consolidation_prompt::{ConsolidationPrompt, EntryRole, PromptEntry};
use jcode_auto_dream::{DreamCoordinator, DreamTrigger};
use jcode_message_types::{ContentBlock, Message, Role};
use jcode_provider_core::Provider;

use crate::logging;
use crate::memory::{MemoryCategory, MemoryManager};

/// Parsed consolidation operation returned by the LLM.
#[derive(Debug, serde::Deserialize)]
struct ConsolidationOp {
    op: String,
    ids: Vec<String>,
    reason: String,
}

/// Evaluate the dream coordinator and, if ready, run consolidation.
///
/// `data_dir` is the jcode data directory (e.g. `~/.jcode`).
/// `session_count` is the total number of ambient cycles completed
/// (fed to the session gate).
/// `provider` is the LLM provider used for the consolidation call.
pub async fn maybe_dream(
    data_dir: PathBuf,
    session_count: u64,
    provider: Arc<dyn Provider>,
) {
    let mut coordinator = DreamCoordinator::new(data_dir);

    // Feed the session gate with the ambient cycle count.
    let mut state = coordinator.session_gate.reset();
    for _ in 0..session_count {
        state = coordinator.session_gate.record_session(state);
    }

    // Evaluate the time gate using wall-clock time.
    coordinator.time_gate_open = evaluate_time_gate(&coordinator);

    match coordinator.evaluate() {
        DreamTrigger::Ready => {
            logging::info("Auto-dream: all gates passed, spawning consolidation");
            let lock = coordinator.lock;
            match lock.acquire().await {
                Ok(()) => {
                    logging::info("Auto-dream: lock acquired, running consolidation");
                    if let Err(e) = run_consolidation(&provider).await {
                        logging::error(&format!(
                            "Auto-dream: consolidation failed: {}",
                            e
                        ));
                    }
                    if let Err(e) = lock.release().await {
                        logging::error(&format!(
                            "Auto-dream: failed to release lock: {}",
                            e
                        ));
                    }
                    logging::info("Auto-dream: consolidation complete");
                }
                Err(e) => {
                    logging::error(&format!(
                        "Auto-dream: failed to acquire lock: {}",
                        e
                    ));
                }
            }
        }
        DreamTrigger::NotReady { reason } => {
            logging::info(&format!("Auto-dream: not ready ({})", reason));
        }
    }
}

/// Run the consolidation LLM call: load memories, build prompt, call LLM,
/// parse and log the resulting operations.
async fn run_consolidation(provider: &Arc<dyn Provider>) -> anyhow::Result<()> {
    let manager = MemoryManager::new();
    let entries = manager.list_all()?;

    if entries.is_empty() {
        logging::info("Auto-dream: no memories to consolidate");
        return Ok(());
    }

    let entry_count = entries.len();
    logging::info(&format!(
        "Auto-dream: building consolidation prompt for {} entries",
        entry_count
    ));

    let prompt_entries: Vec<PromptEntry> = entries
        .into_iter()
        .filter(|e| e.active)
        .map(|e| PromptEntry {
            id: e.id,
            content: e.content,
            role: category_to_role(&e.category),
            confidence: e.confidence,
            strength: e.strength,
            tags: e.tags,
        })
        .collect();

    if prompt_entries.is_empty() {
        logging::info("Auto-dream: no active memories to consolidate");
        return Ok(());
    }

    let prompt = ConsolidationPrompt::build(prompt_entries, None);

    // Build the user message with entries JSON attached.
    let entries_json = prompt.entries_json()?;
    let full_user_message = format!("{}\n\n{}", prompt.user_message, entries_json);

    let user_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: full_user_message,
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    };

    let stream = provider
        .complete(&[user_msg], &[], &prompt.system, None)
        .await?;

    // Collect the streaming response text.
    let mut response_text = String::new();
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        match event? {
            jcode_message_types::StreamEvent::TextDelta(delta) => {
                response_text.push_str(&delta);
            }
            jcode_message_types::StreamEvent::Error { message, .. } => {
                return Err(anyhow::anyhow!("LLM stream error: {}", message));
            }
            _ => {}
        }
    }

    if response_text.is_empty() {
        logging::info("Auto-dream: LLM returned empty response");
        return Ok(());
    }

    // Parse the JSON operations from the response.
    let ops: Vec<ConsolidationOp> = match serde_json::from_str(&response_text) {
        Ok(parsed) => parsed,
        Err(e) => {
            // The LLM may wrap the JSON in markdown fences; try to extract.
            let cleaned = response_text
                .trim()
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim();
            serde_json::from_str(cleaned).map_err(|_| {
                anyhow::anyhow!(
                    "Failed to parse consolidation response: {}. Response: {}",
                    e,
                    &response_text[..response_text.len().min(200)]
                )
            })?
        }
    };

    logging::info(&format!(
        "Auto-dream: LLM proposed {} consolidation operations",
        ops.len()
    ));
    for op in &ops {
        logging::info(&format!(
            "  [{}] ids={:?} reason={}",
            op.op, op.ids, op.reason
        ));
    }

    Ok(())
}

/// Map a `MemoryCategory` to the consolidation `EntryRole`.
fn category_to_role(category: &MemoryCategory) -> EntryRole {
    match category {
        MemoryCategory::Fact | MemoryCategory::Preference => EntryRole::Core,
        MemoryCategory::Entity | MemoryCategory::Correction => EntryRole::Working,
        MemoryCategory::Custom(_) => EntryRole::Episodic,
    }
}

/// Check whether the time gate is open by inspecting the lock file mtime.
///
/// If no lock file exists, the gate is open (first run). If the lock file
/// exists and was created more than 24 hours ago, the gate is open.
fn evaluate_time_gate(coordinator: &DreamCoordinator) -> bool {
    use std::time::{Duration, SystemTime};

    let lock_path = coordinator.lock.lock_path();
    let mtime = match std::fs::metadata(lock_path)
        .ok()
        .and_then(|m| m.modified().ok())
    {
        Some(t) => t,
        None => return true, // No lock file = first run = gate open
    };
    let elapsed = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or(Duration::ZERO);
    elapsed >= Duration::from_secs(jcode_auto_dream::DEFAULT_TIME_GATE_HOURS * 3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_gate_open_when_no_lock_file() {
        let coordinator = DreamCoordinator::new("/tmp/auto-dream-test-nonexistent");
        assert!(evaluate_time_gate(&coordinator));
    }

    #[test]
    fn category_to_role_mapping() {
        assert!(matches!(
            category_to_role(&MemoryCategory::Fact),
            EntryRole::Core
        ));
        assert!(matches!(
            category_to_role(&MemoryCategory::Preference),
            EntryRole::Core
        ));
        assert!(matches!(
            category_to_role(&MemoryCategory::Entity),
            EntryRole::Working
        ));
        assert!(matches!(
            category_to_role(&MemoryCategory::Correction),
            EntryRole::Working
        ));
        assert!(matches!(
            category_to_role(&MemoryCategory::Custom("note".into())),
            EntryRole::Episodic
        ));
    }
}
