//! Multi-line Completion Support
//!
//! This module extends the completion engine to support multi-line code snippets,
//! including proper handling of:
//! - Placeholder navigation (${1:name}, ${2:type})
//! - Indentation preservation
//! - Bracket matching and auto-closing
//! - Snippet expansion with context awareness

use crate::llm_candidate::CompletionCandidate;
use regex::Regex;
use std::collections::HashMap;

/// Represents a multi-line completion snippet with placeholders
#[derive(Debug, Clone)]
pub struct MultilineSnippet {
    /// The full snippet text with placeholders
    pub template: String,
    /// Resolved text (placeholders replaced)
    pub resolved: String,
    /// Placeholder positions for editor navigation
    pub placeholders: Vec<Placeholder>,
    /// Number of lines in the snippet
    pub line_count: usize,
}

/// A placeholder within a snippet (e.g., ${1:name})
#[derive(Debug, Clone)]
pub struct Placeholder {
    pub tab_stop: usize,
    pub name: String,
    pub default_value: String,
    pub start_pos: usize,
    pub end_pos: usize,
}

/// Multi-line completion generator
pub struct MultilineCompleter {
    /// Regex for parsing LSP-style placeholders: ${1:text}, ${2}, etc.
    placeholder_regex: Regex,
    /// Common code templates for different contexts
    templates: HashMap<String, Vec<String>>,
}

impl MultilineCompleter {
    pub fn new() -> Self {
        let placeholder_regex = Regex::new(r"\$\{(\d+)(?::([^}]*))?\}").unwrap();

        let mut completer = Self {
            placeholder_regex,
            templates: HashMap::new(),
        };

        // Initialize common templates
        completer.initialize_templates();

        completer
    }

    /// Convert a single-line completion to multi-line snippet
    pub fn expand_to_multiline(&self, candidate: &CompletionCandidate, context: &str) -> MultilineSnippet {
        let text = &candidate.text;

        // Check if already multi-line
        if text.contains('\n') {
            return self.parse_snippet(text.to_string());
        }

        // Try to expand based on context
        if let Some(expanded) = self.expand_from_template(context, text) {
            return expanded;
        }

        // Otherwise, wrap as single-line snippet
        self.parse_snippet(text.clone())
    }

    /// Parse a snippet template and extract placeholders
    pub fn parse_snippet(&self, template: String) -> MultilineSnippet {
        let mut placeholders = Vec::new();
        let mut resolved = template.clone();
        let mut offset_adjustment = 0i32;

        for cap in self.placeholder_regex.captures_iter(&template) {
            let tab_stop: usize = cap[1].parse().unwrap_or(0);
            let default_value = cap.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
            let name = format!("placeholder_{}", tab_stop);

            let full_match = cap.get(0).unwrap();
            let start = full_match.start() as i32 - offset_adjustment;
            let end = full_match.end() as i32 - offset_adjustment;

            placeholders.push(Placeholder {
                tab_stop,
                name,
                default_value: default_value.clone(),
                start_pos: start as usize,
                end_pos: end as usize,
            });

            // Replace placeholder with default value
            let replacement = if default_value.is_empty() {
                "".to_string()
            } else {
                default_value
            };

            let adjusted_start = full_match.start() as i32 - offset_adjustment;
            let adjusted_end = full_match.end() as i32 - offset_adjustment;
            let range_start = adjusted_start as usize;
            let range_end = adjusted_end as usize;

            resolved.replace_range(range_start..range_end, &replacement);
            offset_adjustment += (full_match.len() as i32) - (replacement.len() as i32);
        }

        // Sort placeholders by tab stop order
        placeholders.sort_by_key(|p| p.tab_stop);

        let line_count = resolved.lines().count();

        MultilineSnippet {
            template,
            resolved,
            placeholders,
            line_count,
        }
    }

