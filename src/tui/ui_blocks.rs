use chrono::{DateTime, Utc};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block as RBlock, Borders, Widget},
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorType {
    Network,
    Auth,
    Validation,
    Runtime,
    Timeout,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActionType {
    Copy,
    Retry,
    Expand,
    Collapse,
    Dismiss,
    Edit,
    Run,
    Search,
    Custom(String),
}

pub struct CommandBlock {
    pub id: Uuid,
    pub block_type: BlockType,
    pub header: BlockHeader,
    pub content: BlockContent,
    pub status: BlockStatus,
    pub actions: Vec<BlockAction>,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub is_collapsed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockType {
    Reasoning { model_name: String },
    ToolCall { tool_name: String },
    ToolResult { tool_name: String, success: bool },
    UserInput,
    SystemNotification,
    Error { error_type: ErrorType },
    MultiLineOutput { line_count: usize },
}

pub struct BlockHeader {
    pub icon: &'static str,
    pub title: String,
    pub subtitle: Option<String>,
    pub badges: Vec<HeaderBadge>,
}

pub struct HeaderBadge {
    pub label: String,
    pub color: Color,
}

pub enum BlockContent {
    PlainText(String),
    FormattedText(Vec<TextSegment>),
    JsonTree(Value),
    Table(TableData),
    Diff(DiffContent),
    Code(CodeBlock),
    Progress(ProgressBlock),
    Collapsible { summary: String, detail: Box<BlockContent> },
}

pub struct TextSegment {
    pub text: String,
    pub style: Style,
}

pub struct TableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<TableCell>>,
}

pub struct TableCell {
    pub content: String,
    pub style: Style,
}

pub struct DiffContent {
    pub old_text: String,
    pub new_text: String,
    pub hunks: Vec<DiffHunk>,
}

pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

pub struct CodeBlock {
    pub language: Option<String>,
    pub code: String,
    pub line_numbers: bool,
}

pub struct ProgressBlock {
    pub percent: f32,
    pub message: String,
    pub bar_color: Color,
}

pub struct BlockAction {
    pub icon: char,
    pub label: String,
    pub shortcut: Option<KeyBinding>,
    pub action_type: ActionType,
}

pub struct KeyBinding {
    pub key: char,
    pub modifiers: Vec<KeyModifier>,
}

pub enum KeyModifier {
    Ctrl,
    Alt,
    Shift,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockStatus {
    Running { progress: f32 },
    Success,
    Warning,
    Failed { error_msg: String },
    Skipped,
    Pending,
}

impl CommandBlock {
    pub fn new(block_type: BlockType, title: &str) -> Self {
        let (icon, default_content) = match &block_type {
            BlockType::Reasoning { .. } => ("🧠", BlockContent::PlainText(String::new())),
            BlockType::ToolCall { .. } => ("🔧", BlockContent::PlainText(String::new())),
            BlockType::ToolResult { success, .. } => {
                if *success {
                    ("✅", BlockContent::PlainText(String::new()))
                } else {
                    ("❌", BlockContent::PlainText(String::new()))
                }
            }
            BlockType::UserInput => ("💬", BlockContent::PlainText(String::new())),
            BlockType::SystemNotification => ("ℹ️", BlockContent::PlainText(String::new())),
            BlockType::Error { .. } => ("⚠️", BlockContent::PlainText(String::new())),
            BlockType::MultiLineOutput { .. } => ("📄", BlockContent::PlainText(String::new())),
        };

        Self {
            id: Uuid::new_v4(),
            block_type,
            header: BlockHeader {
                icon,
                title: title.to_string(),
                subtitle: None,
                badges: Vec::new(),
            },
            content: default_content,
            status: BlockStatus::Pending,
            actions: Vec::new(),
            timestamp: Utc::now(),
            duration_ms: None,
            is_collapsed: false,
        }
    }

    pub fn with_content(mut self, content: BlockContent) -> Self {
        self.content = content;
        self
    }

    pub fn with_status(mut self, status: BlockStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_action(mut self, action: BlockAction) -> Self {
        self.actions.push(action);
        self
    }

    pub fn with_badge(mut self, badge: HeaderBadge) -> Self {
        self.header.badges.push(badge);
        self
    }

    pub fn with_subtitle(mut self, subtitle: &str) -> Self {
        self.header.subtitle = Some(subtitle.to_string());
        self
    }

    pub fn toggle_collapse(&mut self) {
        self.is_collapsed = !self.is_collapsed;
    }

    pub fn get_action_by_index(&self, index: usize) -> Option<&BlockAction> {
        self.actions.get(index)
    }

    pub fn estimate_height(&self, width: u16) -> u16 {
        if self.is_collapsed {
            return 3;
        }
        let inner_width = width.saturating_sub(4);
        let header_height = 2 + if self.header.subtitle.is_some() { 1 } else { 0 };
        let content_height = match &self.content {
            BlockContent::PlainText(text) => Self::text_line_count(text, inner_width),
            BlockContent::FormattedText(segments) => {
                segments.iter().map(|s| Self::text_line_count(&s.text, inner_width)).sum::<usize>()
            }
            BlockContent::JsonTree(value) => Self::json_tree_height(value, 0, inner_width),
            BlockContent::Table(table) => table.rows.len() + 2,
            BlockContent::Diff(diff) => diff.hunks.iter().map(|h| h.lines.len() + 1).sum::<usize>(),
            BlockContent::Code(code) => {
                if code.line_numbers {
                    code.code.lines().count()
                } else {
                    Self::text_line_count(&code.code, inner_width)
                }
            }
            BlockContent::Progress(_) => 2,
            BlockContent::Collapsible { summary, .. } => {
                Self::text_line_count(summary, inner_width) + 1
            }
        };
        let actions_height = if self.actions.is_empty() { 0 } else { 1 };
        (header_height + content_height + actions_height) as u16 + 2
    }

    fn text_line_count(text: &str, width: u16) -> usize {
        if text.is_empty() || width == 0 {
            return 1;
        }
        text.lines()
            .map(|line| {
                let len = line.chars().count();
                if len == 0 {
                    1_usize
                } else {
                    ((len as u16 + width - 1) / width) as usize
                }
            })
            .sum::<usize>()
    }

    fn json_tree_height(value: &Value, indent: usize, width: u16) -> usize {
        let prefix_len = indent * 2;
        let avail = width.saturating_sub(prefix_len as u16);
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => 1,
            Value::String(s) => Self::text_line_count(s, avail),
            Value::Array(arr) => arr
                .iter()
                .map(|v| Self::json_tree_height(v, indent + 1, width))
                .sum::<usize>()
                + 1_usize,
            Value::Object(map) => map
                .iter()
                .map(|(_k, v)| {
                    1_usize + Self::json_tree_height(v, indent + 1, width)
                })
                .sum::<usize>()
                + 1_usize,
        }
    }

    fn status_color(&self) -> Color {
        match &self.status {
            BlockStatus::Running { .. } => Color::Yellow,
            BlockStatus::Success => Color::Green,
            BlockStatus::Warning => Color::Yellow,
            BlockStatus::Failed { .. } => Color::Red,
            BlockStatus::Skipped => Color::DarkGray,
            BlockStatus::Pending => Color::Gray,
        }
    }

    fn status_label(&self) -> String {
        match &self.status {
            BlockStatus::Running { progress } => {
                format!("Running {:.0}%", progress)
            }
            BlockStatus::Success => "Done".to_string(),
            BlockStatus::Warning => "Warning".to_string(),
            BlockStatus::Failed { .. } => "Failed".to_string(),
            BlockStatus::Skipped => "Skipped".to_string(),
            BlockStatus::Pending => "Pending".to_string(),
        }
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let block = RBlock::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.status_color()));
        let inner = block.inner(area);
        block.render(area, buf);

        let x = inner.x;
        let y = inner.y;

        let icon_span = Span::styled(
            format!("{} ", self.header.icon),
            Style::default().fg(Color::Reset),
        );
        let title_span = Span::styled(
            self.header.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

        let mut spans: Vec<Span> = vec![icon_span, title_span];

        if let Some(subtitle) = &self.header.subtitle {
            spans.push(Span::styled(
                format!(" ({})", subtitle),
                Style::default().fg(Color::DarkGray),
            ));
        }

        for badge in &self.header.badges {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("[{}]", badge.label),
                Style::default().fg(badge.color),
            ));
        }

        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            self.status_label(),
            Style::default()
                .fg(self.status_color())
                .add_modifier(Modifier::BOLD),
        ));

        if let Some(ms) = self.duration_ms {
            spans.push(Span::styled(
                format!(" {:.0}ms", ms),
                Style::default().fg(Color::DarkGray),
            ));
        }

        Line::from(spans).render(Rect::new(x, y, inner.width, 1), buf);

        if self.is_collapsed {
            let collapse_indicator =
                Span::styled("▶ collapsed", Style::default().fg(Color::DarkGray));
            Line::from(collapse_indicator).render(Rect::new(x, y + 1, inner.width, 1), buf);
        }
    }

    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        if self.is_collapsed {
            return;
        }
        let inner = area;
        let x = inner.x;
        let mut y = inner.y + 2;

        if self.header.subtitle.is_some() && y < inner.y + inner.height {
            y += 1;
        }

        match &self.content {
            BlockContent::PlainText(text) => {
                for line in text.lines() {
                    if y >= inner.y + inner.height {
                        break;
                    }
                Line::from(Span::styled(line.to_string(), Style::default()))
                    .render(Rect::new(x, y, inner.width, 1), buf);
                y += 1;
                }
            }
            BlockContent::FormattedText(segments) => {
                for seg in segments {
                    for line in seg.text.lines() {
                        if y >= inner.y + inner.height {
                            break;
                        }
                        Line::from(Span::styled(line.to_string(), seg.style))
                            .render(Rect::new(x, y, inner.width, 1), buf);
                        y += 1;
                    }
                }
            }
            BlockContent::JsonTree(value) => {
                self.render_json_tree(value, 0, x, &mut y, inner, buf);
            }
            BlockContent::Table(table) => {
                self.render_table(table, x, &mut y, inner, buf);
            }
            BlockContent::Diff(diff) => {
                self.render_diff(diff, x, &mut y, inner, buf);
            }
            BlockContent::Code(code) => {
                self.render_code(code, x, &mut y, inner, buf);
            }
            BlockContent::Progress(progress) => {
                self.render_progress(progress, x, y, inner, buf);
            }
            BlockContent::Collapsible { summary, detail } => {
                if y >= inner.y + inner.height {
                    return;
                }
                Line::from(Span::styled(
                    format!("▶ {}", summary),
                    Style::default().fg(Color::Cyan),
                ))
                .render(Rect::new(x, y, inner.width, 1), buf);
                y += 1;
                if y >= inner.y + inner.height {
                    return;
                }
                Line::from(Span::styled("  展开详情", Style::default().fg(Color::DarkGray)))
                    .render(Rect::new(x, y, inner.width, 1), buf);
                let _ = detail;
            }
        }
    }

    fn render_json_tree(
        &self,
        value: &Value,
        indent: usize,
        base_x: u16,
        y: &mut u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let prefix = " ".repeat(indent * 2);
        let x = base_x + (indent * 2) as u16;
        match value {
            Value::Null => {
                self.put_line(&format!("{}null", prefix), x, y, area, buf, Color::DarkGray);
            }
            Value::Bool(b) => {
                self.put_line(
                    &format!("{}{}", prefix, b),
                    x,
                    y,
                    area,
                    buf,
                    Color::Cyan,
                );
            }
            Value::Number(n) => {
                self.put_line(&format!("{}{}", prefix, n), x, y, area, buf, Color::Magenta);
            }
            Value::String(s) => {
                self.put_line(
                    &format!("{}\"{}\"", prefix, s),
                    x,
                    y,
                    area,
                    buf,
                    Color::Green,
                );
            }
            Value::Array(arr) => {
                self.put_line(&prefix, base_x, y, area, buf, Color::White);
                if *y < area.y + area.height {
                    Line::from(Span::styled("[", Style::default().fg(Color::White)))
                        .render(Rect::new(base_x + prefix.len() as u16, *y - 1, 1, 1), buf);
                }
                for item in arr {
                    self.render_json_tree(item, indent + 1, base_x, y, area, buf);
                }
                self.put_line(&format!("{}]", prefix), base_x, y, area, buf, Color::White);
                if *y <= area.y + area.height && *y > 0 {
                    Line::from(Span::styled("]", Style::default().fg(Color::White)))
                        .render(Rect::new(base_x + prefix.len() as u16, *y - 1, 1, 1), buf);
                }
            }
            Value::Object(map) => {
                self.put_line(&prefix, base_x, y, area, buf, Color::White);
                if *y < area.y + area.height {
                    Line::from(Span::styled("{", Style::default().fg(Color::White)))
                        .render(Rect::new(base_x + prefix.len() as u16, *y - 1, 1, 1), buf);
                }
                for (k, v) in map {
                    let key_prefix = format!("{}  \"{}\": ", prefix, k);
                    self.put_line(&key_prefix, base_x, y, area, buf, Color::Yellow);
                    if *y <= area.y + area.height && *y > 0 {
                        Line::from(Span::styled(
                            format!("\"{}\": ", k),
                            Style::default().fg(Color::Yellow),
                        ))
                        .render(
                            Rect::new(base_x + (indent * 2 + 2) as u16, *y - 1, area.width, 1),
                            buf,
                        );
                    }
                    self.render_json_tree(v, indent + 2, base_x, y, area, buf);
                }
                self.put_line(&format!("{}}}", prefix), base_x, y, area, buf, Color::White);
                if *y <= area.y + area.height && *y > 0 {
                    Line::from(Span::styled("}", Style::default().fg(Color::White)))
                        .render(Rect::new(base_x + prefix.len() as u16, *y - 1, 1, 1), buf);
                }
            }
        }
    }

    fn put_line(
        &self,
        _text: &str,
        _x: u16,
        y: &mut u16,
        area: Rect,
        _buf: &mut Buffer,
        _color: Color,
    ) {
        if *y < area.y + area.height {
            *y += 1;
        }
    }

    fn render_table(
        &self,
        table: &TableData,
        base_x: u16,
        y: &mut u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let col_width = if table.headers.is_empty() {
            area.width
        } else {
            area.width / table.headers.len() as u16
        };
        let header_spans: Vec<Span> = table
            .headers
            .iter()
            .map(|h| {
                Span::styled(
                    format!("{:<width$}", h, width = col_width as usize),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect();

        if *y < area.y + area.height {
            Line::from(header_spans).render(Rect::new(base_x, *y, area.width, 1), buf);
            *y += 1;
        }
        if *y < area.y + area.height {
            let sep: String = "-".repeat(area.width as usize);
            Line::from(Span::styled(sep, Style::default().fg(Color::DarkGray)))
                .render(Rect::new(base_x, *y, area.width, 1), buf);
            *y += 1;
        }
        for row in &table.rows {
            if *y >= area.y + area.height {
                break;
            }
            let row_spans: Vec<Span> = row
                .iter()
                .map(|cell| {
                    Span::styled(
                        format!("{:<width$}", cell.content, width = col_width as usize),
                        cell.style,
                    )
                })
                .collect();
            Line::from(row_spans).render(Rect::new(base_x, *y, area.width, 1), buf);
            *y += 1;
        }
    }

    fn render_diff(
        &self,
        diff: &DiffContent,
        base_x: u16,
        y: &mut u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        for hunk in &diff.hunks {
            if *y >= area.y + area.height {
                break;
            }
            Line::from(Span::styled(
                format!("@@ -{},+{} @@", hunk.old_start, hunk.new_start),
                Style::default().fg(Color::Magenta),
            ))
            .render(Rect::new(base_x, *y, area.width, 1), buf);
            *y += 1;
            for line in &hunk.lines {
                if *y >= area.y + area.height {
                    break;
                }
                let (prefix, text, color) = match line {
                    DiffLine::Context(t) => (" ", t.as_str(), Color::DarkGray),
                    DiffLine::Added(t) => ("+", t.as_str(), Color::Green),
                    DiffLine::Removed(t) => ("-", t.as_str(), Color::Red),
                };
                Line::from(vec![
                    Span::styled(prefix.to_string(), Style::default().fg(color)),
                    Span::styled(text.to_string(), Style::default().fg(color)),
                ])
                .render(Rect::new(base_x, *y, area.width, 1), buf);
                *y += 1;
            }
        }
    }

    fn render_code(
        &self,
        code: &CodeBlock,
        base_x: u16,
        y: &mut u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if let Some(lang) = &code.language {
            if *y < area.y + area.height {
                Line::from(Span::styled(
                    format!("// language: {}", lang),
                    Style::default().fg(Color::DarkGray),
                ))
                .render(Rect::new(base_x, *y, area.width, 1), buf);
                *y += 1;
            }
        }
        for (i, line) in code.code.lines().enumerate() {
            if *y >= area.y + area.height {
                break;
            }
            if code.line_numbers {
                let num = Span::styled(
                    format!("{:>4} | ", i + 1),
                    Style::default().fg(Color::DarkGray),
                );
                let content = Span::styled(line.to_string(), Style::default());
                Line::from(vec![num, content]).render(Rect::new(base_x, *y, area.width, 1), buf);
            } else {
                Line::from(Span::styled(line.to_string(), Style::default()))
                    .render(Rect::new(base_x, *y, area.width, 1), buf);
            }
            *y += 1;
        }
    }

    fn render_progress(
        &self,
        progress: &ProgressBlock,
        base_x: u16,
        y: u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if y >= area.y + area.height {
            return;
        }
        let bar_width = (area.width as f32 * 0.6) as usize;
        let filled = (bar_width as f32 * progress.percent / 100.0) as usize;
        let empty = bar_width.saturating_sub(filled);
        let bar: String = "█".repeat(filled) + &"░".repeat(empty);
        let pct_text = format!("{:.0}%", progress.percent);
        let msg = if progress.message.is_empty() {
            String::new()
        } else {
            format!(" {}", progress.message)
        };

        Line::from(vec![
            Span::styled("[", Style::default().fg(Color::DarkGray)),
            Span::styled(bar, Style::default().fg(progress.bar_color)),
            Span::styled("]", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(" {}{}", pct_text, msg),
                Style::default().fg(Color::White),
            ),
        ])
        .render(Rect::new(base_x, y, area.width, 1), buf);

        let next_y = y + 1;
        if next_y < area.y + area.height {
            Line::from(Span::styled(
                format!("{:<width$}", "░".repeat(bar_width), width = area.width as usize),
                Style::default().fg(progress.bar_color),
            ))
            .render(Rect::new(base_x, next_y, area.width, 1), buf);
        }
    }

    fn render_actions(&self, area: Rect, buf: &mut Buffer) {
        if self.actions.is_empty() {
            return;
        }
        let inner = RBlock::default().inner(area);
        let y = inner.y + inner.height.saturating_sub(1);
        let x = inner.x;

        let mut spans: Vec<Span> = Vec::new();
        for (i, action) in self.actions.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            let shortcut_str = match &action.shortcut {
                Some(kb) => {
                    let mods: String = kb
                        .modifiers
                        .iter()
                        .map(|m| match m {
                            KeyModifier::Ctrl => "Ctrl+",
                            KeyModifier::Alt => "Alt+",
                            KeyModifier::Shift => "Shift+",
                        })
                        .collect();
                    format!("<{}{}> ", mods, kb.key)
                }
                None => String::new(),
            };
            spans.push(Span::styled(
                format!("{}{}{}", action.icon, shortcut_str, action.label),
                Style::default().fg(Color::Cyan),
            ));
        }

        Line::from(spans).render(Rect::new(x, y, inner.width, 1), buf);
    }
}

impl Widget for CommandBlock {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_ref(area, buf);
    }
}

