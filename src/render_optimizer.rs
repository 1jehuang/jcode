//! TUI 渲染优化
//!
//! 目标：帧率 >60fps, 延迟 <16ms。
//! 策略：
//! 1. 增量渲染 (只重绘变化区域)
//! 2. 虚拟列表 (只渲染可见行)
//! 3. 渲染缓存 (缓存不变区域的渲染结果)
//! 4. 双缓冲 (减少闪烁)
//! 5. 变化合并 (将短时间内的多次重绘合并为一次)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const FPS_TARGET: f64 = 60.0;
const FRAME_BUDGET_US: u64 = 16_000; // 16ms

/// 渲染区域
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// 渲染区域类型别名
pub type RenderRect = Rect;

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self { x, y, width, height }
    }

    /// 检查两个区域是否重叠
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    /// 计算面积
    pub fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

/// 脏区域标记
#[derive(Debug, Clone)]
pub struct DirtyRegion {
    pub rect: Rect,
    pub z_order: u8,         // 层级 (0=背景, 10=内容, 20=覆盖层)
    pub content_hash: u64,   // 内容哈希 (判断是否真需要重绘)
}

/// 增量渲染器
pub struct IncrementalRenderer {
    dirty_regions: Arc<RwLock<Vec<DirtyRegion>>>,
    render_cache: Arc<RwLock<HashMap<u64, String>>>,
    last_frame_time: Arc<RwLock<Instant>>,
    frame_count: Arc<RwLock<u64>>,
    stats: Arc<RwLock<RenderStats>>,
    batch_timer: Arc<RwLock<Option<Instant>>>,
}

#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    pub fps: f64,
    pub frame_time_us: f64,
    pub dirty_regions_count: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub render_time_us: f64,
}

