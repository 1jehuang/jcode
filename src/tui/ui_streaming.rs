use std::time::Instant;
use std::ops::{Range, RangeFrom};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Rope {
    chunks: Vec<String>,
    total_len: usize,
}

impl Rope {
    pub fn new() -> Self {
        Self { chunks: Vec::new(), total_len: 0 }
    }

    pub fn from_str(s: &str) -> Self {
        if s.is_empty() { return Self::new(); }
        Self { chunks: vec![s.to_string()], total_len: s.len() }
    }

    pub fn append(&mut self, data: &str) {
        if data.is_empty() { return; }
        self.total_len += data.len();
        if let Some(last) = self.chunks.last_mut() {
            last.push_str(data);
        } else {
            self.chunks.push(data.to_string());
        }
    }

    pub fn len(&self) -> usize {
        self.total_len
    }

    pub fn is_empty(&self) -> bool {
        self.total_len == 0
    }

    pub fn to_string(&self) -> String {
        self.chunks.concat()
    }

    pub fn line_count(&self) -> usize {
        if self.is_empty() { return 0; }
        let full = self.to_string();
        full.lines().count()
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        let full = self.to_string();
        let end = range.end.min(full.len());
        full[range.start.min(end)..end].to_string()
    }

    pub fn lines_range(&self, range: Range<usize>) -> Vec<String> {
        let full = self.to_string();
        full.lines().skip(range.start).take(range.end.saturating_sub(range.start)).map(|l| l.to_string()).collect()
    }

    pub fn char_at(&self, idx: usize) -> Option<char> {
        let full = self.to_string();
        full.chars().nth(idx)
    }

    pub fn chunks_count(&self) -> usize {
        self.chunks.len()
    }
}

impl Default for Rope {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutputType {
    PlainText,
    Json,
    Table,
    Log,
    Binary,
}

#[derive(Debug, Clone, Default)]
pub struct StreamMetadata {
    pub tool_name: Option<String>,
    pub exit_code: Option<i32>,
    pub bytes_received: u64,
    pub lines_received: usize,
}

#[derive(Debug, Clone)]
pub struct StreamingBlock {
    pub id: Uuid,
    pub command: String,
    pub rope: Rope,
    pub start_time: Instant,
    pub end_time: Option<Instant>,
    pub is_complete: bool,
    pub estimated_total_lines: Option<usize>,
    pub output_type: StreamOutputType,
    pub metadata: StreamMetadata,
}

impl StreamingBlock {
    pub fn new(id: Uuid, command: String) -> Self {
        Self {
            id,
            command,
            rope: Rope::new(),
            start_time: Instant::now(),
            end_time: None,
            is_complete: false,
            estimated_total_lines: None,
            output_type: StreamOutputType::PlainText,
            metadata: StreamMetadata::default(),
        }
    }

    pub fn with_output_type(mut self, output_type: StreamOutputType) -> Self {
        self.output_type = output_type;
        self
    }

    pub fn with_estimated_lines(mut self, estimate: usize) -> Self {
        self.estimated_total_lines = Some(estimate);
        self
    }

    pub fn append(&mut self, data: &str) {
        if data.is_empty() || self.is_complete { return; }
        self.rope.append(data);
        self.metadata.bytes_received += data.len() as u64;
        self.metadata.lines_received += data.matches('\n').count();
    }

    pub fn complete(&mut self, exit_code: Option<i32>) {
        if self.is_complete { return; }
        self.is_complete = true;
        self.end_time = Some(Instant::now());
        self.metadata.exit_code = exit_code;
    }

    pub fn progress(&self) -> f32 {
        match self.estimated_total_lines {
            Some(est) if est > 0 => {
                let current = self.rope.line_count();
                (current as f32 / est as f32).min(1.0)
            }
            _ => if self.is_complete { 1.0 } else { 0.0 },
        }
    }

    pub fn len(&self) -> usize {
        self.rope.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.is_empty()
    }

    pub fn duration_ms(&self) -> Option<u64> {
        let end = self.end_time.unwrap_or(Instant::now());
        Some(end.duration_since(self.start_time).as_millis() as u64)
    }

    pub fn throughput(&self) -> Option<f64> {
        let dur_ms = self.duration_ms()?;
        if dur_ms == 0 { return None; }
        let bytes = self.metadata.bytes_received as f64;
        let seconds = dur_ms as f64 / 1000.0;
        Some(bytes / seconds)
    }

    pub fn render_range(&self, viewport: Range<usize>) -> RenderChunk {
        let total_lines = self.rope.line_count();
        let lines = self.rope.lines_range(viewport.clone());
        let has_more_data = viewport.end < total_lines;
        RenderChunk {
            content: lines.join("\n"),
            total_lines,
            viewport_start: viewport.start,
            is_truncated: false,
            has_more_data,
        }
    }

