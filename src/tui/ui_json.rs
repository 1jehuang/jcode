use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Widget,
};
use serde_json::Value;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum JsonPath {
    Root,
    Key(String, Box<JsonPath>),
    Index(usize, Box<JsonPath>),
}

#[derive(Debug, Clone)]
pub struct JsonColorTheme {
    pub brace_color: Color,
    pub bracket_color: Color,
    pub key_color: Color,
    pub string_color: Color,
    pub number_color: Color,
    pub bool_color: Color,
    pub null_color: Color,
    pub error_color: Color,
    pub highlight_bg: Color,
}

impl Default for JsonColorTheme {
    fn default() -> Self {
        Self {
            brace_color: Color::White,
            bracket_color: Color::White,
            key_color: Color::Cyan,
            string_color: Color::Green,
            number_color: Color::Yellow,
            bool_color: Color::Magenta,
            null_color: Color::DarkGray,
            error_color: Color::Red,
            highlight_bg: Color::Rgb(255, 255, 0),
        }
    }
}

pub enum CollapseMode {
    ExpandAll,
    CollapseAll,
    Smart { threshold_bytes: usize },
}

pub struct JsonMatch {
    pub path: JsonPath,
    pub matched_text: String,
    pub context_before: String,
    pub context_after: String,
}

struct JsonNodeInfo {
    depth: usize,
    is_expanded: bool,
    is_key: bool,
    value_type: JsonValueType,
    display_text: String,
    child_count: usize,
}

enum JsonValueType {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

pub struct JsonRenderer {
    max_depth: usize,
    max_array_items: usize,
    color_theme: JsonColorTheme,
    collapse_mode: CollapseMode,
    expanded_paths: Vec<JsonPath>,
    search_query: Option<String>,
    search_matches: Vec<JsonMatch>,
}

impl JsonRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    pub fn with_theme(mut self, theme: JsonColorTheme) -> Self {
        self.color_theme = theme;
        self
    }

    pub fn toggle_path(&mut self, path: &JsonPath) {
        if let Some(pos) = self.expanded_paths.iter().position(|p| p == path) {
            self.expanded_paths.remove(pos);
        } else {
            self.expanded_paths.push(path.clone());
        }
    }

    pub fn expand_all(&mut self) {
        self.expanded_paths.clear();
        self.collapse_mode = CollapseMode::ExpandAll;
    }

    pub fn collapse_all(&mut self) {
        self.expanded_paths.clear();
        self.collapse_mode = CollapseMode::CollapseAll;
    }

    pub fn search(&mut self, query: &str) -> usize {
        self.search_query = Some(query.to_string());
        self.search_matches.clear();
        if query.is_empty() {
            return 0;
        }
        let lower = query.to_lowercase();
        self.collect_matches(&Value::Null, &JsonPath::Root, &lower);
        self.search_matches.len()
    }

    fn collect_matches(&mut self, value: &Value, path: &JsonPath, query: &str) {
        match value {
            Value::String(s) => {
                if s.to_lowercase().contains(query) {
                    self.search_matches.push(JsonMatch {
                        path: path.clone(),
                        matched_text: s.clone(),
                        context_before: String::new(),
                        context_after: String::new(),
                    });
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    if k.to_lowercase().contains(query) {
                        self.search_matches.push(JsonMatch {
                            path: path.clone(),
                            matched_text: k.clone(),
                            context_before: String::new(),
                            context_after: String::new(),
                        });
                    }
                    let child_path = JsonPath::Key(k.clone(), Box::new(path.clone()));
                    self.collect_matches(v, &child_path, query);
                }
            }
            Value::Array(arr) => {
                for (i, v) in arr.iter().enumerate() {
                    let child_path =
                        JsonPath::Index(i, Box::new(path.clone()));
                    self.collect_matches(v, &child_path, query);
                }
            }
            _ => {}
        }
    }

    pub fn clear_search(&mut self) {
        self.search_query = None;
        self.search_matches.clear();
    }

    pub fn estimate_lines(&self, json: &Value, _available_width: u16) -> usize {
        self.render_value(json, &JsonPath::Root, 0).len()
    }