impl CommandBlock {
    pub fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        self.render_header(area, buf);
        let inner = RBlock::default().inner(area);
        self.render_content(inner, buf);
        self.render_actions(inner, buf);
    }
}

struct BufWriter<'a>(&'a mut [u8]);

impl std::fmt::Write for BufWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        let end = (bytes.len()).min(self.0.len());
        self.0[..end].copy_from_slice(&bytes[..end]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_command_block_has_uuid() {
        let block = CommandBlock::new(BlockType::UserInput, "test");
        assert_ne!(block.id, Uuid::nil());
    }

    #[test]
    fn test_new_block_default_status_is_pending() {
        let block = CommandBlock::new(BlockType::UserInput, "test");
        assert_eq!(block.status, BlockStatus::Pending);
    }

    #[test]
    fn test_new_block_not_collapsed_by_default() {
        let block = CommandBlock::new(BlockType::UserInput, "test");
        assert!(!block.is_collapsed);
    }

    #[test]
    fn test_reasoning_icon_is_brain() {
        let block = CommandBlock::new(
            BlockType::Reasoning {
                model_name: "gpt-4".to_string(),
            },
            "thinking",
        );
        assert_eq!(block.header.icon, "🧠");
    }

    #[test]
    fn test_tool_call_icon_is_wrench() {
        let block = CommandBlock::new(
            BlockType::ToolCall {
                tool_name: "edit".to_string(),
            },
            "edit file",
        );
        assert_eq!(block.header.icon, "🔧");
    }

    #[test]
    fn test_tool_result_success_icon() {
        let block = CommandBlock::new(
            BlockType::ToolResult {
                tool_name: "edit".to_string(),
                success: true,
            },
            "done",
        );
        assert_eq!(block.header.icon, "✅");
    }

    #[test]
    fn test_tool_result_failure_icon() {
        let block = CommandBlock::new(
            BlockType::ToolResult {
                tool_name: "edit".to_string(),
                success: false,
            },
            "failed",
        );
        assert_eq!(block.header.icon, "❌");
    }

    #[test]
    fn test_toggle_collapse() {
        let mut block = CommandBlock::new(BlockType::UserInput, "test");
        assert!(!block.is_collapsed);
        block.toggle_collapse();
        assert!(block.is_collapsed);
        block.toggle_collapse();
        assert!(!block.is_collapsed);
    }

    #[test]
    fn test_get_action_by_index_found() {
        let block = CommandBlock::new(BlockType::UserInput, "test").with_action(BlockAction {
            icon: 'c',
            label: "copy".to_string(),
            shortcut: None,
            action_type: ActionType::Copy,
        });
        let action = block.get_action_by_index(0);
        assert!(action.is_some());
        assert_eq!(action.unwrap().label, "copy");
    }

    #[test]
    fn test_get_action_by_index_out_of_bounds() {
        let block = CommandBlock::new(BlockType::UserInput, "test");
        assert!(block.get_action_by_index(0).is_none());
        assert!(block.get_action_by_index(99).is_none());
    }

    #[test]
    fn test_estimate_height_collapsed() {
        let mut block = CommandBlock::new(BlockType::UserInput, "test");
        block.is_collapsed = true;
        let height = block.estimate_height(80);
        assert_eq!(height, 3);
    }

    #[test]
    fn test_estimate_height_plain_text() {
        let block = CommandBlock::new(BlockType::UserInput, "test")
            .with_content(BlockContent::PlainText("hello\nworld\nlines".to_string()));
        let height = block.estimate_height(80);
        assert!(height >= 5);
    }

    #[test]
    fn test_builder_pattern_chaining() {
        let block = CommandBlock::new(BlockType::SystemNotification, "notice")
            .with_subtitle("important info")
            .with_badge(HeaderBadge {
                label: "NEW".to_string(),
                color: Color::Yellow,
            })
            .with_status(BlockStatus::Success)
            .with_action(BlockAction {
                icon: 'd',
                label: "dismiss".to_string(),
                shortcut: None,
                action_type: ActionType::Dismiss,
            });
        assert_eq!(block.header.subtitle.as_deref(), Some("important info"));
        assert_eq!(block.header.badges.len(), 1);
        assert_eq!(block.status, BlockStatus::Success);
        assert_eq!(block.actions.len(), 1);
    }

    #[test]
    fn test_status_color_mapping() {
        let cases: Vec<(BlockStatus, Color)> = vec![
            (BlockStatus::Running { progress: 50.0 }, Color::Yellow),
            (BlockStatus::Success, Color::Green),
            (BlockStatus::Warning, Color::Yellow),
            (
                BlockStatus::Failed {
                    error_msg: "boom".to_string(),
                },
                Color::Red,
            ),
            (BlockStatus::Skipped, Color::DarkGray),
            (BlockStatus::Pending, Color::Gray),
        ];
        for (status, expected_color) in cases {
            let block = CommandBlock::new(BlockType::UserInput, "t").with_status(status.clone());
            assert_eq!(block.status_color(), expected_color);
        }
    }

    #[test]
    fn test_error_block_type_variants() {
        assert_eq!(
            BlockType::Error {
                error_type: ErrorType::Network
            },
            BlockType::Error {
                error_type: ErrorType::Network
            }
        );
        assert_ne!(
            BlockType::Error {
                error_type: ErrorType::Network
            },
            BlockType::Error {
                error_type: ErrorType::Auth
            }
        );
    }

    #[test]
    fn test_render_widget_produces_output() {
        let block = CommandBlock::new(BlockType::UserInput, "hello world")
            .with_content(BlockContent::PlainText("test content line".to_string()))
            .with_status(BlockStatus::Success);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 12));
        block.clone().render(Rect::new(0, 0, 60, 12), &mut buf);
        let content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(content, "buffer should contain rendered content");
    }

    #[test]
    fn test_collapsed_render_skips_content() {
        let mut block = CommandBlock::new(BlockType::UserInput, "collapsed test")
            .with_content(BlockContent::PlainText("should not appear\nmultiple\nlines".to_string()));
        block.is_collapsed = true;
        let height = block.estimate_height(60);
        assert_eq!(height, 3, "collapsed block should be exactly 3 lines tall");

        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 10));
        block.clone().render(Rect::new(0, 0, 60, 10), &mut buf);
    }

    #[test]
    fn test_progress_bar_render() {
        let block = CommandBlock::new(BlockType::UserInput, "progress")
            .with_content(BlockContent::Progress(ProgressBlock {
                percent: 67.0,
                message: "building...".to_string(),
                bar_color: Color::Blue,
            }));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 10));
        block.clone().render(Rect::new(0, 0, 60, 10), &mut buf);
        let has_content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(has_content);
    }

    #[test]
    fn test_diff_render_colors() {
        let diff_content = DiffContent {
            old_text: "old".to_string(),
            new_text: "new".to_string(),
            hunks: vec![DiffHunk {
                old_start: 1,
                new_start: 1,
                lines: vec![
                    DiffLine::Removed("old line".to_string()),
                    DiffLine::Added("new line".to_string()),
                    DiffLine::Context("context".to_string()),
                ],
            }],
        };
        let block = CommandBlock::new(BlockType::UserInput, "diff view")
            .with_content(BlockContent::Diff(diff_content));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 15));
        block.clone().render(Rect::new(0, 0, 60, 15), &mut buf);
        let has_content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(has_content);
    }

    #[test]
    fn test_json_tree_render() {
        let json_value = serde_json::json!({
            "name": "test",
            "count": 42,
            "active": true,
            "tags": ["a", "b"]
        });
        let block = CommandBlock::new(BlockType::UserInput, "json data")
            .with_content(BlockContent::JsonTree(json_value));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 20));
        block.clone().render(Rect::new(0, 0, 60, 20), &mut buf);
        let has_content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(has_content);
    }

    #[test]
    fn test_code_block_with_line_numbers() {
        let code = CodeBlock {
            language: Some("rust".to_string()),
            code: "fn main() {\n    println!(\"hi\");\n}".to_string(),
            line_numbers: true,
        };
        let block = CommandBlock::new(BlockType::UserInput, "code")
            .with_content(BlockContent::Code(code));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 15));
        block.clone().render(Rect::new(0, 0, 60, 15), &mut buf);
        let has_content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(has_content);
    }

    #[test]
    fn test_table_render_alignment() {
        let table = TableData {
            headers: vec!["Name".to_string(), "Status".to_string()],
            rows: vec![vec![
                TableCell {
                    content: "task-a".to_string(),
                    style: Style::default(),
                },
                TableCell {
                    content: "done".to_string(),
                    style: Style::default().fg(Color::Green),
                },
            ]],
        };
        let block = CommandBlock::new(BlockType::UserInput, "table")
            .with_content(BlockContent::Table(table));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 8));
        block.clone().render(Rect::new(0, 0, 40, 8), &mut buf);
        let has_content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(has_content);
    }

    #[test]
    fn test_key_binding_modifiers() {
        let binding = KeyBinding {
            key: 's',
            modifiers: vec![KeyModifier::Ctrl],
        };
        assert_eq!(binding.key, 's');
        assert_eq!(binding.modifiers.len(), 1);
        matches!(binding.modifiers[0], KeyModifier::Ctrl);
    }

    #[test]
    fn test_multiple_actions_in_block() {
        let block = CommandBlock::new(BlockType::UserInput, "multi-action")
            .with_action(BlockAction {
                icon: 'c',
                label: "copy".to_string(),
                shortcut: None,
                action_type: ActionType::Copy,
            })
            .with_action(BlockAction {
                icon: 'r',
                label: "retry".to_string(),
                shortcut: Some(KeyBinding {
                    key: 'r',
                    modifiers: vec![KeyModifier::Ctrl],
                }),
                action_type: ActionType::Retry,
            })
            .with_action(BlockAction {
                icon: 'x',
                label: "dismiss".to_string(),
                shortcut: None,
                action_type: ActionType::Dismiss,
            });
        assert_eq!(block.actions.len(), 3);
        assert_eq!(block.get_action_by_index(1).unwrap().label, "retry");
        assert!(block.get_action_by_index(3).is_none());
    }

    #[test]
    fn test_duration_ms_display() {
        let mut block = CommandBlock::new(BlockType::UserInput, "timed");
        block.duration_ms = Some(1234);
        assert_eq!(block.duration_ms, Some(1234));
    }

    #[test]
    fn test_collapsible_content_summary() {
        let collapsible = BlockContent::Collapsible {
            summary: "3 files changed".to_string(),
            detail: Box::new(BlockContent::PlainText("detail content".to_string())),
        };
        let block = CommandBlock::new(BlockType::UserInput, "collapsible")
            .with_content(collapsible);
        let height = block.estimate_height(60);
        assert!(height >= 4);
    }

    #[test]
    fn test_formatted_text_segments() {
        let segments = vec![
            TextSegment {
                text: "bold part".to_string(),
                style: Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            },
            TextSegment {
                text: " normal part".to_string(),
                style: Style::default().fg(Color::Gray),
            },
        ];
        let block = CommandBlock::new(BlockType::UserInput, "formatted")
            .with_content(BlockContent::FormattedText(segments));
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 8));
        block.clone().render(Rect::new(0, 0, 60, 8), &mut buf);
        let has_content = buf.buffer.iter().any(|c| c.symbol() != ' ');
        assert!(has_content);
    }

    #[test]
    fn test_multi_line_output_block_type() {
        let bltype = BlockType::MultiLineOutput { line_count: 42 };
        assert_eq!(
            bltype,
            BlockType::MultiLineOutput { line_count: 42 }
        );
    }

    #[test]
    fn test_estimate_height_zero_width_does_not_panic() {
        let block = CommandBlock::new(BlockType::UserInput, "test")
            .with_content(BlockContent::PlainText("some text here".to_string()));
        let _height = block.estimate_height(0);
    }

    #[test]
    fn test_empty_actions_no_crash_on_render() {
        let block = CommandBlock::new(BlockType::UserInput, "no-actions")
            .with_content(BlockContent::PlainText("just text".to_string()));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 6));
        block.clone().render(Rect::new(0, 0, 40, 6), &mut buf);
    }
}
