use super::*;
use std::{hint::black_box, time::Instant};

// Retain the exact former representation for paired, same-process measurements.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
struct Original(Arc<Vec<ContentBlock>>, #[serde(skip)] Arc<ContentMetrics>);

trait BenchContent: Clone + for<'de> serde::Deserialize<'de> {
    fn from_blocks(blocks: Vec<ContentBlock>) -> Self;
    fn mutate(&mut self);
    fn into_blocks(self) -> Vec<ContentBlock>;
}
impl BenchContent for Original {
    fn from_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self(Arc::new(blocks), Arc::default())
    }
    fn mutate(&mut self) {
        if let Some(metrics) = Arc::get_mut(&mut self.1) {
            metrics.json_bytes.take();
            *metrics
                .message_hash
                .get_mut()
                .unwrap_or_else(|e| e.into_inner()) = None;
        } else {
            self.1 = Arc::default();
        }
        black_box(Arc::make_mut(&mut self.0));
    }
    fn into_blocks(self) -> Vec<ContentBlock> {
        Arc::unwrap_or_clone(self.0)
    }
}
impl BenchContent for SharedContent {
    fn from_blocks(blocks: Vec<ContentBlock>) -> Self {
        blocks.into()
    }
    fn mutate(&mut self) {
        black_box(self.as_mut());
    }
    fn into_blocks(self) -> Vec<ContentBlock> {
        self.into_vec()
    }
}
fn measure<T: BenchContent>(blocks: &[ContentBlock], json: &[u8], path: &str) -> f64 {
    let shared = T::from_blocks(blocks.to_vec());
    let iterations = 100_000;
    let start = Instant::now();
    for _ in 0..iterations {
        match path {
            "construct_drop" => {
                black_box(T::from_blocks(black_box(blocks.to_vec())));
            }
            "deserialize_drop" => {
                black_box(serde_json::from_slice::<T>(black_box(json)).unwrap());
            }
            "clone_drop" => {
                black_box(black_box(&shared).clone());
            }
            "unique_mutate" => {
                let mut value = T::from_blocks(blocks.to_vec());
                value.mutate();
                black_box(value);
            }
            "shared_mutate" => {
                let mut value = shared.clone();
                value.mutate();
                black_box(value);
            }
            "unique_into_vec" => {
                black_box(T::from_blocks(blocks.to_vec()).into_blocks());
            }
            "shared_into_vec" => {
                black_box(shared.clone().into_blocks());
            }
            _ => unreachable!(),
        }
    }
    start.elapsed().as_nanos() as f64 / iterations as f64
}
#[test]
#[ignore = "paired representation benchmark, run selfdev with --ignored --nocapture"]
fn benchmark_representation_paths() {
    for size in [0, 128, 16_384] {
        let blocks = if size == 0 {
            vec![]
        } else {
            vec![ContentBlock::ToolResult {
                tool_use_id: "bench".into(),
                content: "x".repeat(size),
                is_error: None,
            }]
        };
        let json = serde_json::to_vec(&blocks).unwrap();
        for path in [
            "construct_drop",
            "deserialize_drop",
            "clone_drop",
            "unique_mutate",
            "shared_mutate",
            "unique_into_vec",
            "shared_into_vec",
        ] {
            let (mut old, mut new) = (Vec::new(), Vec::new());
            for round in 0..9 {
                if round % 2 == 0 {
                    old.push(measure::<Original>(&blocks, &json, path));
                    new.push(measure::<SharedContent>(&blocks, &json, path));
                } else {
                    new.push(measure::<SharedContent>(&blocks, &json, path));
                    old.push(measure::<Original>(&blocks, &json, path));
                }
            }
            old.sort_by(f64::total_cmp);
            new.sort_by(f64::total_cmp);
            eprintln!(
                "paired size={size} path={path} old_ns={:.2} new_ns={:.2} change={:+.2}% old={old:?} new={new:?}",
                old[4],
                new[4],
                100.0 * (new[4] / old[4] - 1.0)
            );
        }
    }
}

#[test]
fn detach_and_unique_mutation_release_cached_metadata() {
    let mut original = SharedContent::from(vec![]);
    original.record_serialized_json_bytes(2);
    original.cached_message_hash(&vec![42; 1_000_000], || 17);
    let mut detached = original.clone();
    detached.extend([]);
    assert!(detached.0.metrics.json_bytes.get().is_none());
    assert!(detached.0.metrics.message_hash.lock().unwrap().is_none());
    assert_eq!(
        original
            .0
            .metrics
            .message_hash
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .0
            .len(),
        1_000_000
    );
    original.clear();
    assert!(original.0.metrics.json_bytes.get().is_none());
    assert!(original.0.metrics.message_hash.lock().unwrap().is_none());
    assert_eq!(Arc::strong_count(&detached.0), 1);
}

#[test]
fn original_representation_wire_and_public_trait_parity() {
    fn assert_traits<T: Send + Sync + Clone + Default + std::fmt::Debug>() {}
    assert_traits::<SharedContent>();
    for json in [
        "[]".to_owned(),
        serde_json::to_string(&vec![ContentBlock::ToolResult {
            tool_use_id: "id\\\"🦀".into(),
            content: "escaped\n\t\"界".repeat(1024),
            is_error: Some(false),
        }])
        .unwrap(),
    ] {
        let original: Original = serde_json::from_str(&json).unwrap();
        let combined: SharedContent = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_vec(&original).unwrap(),
            serde_json::to_vec(&combined).unwrap()
        );
        assert_eq!(
            combined.serialized_json_bytes(),
            serde_json::to_vec(&original).unwrap().len()
        );
        let copy = combined.clone();
        let shared_vec = copy.into_vec();
        assert_eq!(
            serde_json::to_vec(&shared_vec).unwrap(),
            serde_json::to_vec(&original).unwrap()
        );
        assert_eq!(Arc::strong_count(&combined.0), 1);
    }
    for malformed in ["{}", "null", "[", "[42]"] {
        assert!(serde_json::from_str::<Original>(malformed).is_err());
        assert!(serde_json::from_str::<SharedContent>(malformed).is_err());
    }
}

#[test]
fn consuming_unique_payload_releases_metrics_without_retaining_blocks() {
    let blocks = vec![ContentBlock::ToolResult {
        tool_use_id: "id".into(),
        content: "x".repeat(1_000_000),
        is_error: None,
    }];
    let pointer = blocks.as_ptr();
    let mut value = SharedContent::from(blocks);
    let weak = Arc::downgrade(&value.0);
    value.record_serialized_json_bytes(1_000_000);
    value.cached_message_hash(b"metadata", || 9);
    // Weak pointers are not public, but exercise the Arc uniqueness edge too.
    value.extend([]);
    assert!(weak.upgrade().is_none());
    assert_eq!(value.as_ptr(), pointer);
    let weak = Arc::downgrade(&value.0);
    let owned = value.into_vec();
    assert_eq!(owned.as_ptr(), pointer);
    assert!(weak.upgrade().is_none());
}
