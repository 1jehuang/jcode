use crate::message::{ContentBlock, Message};
use crate::session::Session;
use crate::side_panel::SidePanelSnapshot;

use super::DisplayMessage;

const LARGE_DISPLAY_BLOB_THRESHOLD_BYTES: usize = 16 * 1024;

pub fn build_transcript_memory_profile(
    session: &Session,
    resident_provider_messages: &[Message],
    materialized_provider_messages: &[Message],
    provider_view_source: &str,
    display_messages: &[DisplayMessage],
    side_panel: &SidePanelSnapshot,
) -> serde_json::Value {
    let session_profile = session.debug_memory_profile();
    let canonical_transcript_json_bytes = nested_usize(
        &session_profile,
        &["totals", "canonical_transcript_json_bytes"],
    );
    let session_provider_cache_json_bytes =
        nested_usize(&session_profile, &["totals", "provider_cache_json_bytes"]);

    let resident_provider_json_bytes: usize = resident_provider_messages
        .iter()
        .map(crate::process_memory::estimate_json_bytes)
        .sum();
    let mut resident_provider_memory = ProviderMessageMemoryStats::default();
    for message in resident_provider_messages {
        resident_provider_memory.record_message(message);
    }

    let materialized_provider_json_bytes: usize = materialized_provider_messages
        .iter()
        .map(crate::process_memory::estimate_json_bytes)
        .sum();
    let mut materialized_provider_memory = ProviderMessageMemoryStats::default();
    for message in materialized_provider_messages {
        materialized_provider_memory.record_message(message);
    }

    let display_messages_bytes: usize = display_messages
        .iter()
        .map(estimate_display_message_bytes)
        .sum();
    let mut display_memory = DisplayMessageMemoryStats::default();
    for message in display_messages {
        display_memory.record_message(message);
    }

    let side_panel_memory = estimate_side_panel_memory(side_panel);
    let transient_provider_materialization_json_bytes =
        if provider_view_source == "session_materialized" {
            materialized_provider_json_bytes
        } else {
            0
        };

    serde_json::json!({
        "canonical_transcript": {
            "message_count": session.messages.len(),
            "json_bytes": canonical_transcript_json_bytes,
        },
        "session_provider_cache": {
            "count": session
                .debug_memory_profile()["provider_messages_cache"]["count"]
                .as_u64()
                .unwrap_or(0),
            "json_bytes": session_provider_cache_json_bytes,
        },
        "resident_provider_messages": {
            "count": resident_provider_messages.len(),
            "json_bytes": resident_provider_json_bytes,
            "content_blocks": resident_provider_memory.content_blocks,
            "payload_text_bytes": resident_provider_memory.payload_text_bytes(),
            "tool_result_bytes": resident_provider_memory.tool_result_bytes,
            "tool_use_input_json_bytes": resident_provider_memory.tool_use_input_json_bytes,
            "large_blob_count": resident_provider_memory.large_blob_count,
            "large_blob_bytes": resident_provider_memory.large_blob_bytes,
        },
        "provider_view": {
            "source": provider_view_source,
            "count": materialized_provider_messages.len(),
            "json_bytes": materialized_provider_json_bytes,
            "content_blocks": materialized_provider_memory.content_blocks,
            "payload_text_bytes": materialized_provider_memory.payload_text_bytes(),
            "tool_result_bytes": materialized_provider_memory.tool_result_bytes,
            "tool_use_input_json_bytes": materialized_provider_memory.tool_use_input_json_bytes,
            "large_blob_count": materialized_provider_memory.large_blob_count,
            "large_blob_bytes": materialized_provider_memory.large_blob_bytes,
        },
        "display": {
            "count": display_messages.len(),
            "estimate_bytes": display_messages_bytes,
            "content_bytes": display_memory.content_bytes,
            "chrome_text_bytes": display_memory.chrome_text_bytes(),
            "tool_metadata_json_bytes": display_memory.tool_data_json_bytes,
            "tool_rows": {
                "count": display_memory.tool_rows,
                "content_bytes": display_memory.tool_output_bytes,
                "large_count": display_memory.large_tool_output_count,
                "large_bytes": display_memory.large_tool_output_bytes,
            },
            "large_content_count": display_memory.large_content_count,
            "large_content_bytes": display_memory.large_content_bytes,
            "max_content_bytes": display_memory.max_content_bytes,
        },
        "side_panel": {
            "page_count": side_panel_memory.page_count,
            "focused_page_present": side_panel_memory.focused_page_present,
            "focused_content_bytes": side_panel_memory.focused_content_bytes,
            "unfocused_content_bytes": side_panel_memory.unfocused_content_bytes,
            "content_bytes": side_panel_memory.content_bytes,
            "metadata_bytes": side_panel_memory.metadata_bytes,
            "estimate_bytes": side_panel_memory.estimate_bytes,
        },
        "totals": {
            "canonical_transcript_json_bytes": canonical_transcript_json_bytes,
            "session_provider_cache_json_bytes": session_provider_cache_json_bytes,
            "resident_provider_messages_json_bytes": resident_provider_json_bytes,
            "transient_provider_materialization_json_bytes": transient_provider_materialization_json_bytes,
            "provider_view_json_bytes": materialized_provider_json_bytes,
            "display_content_bytes": display_memory.content_bytes,
            "display_chrome_text_bytes": display_memory.chrome_text_bytes(),
            "display_tool_metadata_json_bytes": display_memory.tool_data_json_bytes,
            "display_large_tool_output_bytes": display_memory.large_tool_output_bytes,
            "side_panel_content_bytes": side_panel_memory.content_bytes,
            "side_panel_metadata_bytes": side_panel_memory.metadata_bytes,
        }
    })
}

