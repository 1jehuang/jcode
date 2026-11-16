use crate::ContentBlock;
use std::sync::Arc;

/// Shared immutable message payload, detached on the first mutation.
///
/// Transcript records, provider caches and request snapshots can retain the same
/// blocks without copying their strings. Mutating any snapshot preserves the
/// independent-value semantics of the original vector. Serialization remains a
/// plain JSON array, including legacy persisted sessions.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SharedContent(Arc<Vec<ContentBlock>>);

impl SharedContent {
    pub fn into_vec(self) -> Vec<ContentBlock> {
        Arc::unwrap_or_clone(self.0)
    }

    /// Whether two snapshots retain the same vector and payload allocations.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl From<Vec<ContentBlock>> for SharedContent {
    fn from(value: Vec<ContentBlock>) -> Self {
        Self(Arc::new(value))
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
        &self.0
    }
}

impl std::ops::DerefMut for SharedContent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
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
        Arc::make_mut(&mut self.0).extend(iter);
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
