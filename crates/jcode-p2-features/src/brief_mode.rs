// ════════════════════════════════════════════════════════════════
// Brief 简要输出模式 — 紧凑的 Agent 响应格式
//
// Claude Code 的 Brief Tool: 将冗长的输出压缩为关键信息摘要。
//
// 适用场景:
//   - 用户只需要结果, 不关心过程
//   - Token 节省 (减少输出长度)
//   - 终端宽度受限
//
// 策略:
//   1. 提取关键行 (错误/路径/数值)
//   2. 截断长输出
//   3. 结构化显示 (table/bullet)
//   4. 智能折叠 (默认只看第一层)
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};

/// Line format for brief output
#[derive(Debug, Clone)]
enum LineFormat {
    Keep(String),
    Truncate { head: String, tail: String },
    Skip,
}

/// Brief 格式化器配置
#[derive(Debug, Clone)]
pub struct BriefConfig {
    /// 最大行数 (0=不限制)
    pub max_lines: usize,
    
    /// 每行最大字符数
    pub max_line_width: usize,
    
    /// 是否高亮关键字
    pub highlight_keywords: bool,
    
    /// 关键字列表
    pub keywords: Vec<String>,
    
    /// 是否使用表格格式化结构化数据
    pub use_tables: bool,
    
    /// 截断时显示的省略标记
    pub truncation_marker: String,
}

impl Default for BriefConfig {
    fn default() -> Self {
        Self {
            max_lines: 50,
            max_line_width: 120,
            highlight_keywords: true,
            keywords: vec![
                "error".to_string(), "Error".to_string(), "ERROR".to_string(),
                "warning".to_string(), "Warning".to_string(),
                "success".to_string(), "Success".to_string(),
                "failed".to_string(), "Failed".to_string(),
                "->".to_string(), "==>".to_string(), "=>".to_string(),
                "/dev/".to_string(), ".rs:".to_string(), ".ts:".to_string(), ".py:".to_string(),
                "[OK]".to_string(), "[✓]".to_string(), "[✗]".to_string(),
            ],
            use_tables: true,
            truncation_marker: "...".into(),
        }
    }
}

/// 格式化后的简短输出
#[derive(Debug, Clone)]
pub struct BriefOutput {
    /// 格式化后的文本
    pub text: String,
    /// 原始大小 (行数)
    pub original_lines: usize,
    /// 压缩后大小 (行数)
    pub compressed_lines: usize,
    /// 压缩比
    pub compression_ratio: f32,
}

/// Brief 格式化器
pub struct BriefFormatter {
    config: BriefConfig,
}

impl Default for BriefFormatter {
    fn default() -> Self { Self::new() }
}

impl BriefFormatter {
    pub fn new() -> Self { Self { config: BriefConfig::default() } }
    
    pub fn with_config(config: BriefConfig) -> Self { Self { config } }

    /// 将任意文本转换为 Brief 输出
    pub fn format(&self, input: &str) -> BriefOutput {
        let original_count = input.lines().count();
        
        if original_count == 0 || original_count <= self.config.max_lines / 2 {
            // 太短不需要压缩
            return BriefOutput {
                text: input.to_string(),
                original_lines: original_count,
                compressed_lines: original_count,
                compression_ratio: 1.0,
            };
        }

        let mut processed = Vec::with_capacity(original_count.min(self.config.max_lines));
        let mut line_count = 0;

        for line in input.lines() {
            // 截断超长行
            let formatted = self.format_single_line(line);

            match formatted {
                LineFormat::Keep(l) => {
                    processed.push(l);
                    line_count += 1;
                }
                LineFormat::Truncate { head, tail } => {
                    processed.push(format!("{}{}", head, self.config.truncation_marker));
                    
                    if !tail.is_empty() && line_count < self.config.max_lines {
                        // 显示截断提示
                        // 不追加尾部内容
                    }

                    line_count += 1;
                }
                LineFormat::Skip => {} // 跳过空行或重复行
            }

            if line_count >= self.config.max_lines {
                break;
            }
        }

        // 如果还有更多未显示的内容, 追加省略提示
        if original_count > line_count {
            processed.push(format!(
                "{} [{} lines hidden of {}]",
                self.config.truncation_marker,
                original_count - line_count,
                original_count
            ));
        }

        let compressed = processed.join("\n");
        let compressed_line_count = compressed.lines().count();
        let ratio = if original_count > 0 {
            (compressed_line_count as f32 / original_count as f32) * 100.0
        } else { 100.0 };

        BriefOutput {
            text: compressed,
            original_lines: original_count,
            compressed_lines: compressed_line_count,
            compression_ratio: ratio,
        }
    }

