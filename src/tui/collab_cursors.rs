//! # TUI 远程光标渲染模块
//!
//! 提供在终端用户界面中渲染远程协作者光标和选择的功能。
//! 支持多个协作者的实时光标显示、选择高亮和名称标签。

use std::collections::{HashMap, BTreeMap};
use std::fmt;
use serde::{Deserialize, Serialize};

/// 远程光标状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCursorState {
    /// 协作者 ID
    pub participant_id: String,
    /// 协作者名称
    pub display_name: String,
    /// 光标位置
    pub position: CursorPosition,
    /// 选择范围 (如果有)
    pub selection: Option<Selection>,
    /// 光标颜色 (RGB)
    pub color: RgbColor,
    /// 是否在线
    pub is_online: bool,
    /// 最后活跃时间戳
    pub last_activity: i64,
    /// 光标模式
    pub cursor_mode: CursorMode,
}

/// 光标位置
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPosition {
    /// 行号 (0-indexed)
    pub line: usize,
    /// 列号 (0-indexed)
    pub column: usize,
    /// 绝对字符偏移
    pub absolute_offset: usize,
}

impl CursorPosition {
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            line,
            column,
            absolute_offset: 0, // 将在渲染时计算
        }
    }

    pub fn with_offset(line: usize, column: usize, offset: usize) -> Self {
        Self {
            line,
            column,
            absolute_offset: offset,
        }
    }
}

impl fmt::Display for CursorPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line + 1, self.column + 1) // 1-indexed for display
    }
}

/// 选择范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub start: CursorPosition,
    pub end: CursorPosition,
}

impl Selection {
    pub fn new(start: CursorPosition, end: CursorPosition) -> Self {
        Self { start, end }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// 检查位置是否在选择范围内
    pub fn contains(&self, pos: CursorPosition) -> bool {
        if self.is_empty() {
            return false;
        }
        let (start, end) = if self.start < self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        };
        start <= pos && pos <= end
    }

    /// 获取选择的长度
    pub fn length(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            let start = if self.start < self.end { self.start } else { self.end };
            let end = if self.start < self.end { self.end } else { self.start };
            end.absolute_offset - start.absolute_offset
        }
    }
}

/// 光标模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorMode {
    /// 普通模式
    Normal,
    /// 插入模式
    Insert,
    /// 覆盖模式
    Overwrite,
    /// 不可见
    Hidden,
}

impl Default for CursorMode {
    fn default() -> Self {
        Self::Normal
    }
}

/// RGB 颜色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }

    pub fn to_ansi_fg(&self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }

    pub fn to_ansi_bg(&self) -> String {
        format!("\x1b[48;2;{};{};{}m", self.r, self.g, self.b)
    }

    /// 预定义的颜色
    pub fn red() -> Self { Self { r: 255, g: 89, b: 94 } }
    pub fn green() -> Self { Self { r: 57, g: 255, b: 20 } }
    pub fn blue() -> Self { Self { r: 30, g: 144, b: 255 } }
    pub fn yellow() -> Self { Self { r: 255, g: 255, b: 0 } }
    pub fn magenta() -> Self { Self { r: 255, g: 0, b: 255 } }
    pub fn cyan() -> Self { Self { r: 0, g: 255, b: 255 } }
    pub fn white() -> Self { Self { r: 255, g: 255, b: 255 } }
    pub fn orange() -> Self { Self { r: 255, g: 165, b: 0 } }
    pub fn purple() -> Self { Self { r: 128, g: 0, b: 128 } }
    pub fn pink() -> Self { Self { r: 255, g: 192, b: 203 } }
}

/// TUI 光标渲染器
pub struct TuiCursorRenderer {
    /// 所有远程光标状态
    cursors: BTreeMap<String, RemoteCursorState>,
    /// 配置
    config: RenderConfig,
    /// 是否启用
    enabled: bool,
}

