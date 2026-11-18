use crate::ContentBlock;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(test)]
#[path = "shared_content_bench_tests.rs"]
mod bench_tests;

/// Shared immutable message payload, detached on the first mutation.
///
/// Transcript records, provider caches and request snapshots can retain the same
/// blocks without copying their strings. Mutating any snapshot preserves the
/// independent-value semantics of the original vector. Serialization remains a
/// plain JSON array, including legacy persisted sessions.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SharedContent(Arc<ContentPayload>);

impl std::fmt::Debug for SharedContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedContent")
            .field(&self.0.blocks)
            .field(&self.0.metrics)
            .finish()
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
struct ContentPayload {
    blocks: Vec<ContentBlock>,
    #[serde(skip)]
    metrics: ContentMetrics,
}

impl Clone for ContentPayload {
    fn clone(&self) -> Self {
        Self {
            blocks: self.blocks.clone(),
            // COW detaches must never retain cached immutable measurements.
            metrics: ContentMetrics::default(),
        }
    }
}

#[derive(Debug, Default)]
struct ContentMetrics {
    json_bytes: OnceLock<usize>,
    message_hash: Mutex<Option<(Vec<u8>, u64)>>,
}

impl SharedContent {
    /// Exact serialized array length, shared by immutable snapshots. Keeping a
    /// count rather than JSON bytes avoids retaining a second transcript copy.
    pub fn serialized_json_bytes(&self) -> usize {
        *self.0.metrics.json_bytes.get_or_init(|| {
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
            serde_json::to_writer(&mut counter, &self.0.blocks)
                .map(|()| counter.0)
                .unwrap_or(0)
        })
    }

    pub(crate) fn record_serialized_json_bytes(&self, bytes: usize) {
        if let Err(competing_bytes) = self.0.metrics.json_bytes.set(bytes) {
            // Immutable snapshots may race to record the same measurement.
            // Keep the first value, but detect inconsistent measurements.
            debug_assert_eq!(self.0.metrics.json_bytes.get(), Some(&competing_bytes));
        }
    }

    pub(crate) fn cached_message_hash(
        &self,
        metadata: &[u8],
        compute: impl FnOnce() -> u64,
    ) -> u64 {
        if let Some((key, hash)) = &*self
            .0
            .metrics
            .message_hash
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            && key == metadata
        {
            return *hash;
        }
        // Serialization can be expensive. Do not hold the cache mutex while
        // reading a large payload or computing its hash.
        let hash = compute();
        *self
            .0
            .metrics
            .message_hash
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((metadata.to_vec(), hash));
        hash
    }

    fn mutable_payload(&mut self) -> &mut ContentPayload {
        // A shared detach copies only the vector and starts with empty metrics.
        // Unique mutations clear the existing metrics, releasing the hash key.
        let payload = Arc::make_mut(&mut self.0);
        payload.metrics.json_bytes.take();
        *payload
            .metrics
            .message_hash
            .get_mut()
            .unwrap_or_else(|e| e.into_inner()) = None;
        payload
    }

    pub fn into_vec(self) -> Vec<ContentBlock> {
        match Arc::try_unwrap(self.0) {
            Ok(payload) => payload.blocks,
            Err(payload) => payload.blocks.clone(),
        }
    }

    /// Whether two snapshots retain the same vector and payload allocations.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl From<Vec<ContentBlock>> for SharedContent {
    fn from(value: Vec<ContentBlock>) -> Self {
        Self(Arc::new(ContentPayload {
            blocks: value,
            metrics: ContentMetrics::default(),
        }))
    }
}

impl From<SharedContent> for Vec<ContentBlock> {
    fn from(value: SharedContent) -> Self {
        value.into_vec()
    }
}

impl std::ops::Deref for SharedContent {
    type Target = Vec<ContentBlock>;
    fn deref(&self) -> &Self::Target {
        &self.0.blocks
    }
}

impl std::ops::DerefMut for SharedContent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.mutable_payload().blocks
    }
}

impl AsRef<[ContentBlock]> for SharedContent {
    fn as_ref(&self) -> &[ContentBlock] {
        self.as_slice()
    }
}

impl AsMut<[ContentBlock]> for SharedContent {
    fn as_mut(&mut self) -> &mut [ContentBlock] {
        self.as_mut_slice()
    }
}

impl FromIterator<ContentBlock> for SharedContent {
    fn from_iter<T: IntoIterator<Item = ContentBlock>>(iter: T) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl Extend<ContentBlock> for SharedContent {
    fn extend<T: IntoIterator<Item = ContentBlock>>(&mut self, iter: T) {
        self.mutable_payload().blocks.extend(iter);
    }
}

impl IntoIterator for SharedContent {
    type Item = ContentBlock;
    type IntoIter = std::vec::IntoIter<ContentBlock>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a> IntoIterator for &'a SharedContent {
    type Item = &'a ContentBlock;
    type IntoIter = std::slice::Iter<'a, ContentBlock>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut SharedContent {
    type Item = &'a mut ContentBlock;
    type IntoIter = std::slice::IterMut<'a, ContentBlock>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> SharedContent {
        vec![ContentBlock::ToolResult {
            tool_use_id: "tool-1".into(),
            content: "retained result 🦀".repeat(1024),
            is_error: Some(false),
        }]
        .into()
    }