pub fn estimate_display_message_bytes(message: &DisplayMessage) -> usize {
    message.role.capacity()
        + message.content.capacity()
        + message
            .tool_calls
            .iter()
            .map(|call| call.capacity())
            .sum::<usize>()
        + message
            .title
            .as_ref()
            .map(|title| title.capacity())
            .unwrap_or(0)
        + message
            .tool_data
            .as_ref()
            .map(crate::process_memory::estimate_json_bytes)
            .unwrap_or(0)
}

fn nested_usize(value: &serde_json::Value, path: &[&str]) -> usize {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return 0;
        };
        cursor = next;
    }
    cursor
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

#[derive(Default)]
struct ProviderMessageMemoryStats {
    content_blocks: usize,
    text_bytes: usize,
    reasoning_bytes: usize,
    tool_use_input_json_bytes: usize,
    tool_result_bytes: usize,
    image_data_bytes: usize,
    openai_compaction_bytes: usize,
    large_blob_count: usize,
    large_blob_bytes: usize,
    max_block_bytes: usize,
}

impl ProviderMessageMemoryStats {
    fn record_bytes(&mut self, bytes: usize) {
        self.max_block_bytes = self.max_block_bytes.max(bytes);
        if bytes >= LARGE_DISPLAY_BLOB_THRESHOLD_BYTES {
            self.large_blob_count += 1;
            self.large_blob_bytes += bytes;
        }
    }

    fn record_message(&mut self, message: &Message) {
        for block in &message.content {
            self.content_blocks += 1;
            match block {
                ContentBlock::Text { text, .. } => {
                    self.text_bytes += text.len();
                    self.record_bytes(text.len());
                }
                ContentBlock::Reasoning { text } => {
                    self.reasoning_bytes += text.len();
                    self.record_bytes(text.len());
                }
                ContentBlock::ToolUse { input, .. } => {
                    let bytes = crate::process_memory::estimate_json_bytes(input);
                    self.tool_use_input_json_bytes += bytes;
                    self.record_bytes(bytes);
                }
                ContentBlock::ToolResult { content, .. } => {
                    self.tool_result_bytes += content.len();
                    self.record_bytes(content.len());
                }
                ContentBlock::Image { data, .. } => {
                    self.image_data_bytes += data.len();
                    self.record_bytes(data.len());
                }
                ContentBlock::OpenAICompaction { encrypted_content } => {
                    self.openai_compaction_bytes += encrypted_content.len();
                    self.record_bytes(encrypted_content.len());
                }
            }
        }
    }

    fn payload_text_bytes(&self) -> usize {
        self.text_bytes
            + self.reasoning_bytes
            + self.tool_result_bytes
            + self.image_data_bytes
            + self.openai_compaction_bytes
    }
}

#[derive(Default)]
struct DisplayMessageMemoryStats {
    role_bytes: usize,
    content_bytes: usize,
    tool_call_text_bytes: usize,
    title_bytes: usize,
    tool_data_json_bytes: usize,
    tool_rows: usize,
    tool_output_bytes: usize,
    large_tool_output_count: usize,
    large_tool_output_bytes: usize,
    large_content_count: usize,
    large_content_bytes: usize,
    max_content_bytes: usize,
}

