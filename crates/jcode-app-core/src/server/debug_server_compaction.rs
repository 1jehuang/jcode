#[derive(Default)]
pub(super) struct CompactionMemoryTotals {
    summary_text_bytes: u64,
    encrypted_content_bytes: u64,
    retained_payload_capacity_bytes: u64,
    persisted_metadata_payload_capacity_bytes: u64,
    sessions: Vec<serde_json::Value>,
}

impl CompactionMemoryTotals {
    pub(super) fn observe(&mut self, session_id: &str, profile: &serde_json::Value) {
        let compaction = &profile["compaction"];
        let bytes = |field: &str| compaction[field].as_u64().unwrap_or(0);
        let summary = bytes("summary_text_bytes");
        let encrypted = bytes("encrypted_content_bytes");
        let retained = bytes("retained_payload_capacity_bytes");
        let metadata = bytes("persisted_metadata_payload_capacity_bytes");
        self.summary_text_bytes = self.summary_text_bytes.saturating_add(summary);
        self.encrypted_content_bytes = self.encrypted_content_bytes.saturating_add(encrypted);
        self.retained_payload_capacity_bytes = self
            .retained_payload_capacity_bytes
            .saturating_add(retained);
        self.persisted_metadata_payload_capacity_bytes = self
            .persisted_metadata_payload_capacity_bytes
            .saturating_add(metadata);
        self.sessions.push(serde_json::json!({
            "session_id": session_id,
            "summary_text_bytes": summary,
            "encrypted_content_bytes": encrypted,
            "retained_payload_capacity_bytes": retained,
            "persisted_metadata_payload_capacity_bytes": metadata,
            "total_retained_payload_capacity_bytes": retained.saturating_add(metadata),
        }));
    }

    pub(super) fn into_json(mut self) -> serde_json::Value {
        self.sessions.sort_by(|left, right| {
            right["total_retained_payload_capacity_bytes"]
                .as_u64()
                .cmp(&left["total_retained_payload_capacity_bytes"].as_u64())
                .then_with(|| {
                    left["session_id"]
                        .as_str()
                        .cmp(&right["session_id"].as_str())
                })
        });
        self.sessions.truncate(12);
        serde_json::json!({
            "summary_text_bytes": self.summary_text_bytes,
            "encrypted_content_bytes": self.encrypted_content_bytes,
            "retained_payload_capacity_bytes": self.retained_payload_capacity_bytes,
            "persisted_metadata_payload_capacity_bytes": self.persisted_metadata_payload_capacity_bytes,
            "total_retained_payload_capacity_bytes": self.retained_payload_capacity_bytes
                .saturating_add(self.persisted_metadata_payload_capacity_bytes),
            "top_by_retained_payload_capacity_bytes": self.sessions,
        })
    }
}