    #[test]
    fn cached_sizes_and_hashes_detach_on_all_mutation_paths() {
        let original = payload();
        let expected = serde_json::to_vec(&original).unwrap().len();
        assert_eq!(original.serialized_json_bytes(), expected);
        let mut clone = original.clone();
        assert!(Arc::ptr_eq(&original.0, &clone.0));
        assert_eq!(clone.cached_message_hash(b"meta", || 7), 7);
        assert_eq!(
            original.cached_message_hash(b"meta", || panic!("cache miss")),
            7
        );
        assert_eq!(original.cached_message_hash(b"changed", || 9), 9);
        if let ContentBlock::ToolResult { content, .. } = &mut clone[0] {
            content.push_str("changed");
        }
        assert!(!Arc::ptr_eq(&original.0, &clone.0));
        assert_eq!(original.serialized_json_bytes(), expected);
        assert_eq!(
            clone.serialized_json_bytes(),
            serde_json::to_vec(&clone).unwrap().len()
        );
        assert_eq!(clone.cached_message_hash(b"changed", || 11), 11);

        clone.extend(Vec::<ContentBlock>::new());
        assert!(clone.0.metrics.json_bytes.get().is_none());
        let _ = clone.serialized_json_bytes();
        let _ = clone.as_mut();
        assert!(clone.0.metrics.json_bytes.get().is_none());
        let _ = clone.serialized_json_bytes();
        for block in &mut clone {
            if let ContentBlock::ToolResult { content, .. } = block {
                content.clear();
            }
        }
        assert_eq!(
            clone.serialized_json_bytes(),
            serde_json::to_vec(&clone).unwrap().len()
        );
    }

    #[test]
    fn message_hash_and_size_match_legacy_bytes_across_metadata_and_edits() {
        use crate::{Message, Role, stable_hash_bytes, stable_message_hash};
        let mut message = Message {
            role: Role::Assistant,
            content: payload(),
            timestamp: None,
            tool_duration_ms: None,
        };
        for step in 0..5 {
            let reference = serde_json::to_vec(&message).unwrap();
            assert_eq!(stable_message_hash(&message), stable_hash_bytes(&reference));
            assert_eq!(message.serialized_json_bytes(), reference.len());
            assert_eq!(
                stable_message_hash(&message.clone()),
                stable_hash_bytes(&reference)
            );
            match step {
                0 => message.role = Role::User,
                1 => message.timestamp = Some(chrono::DateTime::from_timestamp(1234, 0).unwrap()),
                2 => message.tool_duration_ms = Some(999),
                3 => message.content.clear(),
                _ => {}
            }
        }
        assert_eq!(
            crate::extend_stable_hash(12, 34),
            stable_hash_bytes(&[12_u64.to_le_bytes(), 34_u64.to_le_bytes()].concat())
        );
    }

    #[test]
    #[ignore = "comparative immutable-content benchmark, run with --ignored --nocapture"]
    fn benchmark_repeated_message_hash_and_size() {
        use crate::{Message, Role, stable_hash_bytes, stable_message_hash};
        use std::hint::black_box;
        use std::time::Instant;
        let message = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "benchmark".into(),
                content: "escaped\n\"界".repeat(32_768),
                is_error: Some(false),
            }]
            .into(),
            timestamp: None,
            tool_duration_ms: None,
        };
        let expected = stable_hash_bytes(&serde_json::to_vec(&message).unwrap());
        assert_eq!(stable_message_hash(&message), expected);
        let iterations = 20;
        let start = Instant::now();
        for _ in 0..iterations {
            let bytes = serde_json::to_vec(black_box(&message)).unwrap();
            black_box(stable_hash_bytes(&bytes));
            black_box(serde_json::to_vec(black_box(&message)).unwrap().len());
        }
        let uncached = start.elapsed();
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(stable_message_hash(black_box(&message)));
            black_box(message.serialized_json_bytes());
        }
        let cached = start.elapsed();
        eprintln!(
            "immutable message metrics: iterations={iterations} uncached={uncached:?} cached={cached:?} ratio={:.2}x",
            uncached.as_secs_f64() / cached.as_secs_f64()
        );
    }

    #[test]
    fn clone_shares_payload_until_mutation() {
        let original = payload();
        let mut copy = original.clone();
        assert!(original.shares_storage_with(&copy));
        let expected = serde_json::to_string(&original).unwrap();
        if let ContentBlock::ToolResult { content, .. } = &mut copy[0] {
            content.push_str(" independently edited");
        }
        assert!(!original.shares_storage_with(&copy));
        assert_eq!(serde_json::to_string(&original).unwrap(), expected);
        assert_ne!(serde_json::to_string(&copy).unwrap(), expected);
    }

    #[test]
    fn serde_matches_legacy_vector_bytes() {
        let shared = payload();
        let json = serde_json::to_string(&shared).unwrap();
        assert_eq!(json, serde_json::to_string(shared.as_slice()).unwrap());
        let loaded: SharedContent = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&loaded).unwrap(), json);
    }

    #[test]
    fn uniquely_owned_into_vec_reuses_vector_allocation() {
        let shared = payload();
        let original_pointer = shared.as_ptr();
        let owned = shared.into_vec();
        assert_eq!(owned.as_ptr(), original_pointer);
    }

    #[test]
    fn owned_iteration_preserves_other_snapshots() {
        let original = payload();
        let copy = original.clone();
        let blocks: Vec<_> = copy.into_iter().collect();
        assert_eq!(
            serde_json::to_string(&blocks).unwrap(),
            serde_json::to_string(&original).unwrap()
        );
        let mut detached = original.clone();
        detached.clear();
        assert_eq!(original.len(), 1);
        assert!(detached.is_empty());
    }
}