    pub fn render_widget<'a>(&'a self, json: &'a Value) -> impl Widget + 'a {
        struct JsonWidget<'a> {
            renderer: &'a JsonRenderer,
            json: &'a Value,
        }
        impl Widget for JsonWidget<'_> {
            fn render(self, area: Rect, buf: &mut Buffer) {
                let lines = self.renderer.render_value(self.json, &JsonPath::Root, 0);
                for (i, line) in lines.into_iter().enumerate() {
                    if i < area.height as usize {
                        buf.set_line(area.x, area.y + i as u16, line, area.width);
                    }
                }
            }
        }
        JsonWidget {
            renderer: self,
            json,
        }
    }

    pub fn to_formatted_string(&self, json: &Value, indent: usize) -> String {
        let lines = self.render_value(json, &JsonPath::Root, 0);
        let prefix: String = " ".repeat(indent);
        lines
            .into_iter()
            .map(|l| {
                let spans: String = l
                    .spans
                    .into_iter()
                    .map(|s| s.content)
                    .collect();
                format!("{}{}", prefix, spans)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_value(
        &self,
        value: &Value,
        path: &JsonPath,
        depth: usize,
    ) -> Vec<Line<'_>> {
        match value {
            Value::Object(map) => self.render_object(map, path, depth),
            Value::Array(arr) => self.render_array(arr, path, depth),
            Value::String(s) => {
                let truncated = self.truncate_string(s, 80);
                vec![self.styled_line(
                    &format!("\"{}\"", truncated),
                    self.color_theme.string_color,
                )]
            }
            Value::Number(n) => {
                vec![self.styled_line(
                    &n.to_string(),
                    self.color_theme.number_color,
                )]
            }
            Value::Bool(b) => {
                vec![self.styled_line(
                    &b.to_string(),
                    self.color_theme.bool_color,
                )]
            }
            Value::Null => {
                vec![self.styled_line("null", self.color_theme.null_color)]
            }
        }
    }

    fn render_object(
        &self,
        obj: &serde_json::Map<String, Value>,
        path: &JsonPath,
        depth: usize,
    ) -> Vec<Line<'_>> {
        if obj.is_empty() {
            return vec![self.styled_line("{}", self.color_theme.brace_color)];
        }
        let is_expanded = self.is_expanded(path, value_byte_size(&Value::Object(obj.clone())));
        let indent = "  ".repeat(depth);
        let child_indent = "  ".repeat(depth + 1);

        let mut lines = Vec::new();
        lines.push(self.styled_line("{", self.color_theme.brace_color));

        if !is_expanded {
            lines.push(self.styled_line(
                &format!(
                    "{}// ... {} items",
                    child_indent,
                    obj.len()
                ),
                self.color_theme.null_color,
            ));
            lines.push(self.styled_line(
                &format!("{}}}", indent),
                self.color_theme.brace_color,
            ));
            return lines;
        }

        let entries: Vec<_> = obj.iter().collect();
        for (i, (key, val)) in entries.iter().enumerate() {
            let comma = if i < obj.len() - 1 { "," } else { "" };
            let key_span = Span::styled(
                format!("\"{}\"", key),
                Style::default()
                    .fg(self.color_theme.key_color),
            );
            let colon = Span::styled(": ", Style::default().fg(Color::White));
            let comma_span =
                Span::styled(comma, Style::default().fg(Color::DarkGray));

            match val {
                Value::Object(child_map)
                    if !child_map.is_empty()
                        && depth + 1 >= self.max_depth =>
                {
                    lines.push(Line::from(vec![
                        Span::raw(format!("{}", child_indent)),
                        key_span,
                        colon,
                        Span::styled(
                            format!("{{ /* {} items */ }}", child_map.len()),
                            Style::default()
                                .fg(self.color_theme.brace_color),
                        ),
                        comma_span,
                    ]));
                }
                Value::Array(child_arr)
                    if !child_arr.is_empty()
                        && depth + 1 >= self.max_depth =>
                {
                    lines.push(Line::from(vec![
                        Span::raw(format!("{}", child_indent)),
                        key_span,
                        colon,
                        Span::styled(
                            format!("[ /* {} items */ ]", child_arr.len()),
                            Style::default()
                                .fg(self.color_theme.bracket_color),
                        ),
                        comma_span,
                    ]));
                }
                Value::Object(_) | Value::Array(_) => {
                    let child_path =
                        JsonPath::Key(key.clone(), Box::new(path.clone()));
                    let mut child_lines =
                        self.render_value(val, &child_path, depth + 1);
                    if let Some(first) = child_lines.first_mut() {
                        first.spans.insert(
                            0,
                            Span::raw(format!("{}", child_indent)),
                        );
                        first.spans.insert(1, key_span);
                        first.spans.insert(2, colon);
                    }
                    if let Some(last) = child_lines.last_mut() {
                        last.spans.push(comma_span);
                    }
                    lines.extend(child_lines);
                }
                _ => {
                    let val_spans: Vec<Span> =
                        self.render_primitive_spans(val);
                    let mut row_spans = vec![
                        Span::raw(format!("{}", child_indent)),
                        key_span,
                        colon,
                    ];
                    row_spans.extend(val_spans);
                    row_spans.push(comma_span);
                    lines.push(Line::from(row_spans));
                }
            }
        }

        lines.push(self.styled_line(
            &format!("{}}}", indent),
            self.color_theme.brace_color,
        ));
        lines
    }

    fn render_array(
        &self,
        arr: &[Value],
        path: &JsonPath,
        depth: usize,
    ) -> Vec<Line<'_>> {
        if arr.is_empty() {
            return vec![self.styled_line("[]", self.color_theme.bracket_color)];
        }
        let is_expanded = self.is_expanded(path, value_byte_size(&Value::Array(arr.to_vec())));
        let indent = "  ".repeat(depth);
        let child_indent = "  ".repeat(depth + 1);

        let mut lines = Vec::new();
        lines.push(self.styled_line("[", self.color_theme.bracket_color));

        if !is_expanded {
            lines.push(self.styled_line(
                &format!("{}// ... {} items", child_indent, arr.len()),
                self.color_theme.null_color,
            ));
            lines.push(self.styled_line(
                &format!("{}]", child_indent),
                self.color_theme.bracket_color,
            ));
            return lines;
        }

        let display_end = if arr.len() > self.max_array_items {
            self.max_array_items
        } else {
            arr.len()
        };

        for i in 0..display_end {
            let val = &arr[i];
            let comma = if i < arr.len() - 1 { "," } else { "" };
            let comma_span =
                Span::styled(comma, Style::default().fg(Color::DarkGray));

            match val {
                Value::Object(child_map)
                    if !child_map.is_empty()
                        && depth + 1 >= self.max_depth =>
                {
                    lines.push(Line::from(vec![
                        Span::raw(format!("{}", child_indent)),
                        Span::styled(
                            format!("{{ /* {} items */ }}", child_map.len()),
                            Style::default()
                                .fg(self.color_theme.brace_color),
                        ),
                        comma_span,
                    ]));
                }
                Value::Array(child_arr)
                    if !child_arr.is_empty()
                        && depth + 1 >= self.max_depth =>
                {
                    lines.push(Line::from(vec![
                        Span::raw(format!("{}", child_indent)),
                        Span::styled(
                            format!("[ /* {} items */ ]", child_arr.len()),
                            Style::default()
                                .fg(self.color_theme.bracket_color),
                        ),
                        comma_span,
                    ]));
                }
                Value::Object(_) | Value::Array(_) => {
                    let child_path =
                        JsonPath::Index(i, Box::new(path.clone()));
                    let mut child_lines =
                        self.render_value(val, &child_path, depth + 1);
                    if let Some(first) = child_lines.first_mut() {
                        first.spans.insert(
                            0,
                            Span::raw(format!("{}", child_indent)),
                        );
                    }
                    if let Some(last) = child_lines.last_mut() {
                        last.spans.push(comma_span);
                    }
                    lines.extend(child_lines);
                }
                _ => {
                    let mut row_spans =
                        vec![Span::raw(format!("{}", child_indent))];
                    row_spans.extend(self.render_primitive_spans(val));
                    row_spans.push(comma_span);
                    lines.push(Line::from(row_spans));
                }
            }
        }

        if arr.len() > self.max_array_items {
            let more = arr.len() - self.max_array_items;
            lines.push(self.styled_line(
                &format!("{}// ... {} more items", child_indent, more),
                self.color_theme.null_color,
            ));
        }

        lines.push(self.styled_line(
            &format!("{}]", child_indent),
            self.color_theme.bracket_color,
        ));
        lines
    }

    fn style_for_value(&self, value: &Value) -> Style {
        match value {
            Value::String(_) => Style::default().fg(self.color_theme.string_color),
            Value::Number(_) => Style::default().fg(self.color_theme.number_color),
            Value::Bool(_) => Style::default().fg(self.color_theme.bool_color),
            Value::Null => Style::default().fg(self.color_theme.null_color),
            _ => Style::default().fg(Color::White),
        }
    }

    fn should_collapse(&self, value: &Value, depth: usize) -> bool {
        match &self.collapse_mode {
            CollapseMode::CollapseAll => true,
            CollapseMode::ExpandAll => false,
            CollapseMode::Smart { threshold_bytes } => {
                depth > 2 || value_byte_size(value) > *threshold_bytes
            }
        }
    }

    fn truncate_string(&self, s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            s.to_string()
        } else {
            format!("{}...", &s[..max_len.saturating_sub(3)])
        }
    }

    fn apply_search_highlight(&self, text: &str, is_match: bool) -> Line<'_> {
        if is_match {
            Line::from(Span::styled(
                text.to_string(),
                Style::default()
                    .fg(Color::Black)
                    .bg(self.color_theme.highlight_bg),
            ))
        } else {
            Line::from(Span::raw(text.to_string()))
        }
    }

    fn is_expanded(&self, path: &JsonPath, byte_size: usize) -> bool {
        if self.expanded_paths.contains(path) {
            return true;
        }
        match &self.collapse_mode {
            CollapseMode::ExpandAll => true,
            CollapseMode::CollapseAll => false,
            CollapseMode::Smart { threshold_bytes } => byte_size <= *threshold_bytes,
        }
    }

    fn styled_line(&self, text: &str, color: Color) -> Line<'static> {
        Line::from(Span::styled(text.to_string(), Style::default().fg(color)))
    }

    fn render_primitive_spans(&self, value: &Value) -> Vec<Span<'static>> {
        match value {
            Value::String(s) => {
                let truncated = self.truncate_string(s, 80);
                vec![Span::styled(
                    format!("\"{}\"", truncated),
                    Style::default().fg(self.color_theme.string_color),
                )]
            }
            Value::Number(n) => {
                vec![Span::styled(
                    n.to_string(),
                    Style::default().fg(self.color_theme.number_color),
                )]
            }
            Value::Bool(b) => {
                vec![Span::styled(
                    b.to_string(),
                    Style::default().fg(self.color_theme.bool_color),
                )]
            }
            Value::Null => {
                vec![Span::styled(
                    "null".to_string(),
                    Style::default().fg(self.color_theme.null_color),
                )]
            }
            _ => vec![Span::raw("?")],
        }
    }
}

