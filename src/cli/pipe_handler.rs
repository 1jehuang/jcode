//! 管道输入处理
//!
//! Claude Code兼容: `cat file | carpai -p "query"` 或 `echo text | carpai`
//!
//! ## 功能特性
//! 1. 自动检测管道输入
//! 2. 支持多种输入格式 (文本/JSON/代码)
//! 3. 智能内容类型检测
//! 4. 大文件流式处理

use anyhow::Result;
use std::io::{self, Read};

/// 管道输入配置
#[derive(Debug, Clone)]
pub struct PipeConfig {
    /// 最大读取大小 (bytes)
    pub max_size: usize,
    
    /// 是否自动检测内容类型
    pub auto_detect_type: bool,
    
    /// 编码 (默认UTF-8)
    pub encoding: String,
}

impl Default for PipeConfig {
    fn default() -> Self {
        Self {
            max_size: 10 * 1024 * 1024, // 10MB
            auto_detect_type: true,
            encoding: "utf-8".to_string(),
        }
    }
}

/// 管道输入结果
#[derive(Debug, Clone)]
pub struct PipeInput {
    /// 原始内容
    pub content: String,
    
    /// 检测到的内容类型
    pub content_type: ContentType,
    
    /// 内容大小 (bytes)
    pub size: usize,
    
    /// 行数
    pub line_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentType {
    PlainText,
    Code(String), // 语言名称
    Json,
    Xml,
    Markdown,
    Log,
    Binary,
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlainText => write!(f, "纯文本"),
            Self::Code(lang) => write!(f, "代码({})", lang),
            Self::Json => write!(f, "JSON"),
            Self::Xml => write!(f, "XML"),
            Self::Markdown => write!(f, "Markdown"),
            Self::Log => write!(f, "日志"),
            Self::Binary => write!(f, "二进制"),
        }
    }
}

/// 处理管道输入
pub fn handle_pipe_input(config: Option<PipeConfig>) -> Result<Option<PipeInput>> {
    let config = config.unwrap_or_default();
    
    // 检查是否有管道输入
    if atty::is(atty::Stream::Stdin) {
        return Ok(None); // stdin是终端，没有管道输入
    }
    
    // 读取所有输入
    let mut content = String::new();
    let mut buffer = [0u8; 8192]; // 8KB缓冲区
    let mut total_read = 0usize;
    
    loop {
        match io::stdin().read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                total_read += n;
                
                if total_read > config.max_size {
                    anyhow::bail!(
                        "管道输入过大: {} bytes (最大限制: {} bytes)\n\
                         提示: 使用 --pipe-max-size 增加限制，或处理更小的输入",
                        total_read,
                        config.max_size
                    );
                }
                
                // 尝试解码为UTF-8
                let chunk = String::from_utf8_lossy(&buffer[..n]);
                content.push_str(&chunk);
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                anyhow::bail!("读取管道输入失败: {}", e);
            }
        }
    }
    
    // 清理末尾换行
    while content.ends_with('\n') || content.ends_with('\r') {
        content.pop();
    }
    
    if content.is_empty() {
        return Ok(None); // 空输入
    }
    
    // 检测内容类型
    let content_type = if config.auto_detect_type {
        detect_content_type(&content)
    } else {
        ContentType::PlainText
    };
    
    let line_count = content.lines().count();
    
    Ok(Some(PipeInput {
        content,
        content_type,
        size: total_read,
        line_count,
    }))
}

/// 检测内容类型
fn detect_content_type(content: &str) -> ContentType {
    let trimmed = content.trim();
    
    // JSON检测
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return ContentType::Json;
        }
    }
    
    // XML检测
    if trimmed.starts_with('<') && trimmed.contains('>') {
        return ContentType::Xml;
    }
    
    // Markdown检测
    if trimmed.starts_with('#') || trimmed.contains("```") {
        return ContentType::Markdown;
    }
    
    // 代码检测 (简单启发式)
    let code_patterns = [
        ("fn ", "Rust"),
        ("function ", "JavaScript"),
        ("def ", "Python"),
        ("public class", "Java"),
        ("func ", "Go"),
        ("package ", "Kotlin"),
        ("import ", "TypeScript"),
        ("#include", "C/C++"),
        ("using namespace", "C++"),
    ];
    
    for (pattern, lang) in code_patterns.iter() {
        if trimmed.contains(pattern) {
            return ContentType::Code(lang.to_string());
        }
    }
    
    // 日志检测
    let log_patterns = ["ERROR", "WARN", "INFO", "DEBUG", "[202", "Traceback"];
    if log_patterns.iter().any(|p| trimmed.contains(p)) {
        return ContentType::Log;
    }
    
    // 默认为纯文本
    ContentType::PlainText
}

/// 格式化管道输入信息 (用于调试显示)
pub fn format_pipe_info(pipe_input: &PipeInput) -> String {
    format!(
        "📥 管道输入:\n   类型: {}\n   大小: {} bytes ({:.2} KB)\n   行数: {}\n   预览: {}",
        pipe_input.content_type,
        pipe_input.size,
        pipe_input.size as f64 / 1024.0,
        pipe_input.line_count,
        truncate_preview(&pipe_input.content, 100),
    )
}

fn truncate_preview(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...", &text[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_detect_json() {
        let json = r#"{"key": "value"}"#;
        assert_eq!(detect_content_type(json), ContentType::Json);
    }
    
    #[test]
    fn test_detect_code_rust() {
        let rust_code = "fn main() {\n    println!(\"Hello\");\n}";
        assert_eq!(detect_content_type(rust_code), ContentType::Code("Rust".to_string()));
    }
    
    #[test]
    fn test_detect_markdown() {
        let md = "# Title\n\nSome **bold** text.";
        assert_eq!(detect_content_type(md), ContentType::Markdown);
    }
    
    #[test]
    fn test_detect_log() {
        let log = "ERROR: Something went wrong\nINFO: Starting up";
        assert_eq!(detect_content_type(log), ContentType::Log);
    }
}