impl IncrementalRenderer {
    pub fn new() -> Self {
        Self {
            dirty_regions: Arc::new(RwLock::new(Vec::new())),
            render_cache: Arc::new(RwLock::new(HashMap::new())),
            last_frame_time: Arc::new(RwLock::new(Instant::now())),
            frame_count: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(RenderStats::default())),
            batch_timer: Arc::new(RwLock::new(None)),
        }
    }

    /// 标记区域为脏 (需要重绘)
    pub async fn mark_dirty(&self, rect: Rect, z_order: u8, content: &str) {
        let hash = Self::compute_hash(content);
        let mut dirty = self.dirty_regions.write().await;

        // 合并重叠区域 (减少矩形数)
        let mut merged = false;
        for region in dirty.iter_mut() {
            if region.rect.intersects(&rect) && region.z_order == z_order {
                // 合并为覆盖两个区域的矩形
                let new_x = region.rect.x.min(rect.x);
                let new_y = region.rect.y.min(rect.y);
                let new_width = region.rect.x.max(rect.x + rect.width) - new_x;
                let new_height = region.rect.y.max(rect.y + rect.height) - new_y;
                region.rect = RenderRect::new(new_x, new_y, new_width, new_height);
                region.content_hash = hash;
                merged = true;
                break;
            }
        }

        if !merged {
            dirty.push(DirtyRegion { rect, z_order, content_hash: hash });
        }

        // 限制脏区域数量 (防止无限膨胀)
        if dirty.len() > 100 {
            // 合并所有脏区域为一个整屏区域
            let full = Rect::new(0, 0, u16::MAX, u16::MAX);
            dirty.clear();
            dirty.push(DirtyRegion { rect: full, z_order: 0, content_hash: hash });
        }
    }

    /// 标记整个屏幕为脏 (强制全屏重绘)
    pub async fn mark_full_dirty(&self, width: u16, height: u16) {
        let mut dirty = self.dirty_regions.write().await;
        dirty.clear();
        dirty.push(DirtyRegion {
            rect: Rect::new(0, 0, width, height),
            z_order: 0,
            content_hash: 0,
        });
    }

    /// 开始渲染帧 (返回是否需要渲染)
    pub async fn begin_frame(&self) -> bool {
        // 帧率控制：确保帧间隔 >= 16ms
        let mut last_frame = self.last_frame_time.write().await;
        let elapsed = last_frame.elapsed();
        if elapsed < Duration::from_micros(FRAME_BUDGET_US as u64) {
            return false; // 还不到下一帧的时间
        }
        *last_frame = Instant::now();

        // 批量合并：如果离上次渲染不到 4ms，等待
        let mut batch_timer = self.batch_timer.write().await;
        match *batch_timer {
            Some(t) if t.elapsed() < Duration::from_millis(4) => {
                return false; // 等待批处理窗口
            }
            _ => {
                *batch_timer = Some(Instant::now());
            }
        }

        let dirty = self.dirty_regions.read().await;
        !dirty.is_empty()
    }

    /// 结束渲染帧
    pub async fn end_frame(&self, render_time_us: u64) {
        let mut dirty = self.dirty_regions.write().await;
        dirty.clear();

        let mut frame_count = self.frame_count.write().await;
        *frame_count += 1;

        // 更新 FPS 统计
        let mut stats = self.stats.write().await;
        let elapsed = self.last_frame_time.read().await;
        let fps = 1_000_000.0 / elapsed.elapsed().as_micros().max(1) as f64;
        stats.fps = fps;
        stats.frame_time_us = render_time_us as f64;
        stats.render_time_us = render_time_us as f64;
    }

    /// 获取脏区域列表 (按 z-order 排序)
    pub async fn get_dirty_regions(&self) -> Vec<DirtyRegion> {
        let dirty = self.dirty_regions.read().await;
        let mut regions = dirty.clone();
        regions.sort_by_key(|r| r.z_order);
        regions
    }

    /// 尝试从缓存获取渲染结果
    pub async fn try_get_cached(&self, hash: u64) -> Option<String> {
        let cache = self.render_cache.read().await;
        if let Some(result) = cache.get(&hash) {
            let mut stats = self.stats.write().await;
            stats.cache_hits += 1;
            Some(result.clone())
        } else {
            let mut stats = self.stats.write().await;
            stats.cache_misses += 1;
            None
        }
    }

    /// 缓存渲染结果
    pub async fn cache_result(&self, hash: u64, rendered: String) {
        let mut cache = self.render_cache.write().await;
        cache.insert(hash, rendered);

        // 限制缓存大小
        if cache.len() > 10_000 {
            cache.clear();
        }
    }

    /// 清除渲染缓存
    pub async fn clear_cache(&self) {
        self.render_cache.write().await.clear();
    }

    /// 获取渲染统计
    pub async fn stats(&self) -> RenderStats {
        let mut stats = self.stats.write().await;
        stats.dirty_regions_count = self.dirty_regions.read().await.len();
        stats.clone()
    }

    /// 计算内容哈希 (用于缓存判断)
    fn compute_hash(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for IncrementalRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// 虚拟列表 (只渲染可见行)
pub struct VirtualList {
    total_items: usize,
    viewport_height: u16,
    item_height: u16,
    scroll_offset: usize,
    visible_range: (usize, usize),
}

impl VirtualList {
    pub fn new(total_items: usize, viewport_height: u16, item_height: u16) -> Self {
        let visible_count = (viewport_height / item_height.max(1)) as usize;
        Self {
            total_items,
            viewport_height,
            item_height,
            scroll_offset: 0,
            visible_range: (0, visible_count.min(total_items)),
        }
    }

    /// 设置滚动偏移
    pub fn scroll_to(&mut self, offset: usize) {
        let max_offset = self.total_items.saturating_sub(
            (self.viewport_height / self.item_height.max(1)) as usize,
        );
        self.scroll_offset = offset.min(max_offset);
        self.update_visible_range();
    }

    /// 滚动指定行数
    pub fn scroll_by(&mut self, delta: isize) {
        if delta > 0 {
            self.scroll_to(self.scroll_offset + delta as usize);
        } else {
            self.scroll_to(self.scroll_offset.saturating_sub(delta.unsigned_abs()));
        }
    }

    /// 获取可见范围
    pub fn visible_range(&self) -> (usize, usize) {
        self.visible_range
    }

    /// 计算跳过行数
    pub fn skip_rows(&self) -> usize {
        self.scroll_offset
    }

    /// 更新可见范围
    fn update_visible_range(&mut self) {
        let visible_count = (self.viewport_height / self.item_height.max(1)) as usize;
        let start = self.scroll_offset;
        let end = (start + visible_count).min(self.total_items);
        self.visible_range = (start, end);
    }

    /// 是否可见
    pub fn is_visible(&self, index: usize) -> bool {
        let (start, end) = self.visible_range;
        index >= start && index < end
    }

    /// 更新总数
    pub fn set_total_items(&mut self, total: usize) {
        self.total_items = total;
        self.update_visible_range();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rect_intersects() {
        let a = RenderRect::new(0, 0, 10, 10);
        let b = RenderRect::new(5, 5, 10, 10);
        let c = RenderRect::new(20, 20, 5, 5);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[tokio::test]
    async fn test_mark_dirty_full() {
        let renderer = IncrementalRenderer::new();
        renderer.mark_full_dirty(80, 24).await;
        let dirty = renderer.get_dirty_regions().await;
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].rect.width, 80);
    }

    #[tokio::test]
    async fn test_dirty_region_merge() {
        let renderer = IncrementalRenderer::new();
        renderer.mark_dirty(Rect::new(0, 0, 10, 10), 0, "a").await;
        renderer.mark_dirty(Rect::new(5, 5, 10, 10), 0, "b").await;
        let dirty = renderer.get_dirty_regions().await;
        // Should be merged into one larger rect
        assert_eq!(dirty.len(), 1);
    }

    #[test]
    fn test_virtual_list_visibility() {
        let mut list = VirtualList::new(100, 20, 1);
        assert!(list.is_visible(0));
        assert!(list.is_visible(19));
        assert!(!list.is_visible(20));

        list.scroll_to(50);
        assert!(list.is_visible(50));
        assert!(!list.is_visible(0));
    }

    #[test]
    fn test_virtual_list_scroll_by() {
        let mut list = VirtualList::new(100, 10, 1);
        list.scroll_by(5);
        assert_eq!(list.skip_rows(), 5);
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let renderer = IncrementalRenderer::new();
        let hash = IncrementalRenderer::compute_hash("test content");
        renderer.cache_result(hash, "cached result".to_string()).await;

        let result = renderer.try_get_cached(hash).await;
        assert_eq!(result, Some("cached result".to_string()));
    }
}
