//! # 代码片段管理
//!
//! 提供可复用的代码片段功能：
//! - **片段存储** - 管理常用命令模板
//! - **变量替换** - 支持动态参数
//! - **分类组织** - 按语言/用途分组
//! - **快捷触发** - 缩写展开

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 代码片段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    /// 唯一标识符（缩写）
    trigger: String,
    /// 描述
    description: String,
    /// 片段内容模板
    body: Vec<String>,
    /// 语言/上下文
    language: Option<String>,
    /// 分类标签
    tags: Vec<String>,
}

/// 片段变量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnippetVariable {
    /// 变量名
    name: String,
    /// 默认值
    default_value: Option<String>,
    /// 描述
    description: Option<String>,
    /// 是否必需
    required: bool,
}

/// 片段展开结果
#[derive(Debug, Clone)]
pub struct ExpandedSnippet {
    /// 展开后的文本
    text: String,
    /// 光标位置（可选）
    cursor_position: Option<usize>,
}

/// 片段管理器
pub struct SnippetManager {
    snippets: HashMap<String, Snippet>,
}

impl SnippetManager {
    fn new() -> Self {
        Self {
            snippets: HashMap::new(),
        }
    }

    /// 注册一个片段
    fn register(&mut self, snippet: Snippet) {
        self.snippets.insert(snippet.trigger.clone(), snippet);
    }

    /// 根据触发词查找片段
    fn find_by_trigger(&self, trigger: &str) -> Option<&Snippet> {
        self.snippets.get(trigger)
    }

    /// 展开片段（替换变量）
    fn expand(&self, trigger: &str, variables: &HashMap<String, String>) -> Option<ExpandedSnippet> {
        let snippet = self.snippets.get(trigger)?;

        let mut text = String::new();
        for line in &snippet.body {
            let expanded = Self::replace_variables(line, variables);
            text.push_str(&expanded);
            text.push('\n');
        }

        // 移除末尾多余的换行
        text = text.trim_end().to_string();

        Some(ExpandedSnippet {
            text,
            cursor_position: None,
        })
    }

    fn replace_variables(template: &str, vars: &HashMap<String, String>) -> String {
        let mut result = template.to_string();
        for (key, value) in vars {
            result = result.replace(&format!("${{{}}}", key), value);
        }
        result
    }

    /// 获取所有匹配前缀的片段
    fn search(&self, prefix: &str) -> Vec<&Snippet> {
        self.snippets
            .values()
            .filter(|s| s.trigger.starts_with(prefix) || s.description.contains(prefix))
            .collect()
    }

    /// 获取片段数量
    fn len(&self) -> usize {
        self.snippets.len()
    }

    fn is_empty(&self) -> bool {
        self.snippets.is_empty()
    }
}

impl Default for SnippetManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 预定义的常用片段
pub fn builtin_snippets() -> Vec<Snippet> {
    vec![
        Snippet {
            trigger: "git-commit".into(),
            description: "Standard git commit message".into(),
            body: vec![
                "git add -A".into(),
                "git commit -m \"${{type}}: ${{message}}\"".into(),
            ],
            language: Some("bash".into()),
            tags: vec!["git".into(), "commit".into()],
        },
        Snippet {
            trigger: "docker-run".into(),
            description: "Run docker container with common options".into(),
            body: vec![
                "docker run -d \\".into(),
                "  --name ${{container_name}} \\".into(),
                "  -p ${{host_port}}:${{container_port}} \\".into(),
                "  ${{image}}".into(),
            ],
            language: Some("bash".into()),
            tags: vec!["docker".into(), "container".into()],
        },
        Snippet {
            trigger: "cargo-test".into(),
            description: "Run Rust tests with output".into(),
            body: vec![
                "cargo test -- --nocapture ${{test_name}}".into(),
            ],
            language: Some("bash".into()),
            tags: vec!["rust".into(), "cargo".into(), "test".into()],
        },
        Snippet {
            trigger: "npm-script".into(),
            description: "Run npm script".into(),
            body: vec![
                "npm run ${{script_name}}".into(),
            ],
            language: Some("bash".into()),
            tags: vec!["npm".into(), "node".into()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snippet_registration() {
        let mut manager = SnippetManager::new();
        manager.register(Snippet {
            trigger: "test".into(),
            description: "Test snippet".into(),
            body: vec!["hello ${{{name}}}".into()],
            language: None,
            tags: vec![],
        });

        assert_eq!(manager.len(), 1);
        assert!(manager.find_by_trigger("test").is_some());
    }

    #[test]
    fn test_snippet_expand() {
        let mut manager = SnippetManager::new();
        manager.register(Snippet {
            trigger: "greet".into(),
            description: "Greeting".into(),
            body: vec!["Hello, ${{name}}!".into()],
            language: None,
            tags: vec![],
        });

        let mut vars = HashMap::new();
        vars.insert("name".into(), "World".into());

        let result = manager.expand("greet", &vars).unwrap();
        assert_eq!(result.text, "Hello, World!");
    }

    #[test]
    fn test_builtin_snippets() {
        let snippets = builtin_snippets();
        assert!(!snippets.is_empty());
        assert!(snippets.iter().any(|s| s.trigger == "git-commit"));
    }
}