    /// Get the next placeholder position for tab navigation
    pub fn get_next_placeholder<'a>(
        &self,
        snippet: &'a MultilineSnippet,
        current_tab_stop: usize,
    ) -> Option<&'a Placeholder> {
        snippet.placeholders.iter().find(|p| p.tab_stop > current_tab_stop)
    }

    /// Apply user input to a placeholder
    pub fn apply_placeholder_value(
        &self,
        snippet: &mut MultilineSnippet,
        tab_stop: usize,
        value: &str,
    ) {
        // First pass: find the target placeholder and compute offset
        let mut target_info: Option<(usize, usize, usize)> = None; // (start_pos, old_len, index)
        for (i, p) in snippet.placeholders.iter().enumerate() {
            if p.tab_stop == tab_stop {
                target_info = Some((p.start_pos, p.end_pos - p.start_pos, i));
                break;
            }
        }

        if let Some((start_pos, old_len, index)) = target_info {
            let new_len = value.len();
            let offset_diff = new_len as i32 - old_len as i32;

            snippet.resolved.replace_range(
                start_pos..start_pos + old_len,
                value,
            );

            // Update subsequent placeholder positions
            for (i, other) in snippet.placeholders.iter_mut().enumerate() {
                if i == index {
                    other.default_value = value.to_string();
                    other.end_pos = start_pos + new_len;
                } else if other.start_pos > start_pos {
                    other.start_pos = (other.start_pos as i32 + offset_diff) as usize;
                    other.end_pos = (other.end_pos as i32 + offset_diff) as usize;
                }
            }
        }
    }

    /// Initialize common code templates
    fn initialize_templates(&mut self) {
        // Rust function template
        self.templates.insert(
            "fn".to_string(),
            vec![
                "fn ${1:name}(${2:params}) -> ${3:ReturnType} {\n    ${4:// body}\n}".to_string(),
            ],
        );

        // Rust struct template
        self.templates.insert(
            "struct".to_string(),
            vec![
                "struct ${1:Name} {\n    ${2:field}: ${3:Type},\n}".to_string(),
            ],
        );

        // Rust impl block
        self.templates.insert(
            "impl".to_string(),
            vec![
                "impl ${1:Type} {\n    ${2:// methods}\n}".to_string(),
            ],
        );

        // For loop
        self.templates.insert(
            "for".to_string(),
            vec![
                "for ${1:item} in ${2:collection} {\n    ${3:// body}\n}".to_string(),
            ],
        );

        // Match expression
        self.templates.insert(
            "match".to_string(),
            vec![
                "match ${1:expr} {\n    ${2:pattern} => ${3:result},\n    _ => ${4:default},\n}".to_string(),
            ],
        );

        // If-else
        self.templates.insert(
            "if".to_string(),
            vec![
                "if ${1:condition} {\n    ${2:// then}\n} else {\n    ${3:// else}\n}".to_string(),
            ],
        );

        // Iterator chain
        self.templates.insert(
            "iter".to_string(),
            vec![
                "${1:collection}.iter()\n    .map(|${2:x}| ${3:x})\n    .filter(|${4:x}| ${5:true})\n    .collect::<Vec<_>>()".to_string(),
            ],
        );

        // Error handling with Result
        self.templates.insert(
            "result".to_string(),
            vec![
                "fn ${1:name}(${2:params}) -> Result<${3:OkType}, ${4:ErrType}> {\n    ${5:// implementation}\n    Ok(${6:value})\n}".to_string(),
            ],
        );
    }

    /// Expand completion using templates
    fn expand_from_template(&self, context: &str, trigger: &str) -> Option<MultilineSnippet> {
        // Find matching template
        let templates = self.templates.get(trigger)?;

        // For now, just use the first template
        // In a real implementation, use context to choose the best one
        templates.first().map(|t| self.parse_snippet(t.clone()))
    }

    /// Preserve indentation when inserting multi-line text
    pub fn preserve_indentation(&self, snippet: &str, base_indent: &str) -> String {
        let lines: Vec<&str> = snippet.lines().collect();
        if lines.is_empty() {
            return snippet.to_string();
        }

        let mut result = lines[0].to_string();
        for line in &lines[1..] {
            result.push('\n');
            result.push_str(base_indent);
            result.push_str(line.trim_start());
        }

        result
    }

    /// Detect the current indentation level from context
    pub fn detect_indentation(&self, line_content: &str, cursor_column: usize) -> String {
        // Count leading spaces/tabs up to cursor
        let prefix = &line_content[..cursor_column.min(line_content.len())];
        let indent_chars = prefix.chars().take_while(|c| c.is_whitespace()).collect::<String>();

        if indent_chars.is_empty() {
            "    ".to_string() // Default to 4 spaces
        } else {
            indent_chars
        }
    }
}

impl Default for MultilineCompleter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_candidate::CandidateKind;

    #[test]
    fn test_parse_simple_snippet() {
        let completer = MultilineCompleter::new();
        let snippet = completer.parse_snippet("fn ${1:name}() -> ${2:void} {\n    ${3:// body}\n}".to_string());

        assert_eq!(snippet.placeholders.len(), 3);
        assert_eq!(snippet.placeholders[0].tab_stop, 1);
        assert_eq!(snippet.placeholders[0].default_value, "name");
        assert_eq!(snippet.line_count, 3);
    }

    #[test]
    fn test_expand_function_template() {
        let completer = MultilineCompleter::new();
        let candidate = CompletionCandidate {
            label: "fn".to_string(),
            text: "fn".to_string(),
            detail: None,
            kind: CandidateKind::Keyword,
            score: 0.9,
        };

        let snippet = completer.expand_to_multiline(&candidate, "fn");
        assert!(snippet.line_count >= 3);
        assert!(!snippet.placeholders.is_empty());
    }

    #[test]
    fn test_preserve_indentation() {
        let completer = MultilineCompleter::new();
        let snippet = "line1\nline2\nline3";
        let indented = completer.preserve_indentation(snippet, "    ");

        assert_eq!(indented, "line1\n    line2\n    line3");
    }

    #[test]
    fn test_apply_placeholder() {
        let mut completer = MultilineCompleter::new();
        let mut snippet = completer.parse_snippet("Hello ${1:name}!".to_string());

        completer.apply_placeholder_value(&mut snippet, 1, "World");

        assert_eq!(snippet.resolved, "Hello World!");
    }
}