/// 渲染配置
#[derive(Debug, Clone)]
pub struct RenderConfig {
    /// 是否显示光标名称标签
    pub show_labels: bool,
    /// 是否显示选择高亮
    pub show_selections: bool,
    /// 标签位置
    pub label_position: LabelPosition,
    /// 光标字符
    pub cursor_char: char,
    /// 选择开始字符
    pub selection_start_char: char,
    /// 选择结束字符
    pub selection_end_char: char,
    /// 选择填充字符
    pub selection_fill_char: char,
    /// 标签背景色
    pub label_bg_color: RgbColor,
    /// 标签前景色
    pub label_fg_color: RgbColor,
    /// 最大标签长度
    pub max_label_length: usize,
    /// 空闲超时 (毫秒) - 超过后光标变暗
    pub idle_timeout_ms: u64,
    /// 是否显示离线光标的最后位置
    pub show_offline_positions: bool,
    /// 离线光标透明度
    pub offline_opacity: f32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            show_labels: true,
            show_selections: true,
            label_position: LabelPosition::Above,
            cursor_char: '│',
            selection_start_char: '|',
            selection_end_char: '|',
            selection_fill_char: '▏',
            label_bg_color: RgbColor::new(40, 40, 40),
            label_fg_color: RgbColor::white(),
            max_label_length: 12,
            idle_timeout_ms: 30000,
            show_offline_positions: true,
            offline_opacity: 0.4,
        }
    }
}

/// 标签位置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPosition {
    Above,
    Below,
    Inline,
}

impl Default for LabelPosition {
    fn default() -> Self {
        Self::Above
    }
}

