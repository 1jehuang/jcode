//! jcode perf — 性能指标命令
//!
//! [I-10] 显示 3 个优化器的实时性能数据:
//! - 缓存命中率 (目标 >85%)
//! - 并发 P99 延迟 (目标 <2000ms)
//! - 渲染帧率 (目标 >60fps)

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;
use async_trait::async_trait;

pub struct PerfCommand;

#[async_trait]
impl Command for PerfCommand {
    fn name(&self) -> &str { "perf" }
    fn description(&self) -> &str {
        "Show performance metrics: cache hit rate, concurrency P99, render FPS"
    }
    fn aliases(&self) -> &[&str] { &["performance", "stats"] }

    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        let mut output = String::from("━━━ 性能指标 ━━━\n\n");

        // 1. 缓存优化器
        output.push_str("📦 缓存优化器\n");
        if let Some(cache) = crate::cache_integration::cache_optimizer() {
            let stats = cache.stats().await;
            let hit_rate = stats.hit_rate() * 100.0;
            let hit_icon = if hit_rate >= 85.0 { "✅" } else if hit_rate >= 50.0 { "⚠️" } else { "🔴" };
            output.push_str(&format!("  {} 命中率: {:.1}% (目标 >85%)\n", hit_icon, hit_rate));
            output.push_str(&format!("  总请求: {}\n", stats.total_requests));
            output.push_str(&format!("  命中: {}  未命中: {}\n", stats.hits, stats.misses));
            output.push_str(&format!("  平均延迟: {:.0}μs\n", stats.avg_latency_us));
            output.push_str(&format!("  内存: {:.1}MB\n", stats.memory_usage_mb));
        } else {
            output.push_str("  ⚪ 未初始化\n");
        }
        output.push('\n');

        // 2. 并发优化器
        output.push_str("⚡ 并发优化器\n");
        if let Some(_opt) = crate::agent::concurrency_integration::concurrency_optimizer() {
            let stats = crate::agent::concurrency_integration::get_concurrency_stats().await;
            let p99_icon = if stats.p99_latency_ms < 2000 { "✅" } else { "🔴" };
            output.push_str(&format!("  {} P99延迟: {}ms (目标 <2000ms)\n", p99_icon, stats.p99_latency_ms));
            output.push_str(&format!("  平均延迟: {:.0}ms\n", stats.avg_latency_ms));
            output.push_str(&format!("  总请求: {}\n", stats.total_requests));
            output.push_str(&format!("  合并请求: {}\n", stats.merged_requests));
            output.push_str(&format!("  节流请求: {}\n", stats.throttled_requests));
            output.push_str(&format!("  活跃连接: {}\n", stats.active_connections));
            output.push_str(&format!("  队列深度: {}\n", stats.queue_depth));
        } else {
            output.push_str("  ⚪ 未初始化\n");
        }
        output.push('\n');

        // 3. 渲染优化器
        output.push_str("🎨 渲染优化器\n");
        let render = crate::tui::render_integration::get_render_stats().await;
        let fps_icon = if render.fps >= 60.0 { "✅" } else if render.fps >= 30.0 { "⚠️" } else { "🔴" };
        output.push_str(&format!("  {} FPS: {:.0} (目标 >60)\n", fps_icon, render.fps));
        output.push_str(&format!("  帧时间: {:.0}μs (预算 16ms)\n", render.frame_time_us));
        output.push_str(&format!("  脏区域数: {}\n", render.dirty_regions_count));
        output.push_str(&format!("  缓存命中: {}\n", render.cache_hits));
        output.push_str(&format!("  缓存未命中: {}\n", render.cache_misses));
        output.push_str(&format!("  渲染时间: {:.0}μs\n", render.render_time_us));

        // 4. 开启/关闭控制
        output.push_str("\n━━━ 控制 ━━━\n");
        output.push_str(&format!(
            "缓存: {} | 并发: {} | 渲染: {}\n",
            if crate::cache_integration::is_cache_enabled() { "🟢 开启" } else { "🔴 关闭" },
            if crate::agent::concurrency_integration::is_concurrency_enabled() { "🟢 开启" } else { "🔴 关闭" },
            if crate::tui::render_integration::is_render_enabled() { "🟢 开启" } else { "🔴 关闭" },
        ));
        output.push_str("\n使用:\n");
        output.push_str("  perf toggle cache    — 切换缓存\n");
        output.push_str("  perf toggle concurrency — 切换并发控制\n");
        output.push_str("  perf toggle render   — 切换渲染优化\n");

        Ok(CommandResult::success(output))
    }
}
