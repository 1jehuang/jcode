use super::*;

pub async fn build_transfer_compaction_state(
    provider: Arc<dyn Provider>,
    messages: Vec<Message>,
    existing_state: Option<crate::session::StoredCompactionState>,
) -> Result<Option<crate::session::StoredCompactionState>> {
    let existing_summary = existing_state.as_ref().map(|state| Summary {
        text: state.summary_text.clone(),
        openai_encrypted_content: state.openai_encrypted_content.clone(),
        covers_up_to_turn: state.covers_up_to_turn,
        original_turn_count: state.original_turn_count,
    });

    if messages.is_empty() {
        return Ok(existing_state.map(|mut state| {
            state.compacted_count = 0;
            state
        }));
    }

    let prior_turns = existing_state
        .as_ref()
        .map(|state| state.original_turn_count.max(state.covers_up_to_turn))
        .unwrap_or(0);
    let result = generate_compaction_artifact(provider, messages.clone(), existing_summary).await?;
    let total_turns = prior_turns + messages.len();

    Ok(Some(crate::session::StoredCompactionState {
        summary_text: result.summary_text,
        openai_encrypted_content: result.openai_encrypted_content,
        covers_up_to_turn: total_turns,
        original_turn_count: total_turns,
        compacted_count: 0,
    }))
}
