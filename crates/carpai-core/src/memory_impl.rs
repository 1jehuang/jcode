use std::path::PathBuf;
use std::collections::HashMap;
use async_trait::async_trait;
use tokio::fs;
use carpai_internal::*;
use tracing::{info, debug};

pub struct LocalMemoryBackend {
    base_path: PathBuf,
}

impl LocalMemoryBackend {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn ensure_dir(&self) -> Result<(), MemoryError> {
        std::fs::create_dir_all(&self.base_path)
            .map_err(|e| MemoryError::StorageError(e.to_string()))
    }

    fn entry_path(&self, id: &str) -> PathBuf {
        self.base_path.join(format!("{}.jsonl", id))
    }

    async fn load_entry(&self, id: &str) -> Result<Option<EnhancedMemoryEntry>, MemoryError> {
        let path = self.entry_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path).await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;
        let entry = serde_json::from_str::<EnhancedMemoryEntry>(content.trim())
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;
        Ok(Some(entry))
    }

    async fn save_entry(&self, entry: &EnhancedMemoryEntry) -> Result<(), MemoryError> {
        let path = self.entry_path(&entry.base.id);
        let line = serde_json::to_string(entry)
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;
        fs::write(&path, format!("{}\n", line)).await
            .map_err(|e| MemoryError::StorageError(e.to_string()))
    }
}

#[async_trait]
impl MemoryBackend for LocalMemoryBackend {
    async fn store(
        &self,
        mut entry: EnhancedMemoryEntry,
    ) -> Result<String, MemoryError> {
        self.ensure_dir()?;

        if entry.base.id.is_empty() {
            entry.base.id = format!("mem-{}", uuid::Uuid::new_v4());
        }

        self.save_entry(&entry).await?;

        debug!(memory_id = %entry.base.id, "Memory entry stored");
        Ok(entry.base.id)
    }

    async fn retrieve(
        &self,
        id: &str,
    ) -> Result<Option<EnhancedMemoryEntry>, MemoryError> {
        self.load_entry(id).await
    }

    async fn search(
        &self,
        query: &EnhancedMemoryQuery,
    ) -> Result<Vec<EnhancedMemoryEntry>, MemoryError> {
        self.ensure_dir()?;

        let mut entries = Vec::new();
        let limit = query.limit.unwrap_or(100);

        let mut dir = fs::read_dir(&self.base_path).await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        while let Some(file) = dir.next_entry().await
            .map_err(|e| MemoryError::StorageError(e.to_string()))? {
            let path = file.path();
            if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                continue;
            }

            let content = fs::read_to_string(&path).await.ok();
            if let Some(content) = content {
                for line in content.lines() {
                    if let Ok(entry) = serde_json::from_str::<EnhancedMemoryEntry>(line) {
                        if self.matches_query(&entry, query) {
                            entries.push(entry);
                            if entries.len() >= limit {
                                return Ok(entries);
                            }
                        }
                    }
                }
            }
        }

