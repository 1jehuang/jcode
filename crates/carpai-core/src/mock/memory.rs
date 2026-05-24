use async_trait::async_trait;
use carpai_internal::*;

pub struct MockMemoryBackend;

impl Default for MockMemoryBackend {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl MemoryBackend for MockMemoryBackend {
    async fn store(&self, _entry: EnhancedMemoryEntry) -> Result<String, MemoryError> {
        Ok(format!("mem-{}", uuid::Uuid::new_v4().simple()))
    }

    async fn retrieve(&self, _id: &str) -> Result<Option<EnhancedMemoryEntry>, MemoryError> {
        Ok(None)
    }

    async fn search(&self, _query: &EnhancedMemoryQuery) -> Result<Vec<EnhancedMemoryEntry>, MemoryError> {
        Ok(vec![])
    }

    async fn delete(&self, _id: &str) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn update(&self, _id: &str, _updates: &EnhancedMemoryUpdate) -> Result<EnhancedMemoryEntry, MemoryError> {
        Err(MemoryError::NotFound(_id.into()))
    }

    async fn vector_search(
        &self,
        _embedding: &[f32],
        _limit: usize,
        _options: &VectorSearchOptions,
    ) -> Result<Vec<VectorSearchResult>, MemoryError> {
        Ok(vec![])
    }

    async fn upsert_embedding(&self, _memory_id: &str, _embedding: Vec<f32>) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn find_duplicate(&self, _content: &str, _threshold: f32) -> Result<Option<String>, MemoryError> {
        Ok(None)
    }

    async fn reinforce(&self, _id: &str, _session_id: &str, _message_index: usize) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn consolidate(&self, _primary_id: &str, _merge_ids: &[String]) -> Result<EnhancedMemoryEntry, MemoryError> {
        Err(MemoryError::NotFound(_primary_id.into()))
    }

    async fn get_by_scope(&self, _scope: MemoryScope, _project_id: Option<&str>, _limit: usize) -> Result<Vec<EnhancedMemoryEntry>, MemoryError> {
        Ok(vec![])
    }

    async fn stats(&self, _scope: Option<MemoryScope>) -> Result<EnhancedMemoryStats, MemoryError> {
        use std::collections::HashMap;
        Ok(EnhancedMemoryStats {
            total_count: 0,
            count_by_scope: HashMap::new(),
            count_by_type: HashMap::new(),
            count_by_trust: HashMap::new(),
            avg_confidence: 0.0,
            storage_size_bytes: 0,
            stale_count: 0,
            superseded_count: 0,
        })
    }

    async fn cleanup(&self, _options: &CleanupOptions) -> Result<CleanupResult, MemoryError> {
        Ok(CleanupResult {
            pruned_count: 0,
            superseded_count: 0,
            freed_bytes: 0,
            errors: vec![],
        })
    }
}
