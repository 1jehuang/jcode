//! TUI 渲染优化器集成
//!
//! 将 IncrementalRenderer 集成到主 TUI 渲染循环中。
//! 挂载点: src/tui/app/run_shell.rs 的 terminal.draw() 调用
//!         src/tui/app/turn.rs 的 terminal.draw() 调用
//!
//! 优化:
//! 1. 增量渲染：只重绘脏区域
//! 2. 帧率控制：确保帧间隔 >= 16ms (60fps)
//! 3. 脏区域合并：减少重绘矩形数至 ≤100
//! 4. 渲染缓存：跳过未变化内容的渲染

use crate::render_optimizer::{
    IncrementalRenderer, RenderRect, RenderStats, VirtualList
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 全局渲染优化器
static RENDERER: std::sync::OnceLock<Arc<IncrementalRenderer>> = std::sync::OnceLock::new();
static RENDER_ENABLED: AtomicBool = AtomicBool::new(true);
static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// FPS 目标
pub const TARGET_FPS: f64 = 60.0;
/// 帧预算 (16ms)
pub const FRAME_BUDGET: Duration = Duration::from_micros(16_000);

/// 初始化全局渲染优化器
pub fn init_render_optimizer() {
    let _ = RENDERER.set(Arc::new(IncrementalRenderer::new()));
    tracing::info!("TUI render optimizer initialized (target: {}fps)", TARGET_FPS);
}

/// 获取渲染优化器
pub fn renderer() -> Option<&'static Arc<IncrementalRenderer>> {
    RENDERER.get()
}

/// 启用/禁用渲染优化
pub fn set_render_enabled(enabled: bool) {
    RENDER_ENABLED.store(enabled, Ordering::Release);
}

pub fn is_render_enabled() -> bool {
    RENDER_ENABLED.load(Ordering::Acquire)
}

// ---- TUI 渲染循环集成 ----

/// 在 terminal.draw() 前调用。
/// 返回 true 表示需要重绘，false 表示跳过此帧。
pub async fn begin_render_frame() -> bool {
    if !is_render_enabled() {
        return true; // 优化禁用，总是渲染
    }

    let renderer = match RENDERER.get() {
        Some(r) => r,
        None => return true,
    };

    // 帧率控制: 确保 >= 16ms 间隔
    let _frame_start = Instant::now();
    let needs_render = renderer.begin_frame().await;

    if !needs_render {
        return false;
    }

    // 增加帧计数
    FRAME_COUNT.fetch_add(1, Ordering::Relaxed);

    // 记录帧开始时间
    needs_render
}

/// 在 terminal.draw() 后调用。
/// 报告实际渲染时间用于性能统计。
pub async fn end_render_frame(render_duration: Duration) {
    if !is_render_enabled() {
        return;
    }

    let renderer = match RENDERER.get() {
        Some(r) => r,
        None => return,
    };

    let render_time_us = render_duration.as_micros() as u64;
    let _ = renderer.end_frame(render_time_us).await;

    // 检查帧率是否达标
    if render_time_us > 16_000 {
        // 超过帧预算 (16ms)
        let overage_pct = ((render_time_us as f64 / 16_000.0) - 1.0) * 100.0;
        if overage_pct > 20.0 {
            tracing::debug!("Frame over budget: {}% over ({}us)", overage_pct as u32, render_time_us);
        }
    }
}

/// 标记 TUI 特定区域为脏 (需要重绘)
pub async fn mark_area_dirty(x: u16, y: u16, width: u16, height: u16, z_order: u8, content: &str) {
    if !is_render_enabled() {
        return;
    }
    let renderer = match RENDERER.get() {
        Some(r) => r,
        None => return,
    };
    let rect = RenderRect::new(x, y, width, height);
    let _ = renderer.mark_dirty(rect, z_order, content).await;
}

/// 在流式文本更新时调用
pub async fn on_stream_content(content: &str, area: RenderRect) {
    mark_area_dirty(area.x, area.y, area.width, area.height, 10, content).await;
}

/// 在工具执行结果返回时调用
pub async fn on_tool_result(area: RenderRect) {
    mark_area_dirty(area.x, area.y, area.width, area.height, 10, "").await;
}

/// 全屏重绘
pub async fn full_redraw(width: u16, height: u16) {
    let renderer = match RENDERER.get() {
        Some(r) => r,
        None => return,
    };
    let _ = renderer.mark_full_dirty(width, height).await;
}

/// 创建虚拟列表 (用于大型输出区域)
pub fn create_virtual_list(total: usize, view_height: u16, item_height: u16) -> VirtualList {
    VirtualList::new(total, view_height, item_height)
}

/// 获取渲染统计
pub async fn get_render_stats() -> RenderStats {
    match RENDERER.get() {
        Some(r) => r.stats().await,
        None => RenderStats::default(),
    }
}

/// 渲染性能监控后台任务
pub async fn render_monitor_loop() {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if let Some(renderer) = RENDERER.get() {
            let stats = renderer.stats().await;
            tracing::info!(
                "Render stats: fps={:.1}, frame_time={:.0}us, dirty={}, cache_hits={}",
                stats.fps,
                stats.frame_time_us,
                stats.dirty_regions_count,
                stats.cache_hits,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init() {
        init_render_optimizer();
        assert!(renderer().is_some());
    }

    #[test]
    fn test_render_enable_disable() {
        assert!(is_render_enabled());
        set_render_enabled(false);
        assert!(!is_render_enabled());
        set_render_enabled(true);
    }

    #[tokio::test]
    async fn test_create_virtual_list() {
        let list = create_virtual_list(100, 20, 1);
        assert_eq!(list.skip_rows(), 0);
    }
}
