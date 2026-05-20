# TUI Completion Prefetch Integration Guide

## 概述

本文档说明如何在 TUI 编辑器中集成补全预取钩子，实现光标移动时自动触发后台预取。

## 已完成的准备工作

✅ **已完成**:
1. `src/tui/completion_helper.rs` - 预取状态管理器
2. `jcode_completion::CompletionEngine` API 就绪
3. 流式预取机制已实现

## 集成步骤

### 步骤 1: 在 App 结构中添加字段

**文件**: `src/tui/app.rs`

在 `pub struct App` 中添加以下字段（约第 503 行附近）:

```rust
use crate::tui::completion_helper::CompletionPrefetchState;
use std::sync::Arc;
use jcode_completion::CompletionEngine;

pub struct App {
    // ... existing fields ...

    /// Completion engine for code suggestions
    completion_engine: Option<Arc<CompletionEngine>>,

    /// Prefetch state manager (debounce, caching)
    completion_prefetch: Arc<CompletionPrefetchState>,

    // ... rest of fields ...
}
```

### 步骤 2: 初始化预取状态

**文件**: `src/tui/app/tui_lifecycle.rs`

在 App 初始化处（约第 400 行）添加:

```rust
use crate::tui::completion_helper::CompletionPrefetchState;

// In the App initialization block:
App {
    // ... existing fields ...

    completion_engine: None, // Will be set later when provider is available
    completion_prefetch: Arc::new(CompletionPrefetchState::new(200)), // 200ms debounce

    // ... rest of fields ...
}
```

### 步骤 3: 创建 Completion Engine

**文件**: `src/tui/app/tui_lifecycle.rs` 或启动逻辑中

在 Provider 初始化后添加:

```rust
use jcode_completion::{CompletionEngine, LspAstProvider};

// After provider is created:
let lsp_manager = None; // Or create actual LSP manager if available
let storage_path = Some(std::path::PathBuf::from("~/.jcode/completion"));

let engine = Arc::new(CompletionEngine::new(
    Box::new(provider.clone()), // Your LLM provider
    lsp_manager,
    storage_path,
));

app.completion_engine = Some(engine);
```

### 步骤 4: 在光标移动时触发预取

**文件**: `src/tui/app/input.rs`

在键盘事件处理函数中找到光标移动的处理位置，添加:

```rust
use crossterm::event::{KeyCode, KeyModifiers};

// In the key handling function where cursor moves:
async fn handle_cursor_movement(
    app: &mut App,
    new_line: usize,
    new_column: usize,
) {
    // ... existing cursor update logic ...

    // Trigger completion prefetch
    if let Some(ref engine) = app.completion_engine {
        let file = app.session.current_file().to_string();
        let content = app.session.current_content();

        app.completion_prefetch.trigger_prefetch(
            engine.clone(),
            file,
            content,
            new_line,
            new_column,
        ).await;
    }
}
```

### 步骤 5: 在文本输入时触发预取

**文件**: `src/tui/app/input.rs`

在字符输入处理中添加:

```rust
// When user types a character:
async fn handle_char_input(app: &mut App, ch: char) {
    // ... existing input handling ...

    // Trigger prefetch after typing (with debounce)
    if let Some(ref engine) = app.completion_engine {
        let cursor_pos = app.cursor_pos;
        let (line, col) = app.cursor_position(); // Helper to convert to line/col

        let file = app.session.current_file().to_string();
        let content = app.session.current_content();

        app.completion_prefetch.trigger_prefetch(
            engine.clone(),
            file,
            content,
            line,
            col,
        ).await;
    }
}
```

### 步骤 6: 显示补全建议（可选）

创建补全 popup 组件：

```rust
// src/tui/ui/completion_popup.rs
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};
use jcode_completion::RankedCandidate;

pub fn render_completion_popup(
    frame: &mut Frame,
    completions: &[RankedCandidate],
    area: Rect,
) {
    if completions.is_empty() {
        return;
    }

    let popup_area = center(area, Constraint::Percentage(50), Constraint::Length(10));
    let block = Block::default()
        .title(" Completions ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Rgb(30, 30, 40)));

    let items: Vec<ListItem> = completions.iter()
        .take(5)
        .map(|c| {
            ListItem::new(format!(
                "{} ({:.0}%)",
                c.candidate.label,
                c.rank_score * 100.0
            ))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([vertical])
        .flex(ratatui::layout::Flex::Center)
        .areas(area);
    let [area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([horizontal])
        .flex(ratatui::layout::Flex::Center)
        .areas(area);
    area
}
```

## 性能优化建议

### 1. 调整 Debounce 间隔

```rust
// For faster response (more aggressive prefetching):
CompletionPrefetchState::new(100) // 100ms

// For lower CPU usage (less frequent):
CompletionPrefetchState::new(500) // 500ms
```

### 2. 条件性预取

只在特定条件下触发预取，避免浪费资源：

```rust
// Only prefetch in code files
if file.ends_with(".rs") || file.ends_with(".ts") || file.ends_with(".py") {
    trigger_prefetch(...);
}

// Don't prefetch in comments or strings
if !is_in_comment_or_string(app.cursor_pos) {
    trigger_prefetch(...);
}
```

### 3. 缓存策略

利用已有的 `StreamingPrefetcher` 缓存：

```rust
// The completion engine already has LRU cache
// No additional caching needed at TUI level
```

## 监控和调试

### 启用详细日志

```bash
export RUST_LOG=jcode_completion=debug,tui::completion_helper=debug
```

### 查看预取统计

在 TUI 中添加调试命令（如 `Ctrl+P`）：

```rust
if let Some(ref engine) = app.completion_engine {
    let stats = engine.get_prefetch_stats();
    app.show_status(&format!(
        "Completion Cache: hit_rate={:.1}%, size={}",
        stats.hit_rate * 100.0,
        stats.cache_size
    ));
}
```

## 测试清单

- [ ] 光标移动时不阻塞 UI
- [ ] 预取频率符合预期（检查日志）
- [ ] 缓存命中率 >50%（运行一段时间后）
- [ ] 内存占用稳定（无泄漏）
- [ ] CPU 使用率 <5%（空闲时）

## 故障排查

### 问题 1: 预取太频繁

**症状**: CPU 使用率高，日志刷屏

**解决**: 增加 debounce 间隔
```rust
CompletionPrefetchState::new(500) // 从 200ms 增加到 500ms
```

### 问题 2: 缓存命中率低

**症状**: `hit_rate < 30%`

**解决**:
1. 检查是否在不同文件间频繁切换
2. 增加缓存大小（在 `streaming_prefetch.rs` 中）
3. 降低预测阈值

### 问题 3: UI 卡顿

**症状**: 光标移动时有延迟

**解决**: 确保 `trigger_prefetch` 是异步的，不阻塞主线程
```rust
// Correct: spawn background task
tokio::spawn(async move { ... });

// Wrong: await in main thread
engine.complete(...).await; // This blocks!
```

## 下一步

1. 按照上述步骤修改代码
2. 运行 `cargo check` 验证编译
3. 启动 TUI 并测试预取功能
4. 收集性能数据并调优参数

---

*文档版本: v1.0*
*创建日期: 2026-05-21*