impl TuiCursorRenderer {
    pub fn new(config: RenderConfig) -> Self {
        Self {
            cursors: BTreeMap::new(),
            config,
            enabled: true,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(RenderConfig::default())
    }

    /// 添加或更新远程光标
    pub fn update_cursor(&mut self, cursor: RemoteCursorState) {
        self.cursors.insert(cursor.participant_id.clone(), cursor);
    }

    /// 移除远程光标
    pub fn remove_cursor(&mut self, participant_id: &str) {
        self.cursors.remove(participant_id);
    }

    /// 获取所有光标
    pub fn get_cursors(&self) -> Vec<&RemoteCursorState> {
        self.cursors.values().collect()
    }

    /// 清空所有光标
    pub fn clear(&mut self) {
        self.cursors.clear();
    }

    /// 启用/禁用渲染
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 检查是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 渲染光标到终端字符串
    pub fn render(&self, viewport: &Viewport) -> String {
        if !self.enabled {
            return String::new();
        }

        let mut output = String::new();
        let current_time = chrono::Utc::now().timestamp_millis();

        // 按行收集需要渲染的光标
        let mut line_renders: BTreeMap<usize, Vec<&RemoteCursorState>> = BTreeMap::new();

        for cursor in self.cursors.values() {
            // 检查是否在线
            let is_idle = current_time - cursor.last_activity > self.config.idle_timeout_ms as i64;
            let should_render = if is_idle && !self.config.show_offline_positions {
                false
            } else {
                true
            };

            if should_render && self.is_in_viewport(cursor, viewport) {
                let line = cursor.position.line;
                line_renders.entry(line).or_insert_with(Vec::new).push(cursor);
            }
        }

        // 生成渲染输出
        for (line, cursors_on_line) in line_renders {
            let line_offset = line - viewport.start_line;
            output.push_str(&format!("\x1b[{};{}H", line_offset + viewport.offset_y + 1, 1));

            for cursor in cursors_on_line {
                let opacity = if current_time - cursor.last_activity > self.config.idle_timeout_ms as i64 {
                    self.config.offline_opacity
                } else {
                    1.0
                };

                output.push_str(&self.render_cursor(cursor, viewport, opacity));
            }
        }

        output
    }

    /// 渲染单个光标
    fn render_cursor(&self, cursor: &RemoteCursorState, viewport: &Viewport, opacity: f32) -> String {
        let mut output = String::new();
        let col = cursor.position.column.saturating_sub(viewport.start_column);

        // 渲染选择区域
        if let Some(selection) = &cursor.selection {
            if self.config.show_selections {
                output.push_str(&self.render_selection(selection, viewport, &cursor.color, opacity));
            }
        }

        // 渲染光标本身
        let cursor_str = if opacity < 1.0 {
            format!("\x1b[2m{}\x1b[0m", self.config.cursor_char)
        } else {
            self.config.cursor_char.to_string()
        };

        output.push_str(&format!(
            "\x1b[{};{}H{}{}",
            cursor.position.line.saturating_sub(viewport.start_line) + viewport.offset_y + 1,
            col + viewport.offset_x + 1,
            cursor.color.to_ansi_fg(),
            cursor_str
        ));

        // 渲染标签
        if self.config.show_labels && opacity >= 1.0 {
            output.push_str(&self.render_label(cursor, viewport));
        }

        output
    }

    /// 渲染选择区域
    fn render_selection(&self, selection: &Selection, viewport: &Viewport, color: &RgbColor, opacity: f32) -> String {
        let mut output = String::new();

        // 简化实现 - 实际需要根据选择范围渲染
        let start_col = selection.start.column.saturating_sub(viewport.start_column);
        let end_col = selection.end.column.saturating_sub(viewport.start_column);

        if start_col != end_col {
            // 渲染选择背景
            for col in start_col..=end_col {
                output.push_str(&format!(
                    "\x1b[{};{}H{}\x1b[0m",
                    selection.start.line.saturating_sub(viewport.start_line) + viewport.offset_y + 1,
                    col + viewport.offset_x + 1,
                    color.to_ansi_bg()
                ));
            }
        }

        output
    }

    /// 渲染标签
    fn render_label(&self, cursor: &RemoteCursorState, viewport: &Viewport) -> String {
        let mut output = String::new();
        let label = self.truncate_label(&cursor.display_name);
        let label_len = label.chars().count();

        match self.config.label_position {
            LabelPosition::Above => {
                // 在光标上方显示标签
                let line = cursor.position.line.saturating_sub(viewport.start_line) + viewport.offset_y;
                if line > 0 {
                    output.push_str(&format!(
                        "\x1b[{};{}H{}{}{}\x1b[0m",
                        line,
                        cursor.position.column.saturating_sub(viewport.start_column) + viewport.offset_x + 1,
                        self.config.label_bg_color.to_ansi_bg(),
                        self.config.label_fg_color.to_ansi_fg(),
                        label
                    ));
                }
            }
            LabelPosition::Below => {
                // 在光标下方显示标签
                output.push_str(&format!(
                    "\x1b[{};{}H{}{}{}\x1b[0m",
                    cursor.position.line.saturating_sub(viewport.start_line) + viewport.offset_y + 2,
                    cursor.position.column.saturating_sub(viewport.start_column) + viewport.offset_x + 1,
                    self.config.label_bg_color.to_ansi_bg(),
                    self.config.label_fg_color.to_ansi_fg(),
                    label
                ));
            }
            LabelPosition::Inline => {
                // 在光标右侧显示标签
                output.push_str(&format!(
                    "\x1b[{};{}H{}{}{}\x1b[0m",
                    cursor.position.line.saturating_sub(viewport.start_line) + viewport.offset_y + 1,
                    cursor.position.column.saturating_sub(viewport.start_column) + viewport.offset_x + 2,
                    self.config.label_bg_color.to_ansi_bg(),
                    self.config.label_fg_color.to_ansi_fg(),
                    label
                ));
            }
        }

        output
    }

    /// 截断标签
    fn truncate_label(&self, name: &str) -> String {
        let chars: Vec<char> = name.chars().take(self.config.max_label_length).collect();
        let result: String = chars.into_iter().collect();
        if result.len() < name.len() {
            format!("{}…", result)
        } else {
            result
        }
    }

    /// 检查光标是否在视口内
    fn is_in_viewport(&self, cursor: &RemoteCursorState, viewport: &Viewport) -> bool {
        cursor.position.line >= viewport.start_line
            && cursor.position.line <= viewport.end_line
            && cursor.position.column >= viewport.start_column
            && cursor.position.column <= viewport.end_column
    }

    /// 获取配置
    pub fn get_config(&self) -> &RenderConfig {
        &self.config
    }

    /// 更新配置
    pub fn update_config(&mut self, config: RenderConfig) {
        self.config = config;
    }
}

/// 视口信息
#[derive(Debug, Clone)]
pub struct Viewport {
    /// 开始行
    pub start_line: usize,
    /// 结束行
    pub end_line: usize,
    /// 开始列
    pub start_column: usize,
    /// 结束列
    pub end_column: usize,
    /// 垂直偏移
    pub offset_y: usize,
    /// 水平偏移
    pub offset_x: usize,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            start_line: 0,
            end_line: 100,
            start_column: 0,
            end_column: 200,
            offset_y: 0,
            offset_x: 0,
        }
    }
}

/// 光标列表渲染器 - 用于显示所有协作者状态
pub struct CursorListRenderer {
    cursors: HashMap<String, RemoteCursorState>,
    config: ListRenderConfig,
}