    pub fn tail_lines(&self, count: usize) -> Vec<String> {
        let total = self.rope.line_count();
        let start = total.saturating_sub(count);
        self.rope.lines_range(start..total)
    }
}

pub struct RenderChunk {
    pub content: String,
    pub total_lines: usize,
    pub viewport_start: usize,
    pub is_truncated: bool,
    pub has_more_data: bool,
}

const DEFAULT_MAX_ACTIVE: usize = 16;

pub struct StreamingBlockManager {
    active_blocks: std::collections::HashMap<Uuid, StreamingBlock>,
    completed_blocks: Vec<Uuid>,
    max_active: usize,
}

impl Default for StreamingBlockManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingBlockManager {
    pub fn new() -> Self {
        Self {
            active_blocks: std::collections::HashMap::new(),
            completed_blocks: Vec::new(),
            max_active: DEFAULT_MAX_ACTIVE,
        }
    }

    pub fn with_max_active(max: usize) -> Self {
        Self { max_active: max, ..Self::new() }
    }

    pub fn start_streaming(&mut self, command: String) -> Uuid {
        let id = Uuid::new_v4();
        let block = StreamingBlock::new(id, command.clone());
        if self.active_blocks.len() >= self.max_active {
            let oldest_key = self.active_blocks.keys().next().copied();
            if let Some(key) = oldest_key {
                self.active_blocks.remove(&key);
            }
        }
        self.active_blocks.insert(id, block);
        id
    }

    pub fn append_data(&mut self, block_id: &Uuid, data: &str) {
        if let Some(block) = self.active_blocks.get_mut(block_id) {
            block.append(data);
        }
    }

    pub fn complete_streaming(&mut self, block_id: &Uuid, exit_code: Option<i32>) {
        if let Some(block) = self.active_blocks.get_mut(block_id) {
            block.complete(exit_code);
            self.completed_blocks.push(*block_id);
        }
    }

    pub fn get_active(&self, block_id: &Uuid) -> Option<&StreamingBlock> {
        self.active_blocks.get(block_id)
    }

    pub fn get_active_mut(&mut self, block_id: &Uuid) -> Option<&mut StreamingBlock> {
        self.active_blocks.get_mut(block_id)
    }

    pub fn active_block_ids(&self) -> Vec<Uuid> {
        self.active_blocks.keys().copied().collect()
    }

    pub fn active_count(&self) -> usize {
        self.active_blocks.len()
    }

    pub fn take_completed(&mut self) -> Vec<StreamingBlock> {
        let ids: Vec<Uuid> = self.completed_blocks.drain(..).collect();
        ids.into_iter().filter_map(|id| self.active_blocks.remove(&id)).collect()
    }

    pub fn cancel_streaming(&mut self, block_id: &Uuid) -> bool {
        self.active_blocks.remove(block_id).is_some()
    }

    pub fn has_active(&self, block_id: &Uuid) -> bool {
        self.active_blocks.contains_key(block_id)
    }

    pub fn is_empty(&self) -> self.active_blocks.is_empty() {
        self.active_blocks.is_empty()
    }

