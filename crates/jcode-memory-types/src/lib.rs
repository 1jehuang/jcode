use std::time::Instant;

/// Represents current memory system activity.
#[derive(Debug, Clone)]
pub struct MemoryActivity {
    /// Current state of the memory system.
    pub state: MemoryState,
    /// When the current state was entered, used for elapsed time display and staleness detection.
    pub state_since: Instant,
    /// Pipeline progress for the per-turn search, verify, inject, maintain flow.
    pub pipeline: Option<PipelineState>,
    /// Recent events, most recent first.
    pub recent_events: Vec<MemoryEvent>,
}

impl MemoryActivity {
    pub fn is_processing(&self) -> bool {
        !matches!(self.state, MemoryState::Idle)
            || self
                .pipeline
                .as_ref()
                .map(PipelineState::has_running_step)
                .unwrap_or(false)
    }
}

/// Status of a single pipeline step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Error,
    Skipped,
}

/// Result data for a completed pipeline step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub summary: String,
    pub latency_ms: u64,
}

/// Tracks the 4-step per-turn memory pipeline: search, verify, inject, maintain.
#[derive(Debug, Clone)]
pub struct PipelineState {
    pub search: StepStatus,
    pub search_result: Option<StepResult>,
    pub verify: StepStatus,
    pub verify_result: Option<StepResult>,
    pub verify_progress: Option<(usize, usize)>,
    pub inject: StepStatus,
    pub inject_result: Option<StepResult>,
    pub maintain: StepStatus,
    pub maintain_result: Option<StepResult>,
    pub started_at: Instant,
}

impl PipelineState {
    pub fn new() -> Self {
        Self {
            search: StepStatus::Pending,
            search_result: None,
            verify: StepStatus::Pending,
            verify_result: None,
            verify_progress: None,
            inject: StepStatus::Pending,
            inject_result: None,
            maintain: StepStatus::Pending,
            maintain_result: None,
            started_at: Instant::now(),
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(
            (&self.search, &self.verify, &self.inject, &self.maintain),
            (
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
            )
        )
    }

    pub fn has_running_step(&self) -> bool {
        matches!(self.search, StepStatus::Running)
            || matches!(self.verify, StepStatus::Running)
            || matches!(self.inject, StepStatus::Running)
            || matches!(self.maintain, StepStatus::Running)
    }
}

impl Default for PipelineState {
    fn default() -> Self {
        Self::new()
    }
}

/// State of the memory sidecar.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MemoryState {
    /// Idle, no activity.
    #[default]
    Idle,
    /// Running embedding search.
    Embedding,
    /// Sidecar checking relevance.
    SidecarChecking { count: usize },
    /// Found relevant memories.
    FoundRelevant { count: usize },
    /// Extracting memories from conversation.
    Extracting { reason: String },
    /// Background maintenance or gardening of the memory graph.
    Maintaining { phase: String },
    /// Agent is actively using a memory tool.
    ToolAction { action: String, detail: String },
}

