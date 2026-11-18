use crate::{ContentBlock, Message, Role};

/// Keep the field order and omission rules identical to `Message`, including
/// the empty array used as the stable hash cache's metadata key.
#[derive(serde::Serialize)]
pub(super) struct Metadata<'a> {
    role: &'a Role,
    content: &'a [ContentBlock],
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: &'a Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_duration_ms: &'a Option<u64>,
}

impl<'a> Metadata<'a> {
    pub(super) fn new(message: &'a Message) -> Self {
        Self {
            role: &message.role,
            content: &[],
            timestamp: &message.timestamp,
            tool_duration_ms: &message.tool_duration_ms,
        }
    }
}

pub(super) fn json_bytes(value: &impl serde::Serialize) -> serde_json::Result<usize> {
    #[derive(Default)]
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter::default();
    serde_json::to_writer(&mut counter, value).map(|()| counter.0)
}

#[cfg(test)]
#[path = "message_metadata_tests.rs"]
mod tests;
