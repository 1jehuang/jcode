use super::*;
use sha2::{Digest, Sha256};

#[cfg(test)]
#[path = "extraction_tests.rs"]
mod tests;

static TRANSCRIPT_EXTRACTION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl MemoryManager {
    /// Extract memories from a session transcript using the configured sidecar.
    pub async fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        if !memory_llm_judge_available() {
            crate::logging::info("Memory transcript extraction skipped: LLM judge unavailable");
            return Ok(Vec::new());
        }

        // Final and periodic triggers must share the successful extraction checkpoint.
        let _extraction_guard = TRANSCRIPT_EXTRACTION_LOCK.lock().await;
        let scope = serde_json::to_vec(&(session_id, &self.project_dir))?;
        let checkpoint = storage::jcode_dir()?
            .join("memory/extraction-checkpoints")
            .join(format!("{:x}.json", Sha256::digest(scope)));
        let transcript_hash = format!("{:x}", Sha256::digest(transcript.as_bytes()));
        match storage::read_json::<String>(&checkpoint) {
            Ok(previous) if previous == transcript_hash => return Ok(Vec::new()),
            Ok(_) => {}
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => crate::logging::warn(&format!(
                "Ignoring unreadable memory extraction checkpoint: {error}"
            )),
        }

        let sidecar = crate::session::Session::load_startup_stub(session_id)
            .map(|session| Sidecar::for_session(&session))
            .unwrap_or_else(|_| Sidecar::new());
        let extracted = sidecar.extract_memories(transcript).await?;

        let mut ids = Vec::new();
        for memory in extracted {
            let category: MemoryCategory = memory.category.parse().unwrap_or(MemoryCategory::Fact);
            let trust = match memory.trust.as_str() {
                "high" => TrustLevel::High,
                "medium" => TrustLevel::Medium,
                _ => TrustLevel::Low,
            };

            let entry = MemoryEntry::new(category, memory.content)
                .with_source(session_id)
                .with_trust(trust);

            // Store in project scope by default
            let id = self.remember_project(entry)?;
            ids.push(id);
        }

        storage::write_json(&checkpoint, &transcript_hash)?;
        Ok(ids)
    }
}