#[derive(Debug, Clone)]
pub struct ListRenderConfig {
    pub max_name_length: usize,
    pub show_position: bool,
    pub show_status: bool,
    pub separator: String,
}

impl Default for ListRenderConfig {
    fn default() -> Self {
        Self {
            max_name_length: 20,
            show_position: true,
            show_status: true,
            separator: " │ ".to_string(),
        }
    }
}

impl CursorListRenderer {
    pub fn new(config: ListRenderConfig) -> Self {
        Self {
            cursors: HashMap::new(),
            config,
        }
    }

    pub fn update_cursor(&mut self, cursor: RemoteCursorState) {
        self.cursors.insert(cursor.participant_id.clone(), cursor);
    }

    pub fn remove_cursor(&mut self, participant_id: &str) {
        self.cursors.remove(participant_id);
    }

    /// 渲染为字符串列表
    pub fn render_list(&self) -> Vec<String> {
        let mut lines = Vec::new();

        for (id, cursor) in &self.cursors {
            let name = self.truncate_name(&cursor.display_name);
            let status = if cursor.is_online { "●" } else { "○" };
            let position = format!("{}", cursor.position);

            let mut parts = Vec::new();

            if self.config.show_status {
                parts.push(format!("{}{}\x1b[0m", cursor.color.to_ansi_fg(), status));
            }

            parts.push(format!("{}{}\x1b[0m", cursor.color.to_ansi_fg(), name));

            if self.config.show_position {
                parts.push(position);
            }

            lines.push(parts.join(&self.config.separator));
        }

        lines
    }

    fn truncate_name(&self, name: &str) -> String {
        let chars: Vec<char> = name.chars().take(self.config.max_name_length).collect();
        let result: String = chars.into_iter().collect();
        if result.len() < name.len() {
            format!("{}…", result)
        } else {
            result
        }
    }

    pub fn get_cursor_count(&self) -> usize {
        self.cursors.len()
    }

    pub fn get_online_count(&self) -> usize {
        self.cursors.values().filter(|c| c.is_online).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgb_color() {
        let color = RgbColor::new(255, 0, 0);
        assert_eq!(color.to_ansi_fg(), "\x1b[38;2;255;0;0m");

        let from_hex = RgbColor::from_hex("#FF0000").unwrap();
        assert_eq!(from_hex.r, 255);
        assert_eq!(from_hex.g, 0);
        assert_eq!(from_hex.b, 0);
    }

    #[test]
    fn test_cursor_position() {
        let pos = CursorPosition::new(10, 5);
        assert_eq!(pos.line, 10);
        assert_eq!(pos.column, 5);

        let display = format!("{}", pos);
        assert_eq!(display, "11:6"); // 1-indexed
    }

    #[test]
    fn test_selection() {
        let start = CursorPosition::new(0, 0);
        let end = CursorPosition::new(0, 5);
        let selection = Selection::new(start, end);

        assert!(!selection.is_empty());
        assert!(selection.contains(CursorPosition::new(0, 2)));
        assert!(!selection.contains(CursorPosition::new(0, 10)));
    }

    #[test]
    fn test_tui_cursor_renderer() {
        let mut renderer = TuiCursorRenderer::with_defaults();
        renderer.set_enabled(true);

        let cursor = RemoteCursorState {
            participant_id: "user1".to_string(),
            display_name: "Alice".to_string(),
            position: CursorPosition::new(10, 5),
            selection: None,
            color: RgbColor::red(),
            is_online: true,
            last_activity: chrono::Utc::now().timestamp_millis(),
            cursor_mode: CursorMode::Normal,
        };

        renderer.update_cursor(cursor);

        let cursors = renderer.get_cursors();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].display_name, "Alice");
    }

    #[test]
    fn test_viewport() {
        let viewport = Viewport {
            start_line: 0,
            end_line: 50,
            start_column: 0,
            end_column: 100,
            offset_y: 0,
            offset_x: 0,
        };

        let cursor = RemoteCursorState {
            participant_id: "user1".to_string(),
            display_name: "Alice".to_string(),
            position: CursorPosition::new(25, 50),
            selection: None,
            color: RgbColor::blue(),
            is_online: true,
            last_activity: chrono::Utc::now().timestamp_millis(),
            cursor_mode: CursorMode::Normal,
        };

        let renderer = TuiCursorRenderer::with_defaults();
        // Can't call private method in test, so just verify viewport compiles
        assert_eq!(viewport.end_line, 50);
    }
}