/// A memory system event.
#[derive(Debug, Clone)]
pub struct MemoryEvent {
    /// Type of event.
    pub kind: MemoryEventKind,
    /// When it happened.
    pub timestamp: Instant,
    /// Optional details.
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InjectedMemoryItem {
    pub section: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum MemoryEventKind {
    /// Embedding search started.
    EmbeddingStarted,
    /// Embedding search completed.
    EmbeddingComplete { latency_ms: u64, hits: usize },
    /// Sidecar started checking.
    SidecarStarted,
    /// Sidecar found memory relevant.
    SidecarRelevant { memory_preview: String },
    /// Sidecar found memory not relevant.
    SidecarNotRelevant,
    /// Sidecar call completed with latency.
    SidecarComplete { latency_ms: u64 },
    /// Memory was surfaced to main agent.
    MemorySurfaced { memory_preview: String },
    /// Memory payload was injected into model context.
    MemoryInjected {
        count: usize,
        prompt_chars: usize,
        age_ms: u64,
        preview: String,
        items: Vec<InjectedMemoryItem>,
    },
    /// Background maintenance started.
    MaintenanceStarted { verified: usize, rejected: usize },
    /// Background maintenance discovered or strengthened links.
    MaintenanceLinked { links: usize },
    /// Background maintenance adjusted confidence.
    MaintenanceConfidence { boosted: usize, decayed: usize },
    /// Background maintenance refined clusters.
    MaintenanceCluster { clusters: usize, members: usize },
    /// Background maintenance inferred or applied a shared tag.
    MaintenanceTagInferred { tag: String, applied: usize },
    /// Background maintenance detected a gap.
    MaintenanceGap { candidates: usize },
    /// Background maintenance completed.
    MaintenanceComplete { latency_ms: u64 },
    /// Extraction started.
    ExtractionStarted { reason: String },
    /// Extraction completed.
    ExtractionComplete { count: usize },
    /// Error occurred.
    Error { message: String },
    /// Agent stored a memory via tool.
    ToolRemembered {
        content: String,
        scope: String,
        category: String,
    },
    /// Agent recalled or searched memories via tool.
    ToolRecalled { query: String, count: usize },
    /// Agent forgot a memory via tool.
    ToolForgot { id: String },
    /// Agent tagged a memory via tool.
    ToolTagged { id: String, tags: String },
    /// Agent linked memories via tool.
    ToolLinked { from: String, to: String },
    /// Agent listed memories via tool.
    ToolListed { count: usize },
}

pub mod ranking {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    struct TopKItem<T> {
        score: f32,
        ordinal: usize,
        value: T,
    }

    impl<T> PartialEq for TopKItem<T> {
        fn eq(&self, other: &Self) -> bool {
            self.score.to_bits() == other.score.to_bits() && self.ordinal == other.ordinal
        }
    }

    impl<T> Eq for TopKItem<T> {}

    impl<T> PartialOrd for TopKItem<T> {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl<T> Ord for TopKItem<T> {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.score
                .total_cmp(&other.score)
                .then_with(|| self.ordinal.cmp(&other.ordinal))
        }
    }

    pub fn top_k_by_score<T, I>(items: I, limit: usize) -> Vec<(T, f32)>
    where
        I: IntoIterator<Item = (T, f32)>,
    {
        if limit == 0 {
            return Vec::new();
        }

        let mut heap: BinaryHeap<Reverse<TopKItem<T>>> = BinaryHeap::new();

        for (ordinal, (value, score)) in items.into_iter().enumerate() {
            let candidate = Reverse(TopKItem {
                score,
                ordinal,
                value,
            });

            if heap.len() < limit {
                heap.push(candidate);
                continue;
            }

            let replace = heap
                .peek()
                .map(|smallest| score > smallest.0.score)
                .unwrap_or(false);
            if replace {
                heap.pop();
                heap.push(candidate);
            }
        }

        let mut results: Vec<_> = heap
            .into_iter()
            .map(|Reverse(item)| (item.value, item.score, item.ordinal))
            .collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
        results
            .into_iter()
            .map(|(value, score, _)| (value, score))
            .collect()
    }

    #[derive(Debug)]
    struct TopKOrdItem<T, K> {
        key: K,
        ordinal: usize,
        value: T,
    }

    impl<T, K: Ord> PartialEq for TopKOrdItem<T, K> {
        fn eq(&self, other: &Self) -> bool {
            self.key == other.key && self.ordinal == other.ordinal
        }
    }

    impl<T, K: Ord> Eq for TopKOrdItem<T, K> {}

    impl<T, K: Ord> PartialOrd for TopKOrdItem<T, K> {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl<T, K: Ord> Ord for TopKOrdItem<T, K> {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.key
                .cmp(&other.key)
                .then_with(|| self.ordinal.cmp(&other.ordinal))
        }
    }

    pub fn top_k_by_ord<T, K, I>(items: I, limit: usize) -> Vec<(T, K)>
    where
        I: IntoIterator<Item = (T, K)>,
        K: Ord,
    {
        if limit == 0 {
            return Vec::new();
        }

        let mut heap: BinaryHeap<Reverse<TopKOrdItem<T, K>>> = BinaryHeap::new();

        for (ordinal, (value, key)) in items.into_iter().enumerate() {
            let candidate = Reverse(TopKOrdItem {
                key,
                ordinal,
                value,
            });

            if heap.len() < limit {
                heap.push(candidate);
                continue;
            }

            let replace = heap
                .peek()
                .map(|smallest| candidate.0.key > smallest.0.key)
                .unwrap_or(false);
            if replace {
                heap.pop();
                heap.push(candidate);
            }
        }

        let mut results: Vec<_> = heap
            .into_iter()
            .map(|Reverse(item)| (item.value, item.key, item.ordinal))
            .collect();
        results.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
        results
            .into_iter()
            .map(|(value, key, _)| (value, key))
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn top_k_by_score_keeps_highest_scores_in_order() {
            let ranked = top_k_by_score([("a", 1.0), ("b", 3.0), ("c", 2.0)], 2);
            assert_eq!(ranked, vec![("b", 3.0), ("c", 2.0)]);
        }

        #[test]
        fn top_k_by_ord_keeps_highest_keys_in_order() {
            let ranked = top_k_by_ord([("a", 1), ("b", 3), ("c", 2)], 2);
            assert_eq!(ranked, vec![("b", 3), ("c", 2)]);
        }

        #[test]
        fn top_k_zero_limit_is_empty() {
            assert!(top_k_by_score([("a", 1.0)], 0).is_empty());
            assert!(top_k_by_ord([("a", 1)], 0).is_empty());
        }
    }
}