fn value_byte_size(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| k.len() + value_byte_size(v))
            .sum(),
        Value::Array(arr) => arr.iter().map(value_byte_size).sum(),
        Value::Number(n) => n.to_string().len(),
        Value::Bool(_) => 4 | 5,
        Value::Null => 4,
    }
}

impl Default for JsonRenderer {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_array_items: 20,
            color_theme: JsonColorTheme::default(),
            collapse_mode: CollapseMode::Smart {
                threshold_bytes: 200,
            },
            expanded_paths: vec![],
            search_query: None,
            search_matches: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_null() {
        let renderer = JsonRenderer::new();
        let json = Value::Null;
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "null");
    }

    #[test]
    fn test_render_bool_true() {
        let renderer = JsonRenderer::new();
        let json = Value::Bool(true);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "true");
    }

    #[test]
    fn test_render_number() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!(42);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "42");
    }

    #[test]
    fn test_render_string() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!("hello");
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "\"hello\"");
    }

    #[test]
    fn test_render_empty_object() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!({});
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "{}");
    }

    #[test]
    fn test_render_empty_array() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!([]);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[]");
    }

    #[test]
    fn test_render_simple_object() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!({"name": "Alice", "age": 30});
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let flat: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(flat.contains("\"name\""));
        assert!(flat.contains("\"Alice\""));
        assert!(flat.contains("\"age\""));
        assert!(flat.contains("30"));
    }

    #[test]
    fn test_render_simple_array() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!([1, 2, 3]);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let flat: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(flat.starts_with('['));
        assert!(flat.ends_with(']'));
        assert!(flat.contains('1') && flat.contains('2') && flat.contains('3'));
    }

    #[test]
    fn test_long_string_truncation() {
        let renderer = JsonRenderer::new();
        let long_str = "x".repeat(200);
        let json = serde_json::json!(long_str);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("..."));
        assert!(text.len() < 200);
    }

    #[test]
    fn test_large_array_truncation() {
        let renderer = JsonRenderer::with_max_depth(JsonRenderer::new(), 10);
        let big_arr: Vec<i32> = (0..50).collect();
        let json = serde_json::json!(big_arr);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let flat: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(flat.contains("more items"));
    }

    #[test]
    fn test_toggle_path_expand_collapse() {
        let mut renderer = JsonRenderer::new();
        let json = serde_json::json!({"a": {"b": 1}});
        let path = JsonPath::Root;
        let before = renderer.estimate_lines(&json, 80);
        renderer.toggle_path(&path);
        let after = renderer.estimate_lines(&json, 80);
        renderer.toggle_path(&path);
        let restored = renderer.estimate_lines(&json, 80);
        assert_ne!(before, 0);
        assert_eq!(after, restored);
    }

    #[test]
    fn test_search_finds_string_match() {
        let mut renderer = JsonRenderer::new();
        let json = serde_json::json!({"name": "Alice", "city": "Beijing"});
        let count = renderer.search("Alice");
        assert_eq!(count, 1);
        assert_eq!(renderer.search_matches.len(), 1);
        assert_eq!(renderer.search_matches[0].matched_text, "Alice");
    }

    #[test]
    fn test_clear_search_resets_state() {
        let mut renderer = JsonRenderer::new();
        let json = serde_json::json!({"key": "value"});
        renderer.search("value");
        assert!(!renderer.search_matches.is_empty());
        renderer.clear_search();
        assert!(renderer.search_query.is_none());
        assert!(renderer.search_matches.is_empty());
    }

    #[test]
    fn test_to_formatted_string_output() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!({"x": 1});
        let output = renderer.to_formatted_string(&json, 2);
        assert!(output.starts_with("  "));
        assert!(output.contains("{"));
        assert!(output.contains("\"x\""));
    }

    #[test]
    fn test_custom_color_theme() {
        let theme = JsonColorTheme {
            brace_color: Color::Blue,
            bracket_color: Color::Blue,
            key_color: Color::Red,
            string_color: Color::Blue,
            number_color: Color::Green,
            bool_color: Color::Cyan,
            null_color: Color::White,
            error_color: Color::Red,
            highlight_bg: Color::Black,
        };
        let renderer = JsonRenderer::with_theme(JsonRenderer::new(), theme);
        let json = serde_json::json!({"k": "v"});
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        assert!(!lines.is_empty());
        assert!(lines[0].spans.iter().any(|s| s.style.fg == Some(Color::Blue)));
    }

    #[test]
    fn test_nested_object_depth_limit() {
        let renderer = JsonRenderer::with_max_depth(JsonRenderer::new(), 2);
        let json = serde_json::json!({"a": {"b": {"c": 1}}});
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let flat: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(flat.contains("items"));
    }

    #[test]
    fn test_estimate_lines_basic() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!([1, 2, 3]);
        let count = renderer.estimate_lines(&json, 80);
        assert!(count > 0);
    }

    #[test]
    fn test_default_max_depth_is_six() {
        let renderer = JsonRenderer::new();
        assert_eq!(renderer.max_depth, 6);
    }

    #[test]
    fn test_default_max_array_items_is_twenty() {
        let renderer = JsonRenderer::new();
        assert_eq!(renderer.max_array_items, 20);
    }

    #[test]
    fn test_smart_collapse_small_object_expands() {
        let renderer = JsonRenderer::new();
        let json = serde_json::json!({"a": 1, "b": 2});
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let flat: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(flat.contains("\"a\""));
        assert!(flat.contains("\"b\""));
    }

    #[test]
    fn test_bool_false_rendering() {
        let renderer = JsonRenderer::new();
        let json = Value::Bool(false);
        let lines = renderer.render_value(&json, &JsonPath::Root, 0);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "false");
    }
}