    /// 格式化 JSON 为紧凑的 key-value 表格
    pub fn format_json(&self, json_str: &str) -> BriefOutput {
        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(value) => {
                let table = self.json_to_table(&value);
                let compressed_lines = table.lines().count();
                BriefOutput {
                    text: table,
                    original_lines: json_str.lines().count(),
                    compressed_lines,
                    compression_ratio: 1.0, // JSON 通常会膨胀
                }
            }
            Err(_) => self.format(json_str), // 非 JSON, 当作普通文本处理
        }
    }

    /// 格式化文件差异为简洁视图
    pub fn format_diff(&self, diff_text: &str) -> BriefOutput {
        let mut result_lines = Vec::new();
        let mut added = 0u32;
        let mut removed = 0u32;

        for line in diff_text.lines() {
            let trimmed = line.trim();
            
            if trimmed.starts_with('+') {
                added += 1;
                if self.config.highlight_keywords {
                    result_lines.push(format!("\x1b[32m{}\x1b[0m", line)); // Green
                } else {
                    result_lines.push(line.into());
                }
            } else if trimmed.starts_with('-') {
                removed += 1;
                if self.config.highlight_keywords {
                    result_lines.push(format!("\x1b[31m{}\x1b[0m", line)); // Red
                } else {
                    result_lines.push(line.into());
                }
            } else if trimmed.starts_with("@@") {
                // Hunk header
                result_lines.push(format!("\x1b[33m{}\x1b[0m", line)); // Yellow/Cyan
            } else {
                // Context lines (限制数量)
                result_lines.push(line.into());
            }

            if result_lines.len() >= self.config.max_lines {
                result_lines.push(self.config.truncation_marker.clone());
                break;
            }
        }

        let summary = format!("\n--- Diff Summary: +{} /-{} lines ---\n", added, removed);
        result_lines.push(summary);

        BriefOutput {
            text: result_lines.join("\n"),
            original_lines: diff_text.lines().count(),
            compressed_lines: result_lines.len(),
            compression_ratio: 1.0,
        }
    }

    // --- 内部方法 -----------------------------

    fn format_single_line(&self, line: &str) -> LineFormat {
        let trimmed = line.trim();

        // 跳过纯空白行 (但保留一个作为分隔)
        if trimmed.is_empty() {
            return LineFormat::Keep(String::new());
        }

        // 关键字高亮
        let highlighted = if self.config.highlight_keywords {
            self.highlight_line(trimmed)
        } else {
            trimmed.to_string()
        };

        if highlighted.len() > self.config.max_line_width {
            // 截断并添加省略号
            let end = if self.config.max_line_width > 3 {
                self.config.max_line_width - 3
            } else {
                self.config.max_line_width
            };

            LineFormat::Truncate {
                head: highlighted[..end].to_string(),
                tail: if highlighted.len() > end { format!("...{}", &highlighted[highlighted.len()-10..]) } else { String::new() },
            }
        } else {
            LineFormat::Keep(highlighted)
        }
    }

    fn highlight_line(&self, line: &str) -> String {
        let mut result = line.to_string();

        for kw in &self.config.keywords {
            if line.contains(kw) {
                // 用 ANSI bold 包裹关键词
                result = result.replace(
                    kw,
                    &format!("\x1b[1m{}\x1b[0m", kw),
                );
            }
        }

        result
    }

    fn json_to_table(&self, value: &serde_json::Value) -> String {
        let mut rows = Vec::new();

        match value {
            serde_json::Value::Object(map) => {
                for (key, val) in map {
                    let val_str = match val {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        serde_json::Value::Null => "null".into(),
                        other => {
                            // 递归处理嵌套对象/数组
                            let inner = self.json_to_table(other);
                            if inner.lines().count() > 3 {
                                format!("{{ ... }}")
                            } else {
                                inner.replace('\n', " | ")
                            }
                        }
                        other_val => format!("{:?}", other_val),
                    };
                    rows.push(format!("  {:30} : {}", key, val_str));
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, item) in arr.iter().enumerate() {
                    let val_str = match item {
                        serde_json::Value::String(s) => s.clone(),
                        other => format!("{:?}", other),
                    };
                    rows.push(format!("  [{:>3}] {}", i, val_str));
                }
            }
            _ => {
                return value.to_string();
            }
        }

        if rows.is_empty() {
            "(empty)".to_string()
        } else {
            format!("{}", rows.join("\n"))
        }
    }
}
