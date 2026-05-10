use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct CargoMessage {
    reason: String,
    message: Option<CompilerMessage>,
    target: Option<CargoTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    name: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CompilerMessage {
    rendered: Option<String>,
    message: String,
    level: String,
    code: Option<DiagnosticCode>,
    spans: Vec<DiagnosticSpan>,
    children: Option<Vec<CompilerMessage>>,
}

#[derive(Debug, Deserialize)]
struct DiagnosticCode {
    code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct DiagnosticSpan {
    file_name: String,
    line_start: usize,
    line_end: usize,
    column_start: usize,
    column_end: usize,
    is_primary: Option<bool>,
    text: Option<Vec<SpanText>>,
    suggested_replacement: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct SpanText {
    text: String,
    highlight_start: usize,
    highlight_end: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedDiagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub lint_code: String,
    pub message: String,
    pub level: String,
    pub rendered: Option<String>,
    pub crate_name: String,
    pub source_snippet: Option<String>,
}

pub struct CargoDiagnosticParser;

impl CargoDiagnosticParser {
    pub fn new() -> Self {
        CargoDiagnosticParser
    }

    pub fn parse_file(&self, path: &Path) -> Result<Vec<ParsedDiagnostic>> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取 cargo JSON 输出文件: {:?}", path))?;
        self.parse_json(&content)
    }

    pub fn parse_json(&self, json_content: &str) -> Result<Vec<ParsedDiagnostic>> {
        let mut diagnostics = Vec::new();

        for line in json_content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Ok(msg) = serde_json::from_str::<CargoMessage>(line)
                && msg.reason == "compiler-message"
                && let Some(compiler_msg) = msg.message
                && (compiler_msg.level == "warning" || compiler_msg.level == "error")
                && let Some(parsed) =
                    Self::extract_diagnostic(&compiler_msg, &msg.target, line)
            {
                diagnostics.push(parsed);
            } else if let Ok(json_value) =
                serde_json::from_str::<serde_json::Value>(line)
                && let Some(reason) =
                    json_value.get("reason").and_then(|v| v.as_str())
                && reason == "compiler-message"
                && let Some(parsed) = Self::extract_from_value(&json_value, line)
            {
                diagnostics.push(parsed);
            }
        }

        Ok(diagnostics)
    }

    fn extract_diagnostic(
        msg: &CompilerMessage,
        target: &Option<CargoTarget>,
        _raw_json: &str,
    ) -> Option<ParsedDiagnostic> {
        let primary_span = msg.spans.iter().find(|s| s.is_primary.unwrap_or(true))?;

        let file_path = primary_span.file_name.clone();
        let line = primary_span.line_start;
        let column = primary_span.column_start;
        let lint_code = msg
            .code
            .as_ref()
            .map(|c| c.code.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let message = msg.message.clone();
        let level = msg.level.clone();
        let rendered = msg.rendered.clone();
        let crate_name = target
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let source_snippet = primary_span
            .text
            .as_ref()
            .and_then(|texts| texts.first().map(|t| t.text.clone()));

        Some(ParsedDiagnostic {
            file_path,
            line,
            column,
            lint_code,
            message,
            level,
            rendered,
            crate_name,
            source_snippet,
        })
    }

    fn extract_from_value(value: &serde_json::Value, _raw_json: &str) -> Option<ParsedDiagnostic> {
        let msg = value.get("message")?;
        let spans = msg.get("spans")?.as_array()?;
        let primary_span = spans.iter().find(|s| {
            s.get("is_primary")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        })?;

        let file_path = primary_span
            .get("file_name")
            .and_then(|v| v.as_str())?
            .to_string();
        let line = primary_span
            .get("line_start")
            .and_then(|v| v.as_u64())? as usize;
        let column = primary_span
            .get("column_start")
            .and_then(|v| v.as_u64())? as usize;
        let lint_code = msg
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let message = msg.get("message").and_then(|v| v.as_str())?.to_string();
        let level = msg.get("level").and_then(|v| v.as_str())?.to_string();
        let rendered = msg
            .get("rendered")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let crate_name = value
            .get("target")
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let source_snippet = primary_span
            .get("text")
            .and_then(|t| t.as_array())
            .and_then(|texts| texts.first())
            .and_then(|t| t.get("text"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Some(ParsedDiagnostic {
            file_path,
            line,
            column,
            lint_code,
            message,
            level,
            rendered,
            crate_name,
            source_snippet,
        })
    }
}

impl Default for CargoDiagnosticParser {
    fn default() -> Self {
        Self::new()
    }
}