impl DisplayMessageMemoryStats {
    fn record_message(&mut self, message: &DisplayMessage) {
        self.role_bytes += message.role.len();
        self.content_bytes += message.content.len();
        self.tool_call_text_bytes += message
            .tool_calls
            .iter()
            .map(|call| call.len())
            .sum::<usize>();
        self.title_bytes += message.title.as_ref().map(|title| title.len()).unwrap_or(0);
        self.tool_data_json_bytes += message
            .tool_data
            .as_ref()
            .map(crate::process_memory::estimate_json_bytes)
            .unwrap_or(0);
        self.max_content_bytes = self.max_content_bytes.max(message.content.len());
        if message.content.len() >= LARGE_DISPLAY_BLOB_THRESHOLD_BYTES {
            self.large_content_count += 1;
            self.large_content_bytes += message.content.len();
        }
        if message.role == "tool" {
            self.tool_rows += 1;
            self.tool_output_bytes += message.content.len();
            if message.content.len() >= LARGE_DISPLAY_BLOB_THRESHOLD_BYTES {
                self.large_tool_output_count += 1;
                self.large_tool_output_bytes += message.content.len();
            }
        }
    }

    fn chrome_text_bytes(&self) -> usize {
        self.role_bytes + self.tool_call_text_bytes + self.title_bytes
    }
}

#[derive(Default)]
struct SidePanelMemoryStats {
    page_count: usize,
    focused_page_present: bool,
    focused_content_bytes: usize,
    unfocused_content_bytes: usize,
    content_bytes: usize,
    metadata_bytes: usize,
    estimate_bytes: usize,
}

fn estimate_side_panel_memory(snapshot: &SidePanelSnapshot) -> SidePanelMemoryStats {
    let focused_page_id = snapshot.focused_page_id.as_deref();
    let mut stats = SidePanelMemoryStats {
        page_count: snapshot.pages.len(),
        focused_page_present: snapshot.focused_page().is_some(),
        ..SidePanelMemoryStats::default()
    };

    stats.metadata_bytes += snapshot
        .focused_page_id
        .as_ref()
        .map(|id| id.capacity())
        .unwrap_or(0);

    for page in &snapshot.pages {
        let page_metadata_bytes =
            page.id.capacity() + page.title.capacity() + page.file_path.capacity();
        stats.metadata_bytes += page_metadata_bytes;
        stats.content_bytes += page.content.capacity();
        stats.estimate_bytes += page_metadata_bytes + page.content.capacity();
        if focused_page_id == Some(page.id.as_str()) {
            stats.focused_content_bytes += page.content.capacity();
        } else {
            stats.unfocused_content_bytes += page.content.capacity();
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Role};
    use crate::side_panel::{SidePanelPage, SidePanelPageFormat, SidePanelPageSource};

    #[test]
    fn transcript_memory_profile_breaks_out_provider_display_and_side_panel() {
        let mut session = Session::create_with_id(
            "session_memory_profile_unit".to_string(),
            None,
            Some("memory profile".to_string()),
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello world".to_string(),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![
                ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "printf 'hello'"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: "hello from tool".to_string(),
                    is_error: None,
                },
            ],
        );

        let display_messages = crate::tui::display_messages_from_session(&session);
        let provider_messages = session.messages_for_provider_uncached();
        let side_panel = SidePanelSnapshot {
            focused_page_id: Some("page_a".to_string()),
            pages: vec![
                SidePanelPage {
                    id: "page_a".to_string(),
                    title: "Focused".to_string(),
                    file_path: "/tmp/focused.md".to_string(),
                    format: SidePanelPageFormat::Markdown,
                    source: SidePanelPageSource::Managed,
                    content: "# Focused\nhello".to_string(),
                    updated_at_ms: 1,
                },
                SidePanelPage {
                    id: "page_b".to_string(),
                    title: "Other".to_string(),
                    file_path: "/tmp/other.md".to_string(),
                    format: SidePanelPageFormat::Markdown,
                    source: SidePanelPageSource::Managed,
                    content: "# Other\nworld".to_string(),
                    updated_at_ms: 2,
                },
            ],
        };

        let profile = build_transcript_memory_profile(
            &session,
            &[],
            &provider_messages,
            "session_materialized",
            &display_messages,
            &side_panel,
        );

        assert_eq!(
            profile["canonical_transcript"]["message_count"],
            serde_json::json!(2)
        );
        assert_eq!(
            profile["provider_view"]["source"],
            serde_json::json!("session_materialized")
        );
        assert_eq!(profile["display"]["count"], serde_json::json!(2));
        assert_eq!(
            profile["display"]["tool_rows"]["count"],
            serde_json::json!(1)
        );
        assert_eq!(profile["side_panel"]["page_count"], serde_json::json!(2));
        assert!(
            profile["totals"]["transient_provider_materialization_json_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
    }
}