        Ok(entries)
    }

    async fn delete(
        &self,
        id: &str,
    ) -> Result<(), MemoryError> {
        let path = self.entry_path(id);

        if !path.exists() {
            return Err(MemoryError::NotFound(id.to_string()));
        }

        fs::remove_file(&path).await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        debug!(memory_id = %id, "Memory entry deleted");
        Ok(())
    }

    async fn update(
        &self,
        id: &str,
        updates: &EnhancedMemoryUpdate,
    ) -> Result<EnhancedMemoryEntry, MemoryError> {
        let mut entry = self.load_entry(id).await?
            .ok_or_else(|| MemoryError::NotFound(id.to_string()))?;

        if let Some(ref content) = updates.content {
            entry.base.content = content.clone();
        }
        if let Some(ref metadata) = updates.metadata {
            entry.base.metadata.extend(metadata.clone());
        }
        if let Some(ref tags) = updates.tags {
            entry.base.metadata.insert("tags".to_string(), tags.join(","));
        }
        if let Some(scope) = updates.scope {
            entry.scope = scope;
        }
        if let Some(trust) = updates.trust {
            entry.trust = trust;
        }
        if let Some(active) = updates.active {
            entry.active = active;
        }

        self.save_entry(&entry).await?;

        debug!(memory_id = %id, "Memory entry updated");
        Ok(entry)
    }

    async fn vector_search(
        &self,
        _embedding: &[f32],
        _limit: usize,
        _options: &VectorSearchOptions,
    ) -> Result<Vec<VectorSearchResult>, MemoryError> {
        Ok(Vec::new())
    }

    async fn upsert_embedding(
        &self,
        _memory_id: &str,
        _embedding: Vec<f32>,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn find_duplicate(
        &self,
        _content: &str,
        _threshold: f32,
    ) -> Result<Option<String>, MemoryError> {
        Ok(None)
    }

    async fn reinforce(
        &self,
        id: &str,
        session_id: &str,
        message_index: usize,
    ) -> Result<(), MemoryError> {
        let mut entry = self.load_entry(id).await?
            .ok_or_else(|| MemoryError::NotFound(id.to_string()))?;

        entry.strength += 1;
        entry.reinforcements.push(Reinforcement {
            session_id: session_id.to_string(),
            message_index,
            timestamp: chrono::Utc::now(),
        });

        self.save_entry(&entry).await?;

        debug!(memory_id = %id, "Memory reinforced");
        Ok(())
    }

    async fn consolidate(
        &self,
        primary_id: &str,
        merge_ids: &[String],
    ) -> Result<EnhancedMemoryEntry, MemoryError> {
        let mut primary = self.load_entry(primary_id).await?
            .ok_or_else(|| MemoryError::NotFound(primary_id.to_string()))?;

        for merge_id in merge_ids {
            if let Ok(Some(mut merge_entry)) = self.load_entry(merge_id).await {
                let metadata_to_merge = std::mem::take(&mut merge_entry.base.metadata);
                primary.base.metadata.extend(metadata_to_merge);
                primary.strength += merge_entry.strength;
                merge_entry.active = false;
                merge_entry.superseded_by = Some(primary_id.to_string());
                let _ = self.save_entry(&merge_entry).await;
            }
        }

        self.save_entry(&primary).await?;

        debug!(primary_id = %primary_id, merged_count = merge_ids.len(), "Memories consolidated");
        Ok(primary)
    }

    async fn get_by_scope(
        &self,
        scope: MemoryScope,
        _project_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EnhancedMemoryEntry>, MemoryError> {
        let query = EnhancedMemoryQuery {
            scope: Some(scope),
            active_only: true,
            limit: Some(limit),
            ..Default::default()
        };
        self.search(&query).await
    }

    async fn stats(
        &self,
        scope: Option<MemoryScope>,
    ) -> Result<EnhancedMemoryStats, MemoryError> {
        self.ensure_dir()?;

        let mut total_count = 0usize;
        let mut count_by_scope: HashMap<MemoryScope, usize> = HashMap::new();
        let mut count_by_type: HashMap<MemoryType, usize> = HashMap::new();
        let mut count_by_trust: HashMap<TrustLevel, usize> = HashMap::new();
        let mut total_confidence = 0.0f32;
        let mut storage_size_bytes = 0u64;
        let mut stale_count = 0usize;
        let mut superseded_count = 0usize;

        let mut dir = fs::read_dir(&self.base_path).await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        while let Some(file) = dir.next_entry().await
            .map_err(|e| MemoryError::StorageError(e.to_string()))? {
            let path = file.path();
            if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                continue;
            }

            if let Ok(metadata) = fs::metadata(&path).await {
                storage_size_bytes += metadata.len();
            }

            let content = fs::read_to_string(&path).await.ok();
            if let Some(content) = content {
                if let Ok(entry) = serde_json::from_str::<EnhancedMemoryEntry>(content.trim()) {
                    match scope {
                        Some(s) if s != entry.scope && s != MemoryScope::All => continue,
                        _ => {}
                    }

                    if !entry.active {
                        if entry.superseded_by.is_some() {
                            superseded_count += 1;
                        } else {
                            stale_count += 1;
                        }
                    }

                    total_count += 1;
                    total_confidence += entry.confidence;
                    *count_by_scope.entry(entry.scope).or_insert(0) += 1;
                    *count_by_type.entry(entry.base.memory_type).or_insert(0) += 1;
                    *count_by_trust.entry(entry.trust).or_insert(0) += 1;
                }
            }
        }

        let avg_confidence = if total_count > 0 {
            total_confidence / total_count as f32
        } else {
            0.0
        };

        Ok(EnhancedMemoryStats {
            total_count,
            count_by_scope,
            count_by_type,
            count_by_trust,
            avg_confidence,
            storage_size_bytes,
            stale_count,
            superseded_count,
        })
    }

    async fn cleanup(
        &self,
        options: &CleanupOptions,
    ) -> Result<CleanupResult, MemoryError> {
        self.ensure_dir()?;

        let mut pruned_count = 0usize;
        let mut superseded_count = 0usize;
        let mut freed_bytes = 0u64;
        let mut errors = Vec::new();

        let mut dir = fs::read_dir(&self.base_path).await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        while let Some(file) = dir.next_entry().await
            .map_err(|e| MemoryError::StorageError(e.to_string()))? {
            let path = file.path();
            if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                continue;
            }

            let should_delete = {
                let content = fs::read_to_string(&path).await.ok();
                match content {
                    Some(content) => {
                        if let Ok(entry) = serde_json::from_str::<EnhancedMemoryEntry>(content.trim()) {
                            let age_expired = options.older_than.map_or(false, |threshold| {
                                entry.base.created_at < threshold
                            });

                            let confidence_low = options.below_confidence.map_or(false, |min_conf| {
                                entry.confidence < min_conf
                            });

                            let is_stale = !entry.active && entry.superseded_by.is_none();

                            age_expired || confidence_low || (is_stale && options.hard_delete)
                        } else {
                            false
                        }
                    }
                    None => false,
                }
            };

            if should_delete {
                if let Ok(size) = fs::metadata(&path).await {
                    freed_bytes += size.len();
                }

                match fs::remove_file(&path).await {
                    Ok(_) => {
                        pruned_count += 1;
                        if let Some(max) = options.max_prune {
                            if pruned_count >= max {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        errors.push(format!("Failed to delete {}: {}", path.display(), e));
                    }
                }
            }
        }

        debug!(
            pruned = pruned_count,
            superseded = superseded_count,
            freed = freed_bytes,
            errors = errors.len(),
            "Cleanup completed"
        );

        Ok(CleanupResult {
            pruned_count,
            superseded_count,
            freed_bytes,
            errors,
        })
    }
}

impl LocalMemoryBackend {
    fn matches_query(&self, entry: &EnhancedMemoryEntry, query: &EnhancedMemoryQuery) -> bool {
        if query.active_only && !entry.active {
            return false;
        }

        if let Some(scope) = query.scope {
            if scope != MemoryScope::All && scope != entry.scope {
                return false;
            }
        }

        if let Some(memory_type) = query.memory_type {
            if memory_type != entry.base.memory_type {
                return false;
            }
        }

        if let Some(min_trust) = query.min_trust {
            match (min_trust, entry.trust) {
                (TrustLevel::High, TrustLevel::Medium) | (TrustLevel::High, TrustLevel::Low) => return false,
                (TrustLevel::Medium, TrustLevel::Low) => return false,
                _ => {}
            }
        }

        if let Some(ref text_query) = query.text_query {
            if !entry.base.content.to_lowercase().contains(&text_query.to_lowercase()) {
                return false;
            }
        }

        if let Some(ref tags) = query.tags {
            let entry_tags = entry.base.metadata.get("tags")
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
                .unwrap_or_default();

            if !tags.iter().all(|t| entry_tags.contains(t)) {
                return false;
            }
        }

        if let Some(after) = query.created_after {
            if entry.base.created_at < after {
                return false;
            }
        }

        if let Some(before) = query.created_before {
            if entry.base.created_at > before {
                return false;
            }
        }

        true
    }
}
