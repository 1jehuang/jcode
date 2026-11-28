use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, Weak};

#[cfg(test)]
#[path = "extraction_tests.rs"]
mod tests;

type ExtractionLocks = HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>;
static EXTRACTION_LOCKS: LazyLock<Mutex<ExtractionLocks>> = LazyLock::new(Default::default);
static EXTRACTION_WRITES: Mutex<()> = Mutex::new(());

fn extraction_lock(checkpoint: &std::path::Path) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = EXTRACTION_LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(checkpoint).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(checkpoint.into(), Arc::downgrade(&lock));
    lock
}

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

        let scope = serde_json::to_vec(&(session_id, &self.project_dir))?;
        let checkpoint = storage::jcode_dir()?
            .join("memory/extraction-checkpoints")
            .join(format!("{:x}.json", Sha256::digest(scope)));
        let lock = extraction_lock(&checkpoint);
        let _extraction_guard = lock.lock().await;
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

        // Different checkpoints may share a project graph; serialize only its writes.
        let _write_guard = EXTRACTION_WRITES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