    pub fn total_bytes_received(&self) -> u64 {
        self.active_blocks.values().map(|b| b.metadata.bytes_received).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn rope_new_is_empty() {
        let rope = Rope::new();
        assert!(rope.is_empty());
        assert_eq!(rope.len(), 0);
    }

    #[test]
    fn rope_from_str_preserves_content() {
        let rope = Rope::from_str("hello world");
        assert_eq!(rope.len(), 11);
        assert!(!rope.is_empty());
        assert_eq!(rope.to_string(), "hello world");
    }

    #[test]
    fn rope_append_increases_length() {
        let mut rope = Rope::new();
        rope.append("hello");
        assert_eq!(rope.len(), 5);
        rope.append(" world");
        assert_eq!(rope.len(), 11);
    }

    #[test]
    fn rope_append_empty_is_noop() {
        let mut rope = Rope::from_str("hi");
        rope.append("");
        assert_eq!(rope.len(), 2);
    }

    #[test]
    fn rope_slice_returns_correct_range() {
        let rope = Rope::from_str("abcdef");
        assert_eq!(rope.slice(1..4), "bcd");
    }

    #[test]
    fn rope_line_count() {
        let rope = Rope::from_str("line1\nline2\nline3");
        assert_eq!(rope.line_count(), 3);
    }

    #[test]
    fn streaming_block_new_has_correct_defaults() {
        let id = Uuid::nil();
        let block = StreamingBlock::new(id, "cargo build".to_string());
        assert_eq!(block.id, id);
        assert_eq!(block.command, "cargo build");
        assert!(!block.is_complete);
        assert!(block.is_empty());
        assert!(block.end_time.is_none());
    }

    #[test]
    fn streaming_block_append_adds_data() {
        let mut block = StreamingBlock::new(Uuid::nil(), "echo".to_string());
        block.append("hello\n");
        block.append("world\n");
        assert_eq!(block.len(), 12);
        assert_eq!(block.metadata.lines_received, 2);
        assert_eq!(block.metadata.bytes_received, 12);
    }

    #[test]
    fn streaming_block_complete_sets_state() {
        let mut block = StreamingBlock::new(Uuid::nil(), "test".to_string());
        block.append("data");
        block.complete(Some(0));
        assert!(block.is_complete);
        assert_eq!(block.metadata.exit_code, Some(0));
        assert!(block.end_time.is_some());
        assert_eq!(block.progress(), 1.0);
    }

    #[test]
    fn streaming_block_append_after_complete_ignored() {
        let mut block = StreamingBlock::new(Uuid::nil(), "test".to_string());
        block.complete(None);
        block.append("should be ignored");
        assert!(block.is_empty());
    }

    #[test]
    fn streaming_block_progress_with_estimate() {
        let mut block = StreamingBlock::new(Uuid::nil(), "gen".to_string())
            .with_estimated_lines(100);
        assert!((block.progress() - 0.0).abs() < f32::EPSILON);
        for _ in 0..50 { block.append("a line of output data here\n"); }
        let prog = block.progress();
        assert!(prog > 0.0 && prog <= 1.0);
    }

    #[test]
    fn streaming_block_duration_and_throughput() {
        let mut block = StreamingBlock::new(Uuid::nil(), "bench".to_string());
        block.append(std::iter::repeat("x").take(1024).collect::<String>().as_str());
        thread::sleep(std::time::Duration::from_millis(10));
        block.complete(Some(0));
        assert!(block.duration_ms().unwrap() > 0);
        assert!(block.throughput().unwrap() > 0.0);
    }

    #[test]
    fn streaming_block_render_range() {
        let mut block = StreamingBlock::new(Uuid::nil(), "log".to_string());
        for i in 0..10u32 { block.append(&format!("line {}\n", i)); }
        let chunk = block.render_range(2..5);
        assert_eq!(chunk.viewport_start, 2);
        assert!(chunk.has_more_data);
        assert!(chunk.content.contains("line 2"));
    }

    #[test]
    fn streaming_block_tail_lines() {
        let mut block = StreamingBlock::new(Uuid::nil(), "tail".to_string());
        for i in 0..20u32 { block.append(&format!("line {}\n", i)); }
        let tail = block.tail_lines(3);
        assert_eq!(tail.len(), 3);
        assert!(tail.last().unwrap().contains("19"));
    }

    #[test]
    fn manager_start_streaming_returns_id() {
        let mut mgr = StreamingBlockManager::new();
        let id = mgr.start_streaming("cargo test".to_string());
        assert!(mgr.has_active(&id));
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn manager_append_data_to_block() {
        let mut mgr = StreamingBlockManager::new();
        let id = mgr.start_streaming("echo".to_string());
        mgr.append_data(&id, "hello world");
        let block = mgr.get_active(&id).unwrap();
        assert_eq!(block.len(), 11);
    }

    #[test]
    fn manager_complete_then_take() {
        let mut mgr = StreamingBlockManager::new();
        let id = mgr.start_streaming("ls".to_string());
        mgr.append_data(&id, "file1\nfile2\n");
        mgr.complete_streaming(&id, Some(0));
        assert_eq!(mgr.active_count(), 1);
        let completed = mgr.take_completed();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].metadata.exit_code, Some(0));
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn manager_cancel_removes_block() {
        let mut mgr = StreamingBlockManager::new();
        let id = mgr.start_streaming("long cmd".to_string());
        assert!(mgr.cancel_streaming(&id));
        assert!(!mgr.has_active(&id));
    }

    #[test]
    fn manager_cancel_nonexistent_returns_false() {
        let mut mgr = StreamingBlockManager::new();
        assert!(!mgr.cancel_streaming(&Uuid::nil()));
    }

    #[test]
    fn manager_max_active_evicts_oldest() {
        let mut mgr = StreamingBlockManager::with_max_active(2);
        let id1 = mgr.start_streaming("cmd1".to_string());
        let id2 = mgr.start_streaming("cmd2".to_string());
        let id3 = mgr.start_streaming("cmd3".to_string());
        assert!(!mgr.has_active(&id1));
        assert!(mgr.has_active(&id2));
        assert!(mgr.has_active(&id3));
    }

    #[test]
    fn manager_total_bytes_across_all_blocks() {
        let mut mgr = StreamingBlockManager::new();
        let id1 = mgr.start_streaming("a".to_string());
        let id2 = mgr.start_streaming("b".to_string());
        mgr.append_data(&id1, "1111");
        mgr.append_data(&id2, "22");
        assert_eq!(mgr.total_bytes_received(), 6);
    }

    #[test]
    fn stream_output_type_variants_exist() {
        let types = vec![
            StreamOutputType::PlainText,
            StreamOutputType::Json,
            StreamOutputType::Table,
            StreamOutputType::Log,
            StreamOutputType::Binary,
        ];
        assert_eq!(types.len(), 5);
    }
}
