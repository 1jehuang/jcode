//! W4: Article Analyzer — 分析知识库/文档/wiki 站点
//! 移植自: Understand-Anything agents/article-analyzer
//! 处理 Karpathy 模式的 LLM wiki 或文档站，生成带社区聚类的力导向知识图谱
//!
//! 特点:
//! - 支持 Markdown wikilinks 解析: [[Page Name]] 和 [text](link)
//! - 文档间引用关系提取和聚类
//! - 按主题/标签自动分组 (社区检测)
//! - 知识图谱输出兼容主流水线

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::knowledge_graph::export;
use super::{KGEdge, KGNode, KnowledgeGraph, NodeKind, RelationType, PipelineConfig};
use super::project_scanner::scan_project;

/// 文章分析结果
#[derive(Debug, Clone)]
pub struct ArticleAnalysis {
    pub file_path: String,
    pub title: String,
    pub wikilinks: Vec<String>,       // [[Page Name]] 引用
    pub external_links: Vec<String>,  // [text](url) 外部链接
    pub tags: Vec<String>,            // 标签/分类
    pub summary: String,
    pub word_count: usize,
}

/// Agent 7: 分析知识库/文档站
pub struct ArticleAnalyzer {
    articles: Arc<RwLock<Vec<ArticleAnalysis>>>,
}

impl ArticleAnalyzer {
    pub fn new() -> Self {
        Self { articles: Arc::new(RwLock::new(Vec::new())) }
    }

    /// 运行完整分析流水线
    pub async fn analyze(
        &self,
        root: &Path,
        output_path: &Path,
    ) -> Result<KnowledgeGraph, String> {
        // 1. 扫描 Markdown 文件
        let config = PipelineConfig::default();
        let files = scan_project(root, &config).await?;
        let md_files: Vec<_> = files.iter()
            .filter(|f| f.language == "Markdown" || f.path.ends_with(".md") || f.path.ends_with(".mdx"))
            .collect();

        if md_files.is_empty() {
            return Err("No Markdown files found in the specified path".to_string());
        }

        // 2. 逐个分析文章
        let mut all_articles = Vec::new();
        for file in &md_files {
            let full_path = root.join(&file.path);
            if let Ok(content) = tokio::fs::read_to_string(&full_path).await {
                let analysis = self.analyze_article(&file.path, &content);
                all_articles.push(analysis);
            }
        }
        *self.articles.write().await = all_articles.clone();

        // 3. 构建知识图谱
        let graph = self.build_graph(&all_articles, root);

        // 4. 输出 JSON
        export::to_json(&graph, output_path)?;

        Ok(graph)
    }

    /// 分析单篇文章 (Markdown 文件)
    pub fn analyze_article(&self, file_path: &str, content: &str) -> ArticleAnalysis {
        let title = self.extract_title(content, file_path);
        let wikilinks = self.extract_wikilinks(content);
        let external_links = self.extract_external_links(content);
        let tags = self.extract_tags(content, file_path);
        let summary = self.extract_summary(content);
        let word_count = content.split_whitespace().count();

        ArticleAnalysis {
            file_path: file_path.to_string(),
            title: title.clone(),
            wikilinks,
            external_links,
            tags,
            summary,
            word_count,
        }
    }

    /// 提取标题 (第一个 # 标题 或 文件名)
    fn extract_title(&self, content: &str, file_path: &str) -> String {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") {
                return trimmed.trim_start_matches("# ").trim().to_string();
            }
            if trimmed.starts_with("#") && !trimmed.starts_with("##") && !trimmed.starts_with("#!") {
                return trimmed.trim_start_matches('#').trim().to_string();
            }
        }
        // 回退: 使用文件名 (去掉 .md 后缀)
        Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_path)
            .replace('-', " ")
            .replace('_', " ")
    }

    /// 提取 Wikilinks: [[Page Name]] 或 [[Page Name|display text]]
    fn extract_wikilinks(&self, content: &str) -> Vec<String> {
        let mut links = Vec::new();
        // 匹配 [[...]] 模式
        for line in content.lines() {
            let mut remaining = line;
            while let Some(start) = remaining.find("[[") {
                let after_start = &remaining[start + 2..];
                if let Some(end) = after_start.find("]]") {
                    let link_text = &after_start[..end];
                    // 处理 [[page|display]] 格式
                    let page_name = if let Some(pipe_pos) = link_text.find('|') {
                        &link_text[..pipe_pos]
                    } else {
                        link_text
                    };
                    links.push(page_name.trim().to_string());
                    remaining = &after_start[end + 2..];
                } else {
                    break;
                }
            }
        }
        // 去重保持顺序
        let mut seen = Vec::new();
        for link in links {
            if !seen.contains(&link) {
                seen.push(link);
            }
        }
        seen
    }

    /// 提取外部链接: [text](url)
    fn extract_external_links(&self, content: &str) -> Vec<String> {
        let mut links = Vec::new();
        for line in content.lines() {
            let mut remaining = line;
            while let Some(start) = remaining.find("](") {
                // 往前找 [
                let before = &remaining[..start];
                if let Some(_bracket_start) = before.rfind('[') {
                    let after_start = &remaining[start + 2..];
                    if let Some(end) = after_start.find(')') {
                        let url = &after_start[..end];
                        if url.starts_with("http://") || url.starts_with("https://") {
                            links.push(url.to_string());
                        }
                        remaining = &after_start[end + 1..];
                    } else {
                        break;
                    }
                } else {
                    remaining = &remaining[start + 2..];
                }
            }
        }
        links
    }

    /// 提取标签 (tags: [...] 或 tags: [...] 格式)
    fn extract_tags(&self, content: &str, file_path: &str) -> Vec<String> {
        let mut tags = Vec::new();

        // 从 frontmatter 提取 (--- 之间的 YAML)
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() > 2 && lines[0].trim() == "---" {
            if let Some(end) = lines[1..].iter().position(|l| l.trim() == "---") {
                let frontmatter = &lines[1..=end];
                for line in frontmatter {
                    let trimmed = line.trim();
                    if let Some(tag_val) = trimmed.strip_prefix("tags:") {
                        let tag_val = tag_val.trim();
                        // 支持 [tag1, tag2] 格式
                        if tag_val.starts_with('[') && tag_val.ends_with(']') {
                            let inner = &tag_val[1..tag_val.len() - 1];
                            for t in inner.split(',') {
                                let t = t.trim().trim_matches('"').trim_matches('\'');
                                if !t.is_empty() {
                                    tags.push(t.to_string());
                                }
                            }
                        } else if !tag_val.is_empty() {
                            tags.push(tag_val.to_string());
                        }
                    }
                }
            }
        }

        // 从路径提取目录标签
        if let Some(parent) = Path::new(file_path).parent() {
            for component in parent.components() {
                if let std::path::Component::Normal(name) = component {
                    let name = name.to_string_lossy();
                    if !name.starts_with('.') && name != "docs" && name != "src" {
                        tags.push(name.to_string());
                    }
                }
            }
        }

        tags
    }

    /// 提取摘要 (第一段非空文本)
    fn extract_summary(&self, content: &str) -> String {
        let mut in_frontmatter = content.starts_with("---");
        for line in content.lines().skip(1) {
            let trimmed = line.trim();
            if in_frontmatter {
                if trimmed == "---" {
                    in_frontmatter = false;
                }
                continue;
            }
            // 跳过标题和空行
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            // 跳过代码块
            if trimmed.starts_with("```") {
                continue;
            }
            // 跳过列表
            if trimmed.starts_with('-') || trimmed.starts_with('*') || trimmed.starts_with("1.") {
                continue;
            }
            if trimmed.len() > 20 {
                return trimmed.chars().take(300).collect();
            }
        }
        String::new()
    }

    /// 构建知识图谱
    fn build_graph(&self, articles: &[ArticleAnalysis], root: &Path) -> KnowledgeGraph {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        // 创建文章节点
        let mut article_map: HashMap<String, usize> = HashMap::new();
        for (i, article) in articles.iter().enumerate() {
            let node_id = format!("article::{}", article.file_path.replace('\\', "/"));
            article_map.insert(article.file_path.clone(), i);

            let tags_str = if article.tags.is_empty() {
                "documentation".to_string()
            } else {
                article.tags.join(", ")
            };

            nodes.push(KGNode {
                id: node_id,
                name: article.title.clone(),
                kind: NodeKind::Documentation,
                file_path: article.file_path.clone(),
                line: 0,
                column: 0,
                summary: article.summary.clone(),
                architecture_layer: Some("Knowledge".to_string()),
                domain: Some(tags_str),
                complexity: None,
            });
        }

        // 创建 wikilink 边
        for article in articles {
            let source_id = format!("article::{}", article.file_path.replace('\\', "/"));
            for link in &article.wikilinks {
                // 查找被引用的文章
                let link_path = link.replace(' ', "-").to_lowercase() + ".md";
                let target_id = format!("article::{}", link_path);

                edges.push(KGEdge {
                    source: source_id.clone(),
                    target: target_id,
                    relation: RelationType::References,
                    weight: 1.0,
                });
            }
        }

        // 创建标签聚类边 (相同标签的文章相连)
        let mut tag_groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, article) in articles.iter().enumerate() {
            for tag in &article.tags {
                tag_groups.entry(tag.clone()).or_default().push(i);
            }
        }
        for (_tag, indices) in &tag_groups {
            for i in 1..indices.len() {
                let prev_id = format!("article::{}", articles[indices[i - 1]].file_path.replace('\\', "/"));
                let curr_id = format!("article::{}", articles[indices[i]].file_path.replace('\\', "/"));
                edges.push(KGEdge {
                    source: prev_id,
                    target: curr_id,
                    relation: RelationType::DependsOn,
                    weight: 0.5,
                });
            }
        }

        KnowledgeGraph {
            metadata: super::GraphMetadata {
                project_name: root.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("knowledge-base")
                    .to_string(),
                project_root: root.to_string_lossy().to_string(),
                generated_at: format!("{:?}", std::time::SystemTime::now()),
                total_files: articles.len(),
                total_nodes: nodes.len(),
                total_edges: edges.len(),
                languages: vec!["Markdown".to_string()],
                version: "1.0".to_string(),
            },
            nodes,
            edges,
        }
    }

    /// 获取统计信息
    pub async fn stats(&self) -> String {
        let articles = self.articles.read().await;
        let mut total_wikilinks = 0usize;
        let mut total_ext_links = 0usize;
        let mut all_tags = HashSet::new();

        for a in articles.iter() {
            total_wikilinks += a.wikilinks.len();
            total_ext_links += a.external_links.len();
            for t in &a.tags {
                all_tags.insert(t.clone());
            }
        }

        format!(
            "━━━ Article Analyzer Stats ━━━\n\
             Articles analyzed:   {}\n\
             Total wikilinks:     {}\n\
             External links:      {}\n\
             Unique tags:         {}\n\
             Total words:         {}",
            articles.len(),
            total_wikilinks,
            total_ext_links,
            all_tags.len(),
            articles.iter().map(|a| a.word_count).sum::<usize>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_title() {
        let analyzer = ArticleAnalyzer::new();
        let content = "# My Awesome Wiki\n\nSome content here";
        assert_eq!(analyzer.extract_title(content, "test.md"), "My Awesome Wiki");

        // 回退到文件名
        assert_eq!(analyzer.extract_title("no heading", "my-page.md"), "my page");
    }

    #[test]
    fn test_extract_wikilinks() {
        let analyzer = ArticleAnalyzer::new();
        let content = "See [[Getting Started]] and [[API Reference|api]] for details.";
        let links = analyzer.extract_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0], "Getting Started");
        assert_eq!(links[1], "API Reference"); // | 后的 display 被忽略
    }

    #[test]
    fn test_extract_external_links() {
        let analyzer = ArticleAnalyzer::new();
        let content = "Check [OpenAI](https://openai.com) and [GitHub](https://github.com)";
        let links = analyzer.extract_external_links(content);
        assert_eq!(links.len(), 2);
        assert!(links[0].starts_with("http"));
    }

    #[test]
    fn test_extract_tags_from_frontmatter() {
        let analyzer = ArticleAnalyzer::new();
        let content = "---\ntags: [rust, async, tokio]\n---\n\n# My Article\n\nContent here";
        let tags = analyzer.extract_tags(content, "docs/advanced/async.md");
        assert!(tags.contains(&"rust".to_string()));
        assert!(tags.contains(&"async".to_string()));
        assert!(tags.contains(&"tokio".to_string()));
        assert!(tags.contains(&"advanced".to_string())); // 从路径提取
    }

    #[test]
    fn test_extract_summary() {
        let analyzer = ArticleAnalyzer::new();
        let content = "# Title\n\nThis is the first paragraph of the article. It should be extracted as the summary.\n\nSecond paragraph.";
        let summary = analyzer.extract_summary(content);
        assert!(summary.contains("first paragraph"));
        assert!(summary.len() < 300);
    }

    #[test]
    fn test_analyze_article() {
        let analyzer = ArticleAnalyzer::new();
        let content = "---\ntags: [rust, tutorial]\n---\n\n# Getting Started with Rust\n\nThis is a tutorial about Rust programming.\n\nSee [[Installation Guide]] for setup.";
        let analysis = analyzer.analyze_article("docs/rust/getting-started.md", content);

        assert_eq!(analysis.title, "Getting Started with Rust");
        assert!(analysis.wikilinks.contains(&"Installation Guide".to_string()));
        assert!(analysis.tags.contains(&"rust".to_string()));
        assert!(analysis.summary.contains("Rust programming"));
    }

    #[test]
    fn test_build_graph() {
        let analyzer = ArticleAnalyzer::new();
        let articles = vec![
            ArticleAnalysis {
                file_path: "index.md".to_string(),
                title: "Home".to_string(),
                wikilinks: vec!["Guide".to_string()],
                external_links: vec![],
                tags: vec!["home".to_string()],
                summary: "Welcome".to_string(),
                word_count: 100,
            },
            ArticleAnalysis {
                file_path: "guide.md".to_string(),
                title: "Guide".to_string(),
                wikilinks: vec![],
                external_links: vec![],
                tags: vec!["guide".to_string()],
                summary: "How to".to_string(),
                word_count: 200,
            },
        ];

        let graph = analyzer.build_graph(&articles, Path::new("."));
        assert_eq!(graph.nodes.len(), 2);
        assert!(graph.edges.iter().any(|e| e.relation == RelationType::References));
    }

    #[tokio::test]
    async fn test_full_analysis() {
        // 创建临时测试目录
        let temp = std::env::temp_dir().join("carpai-article-test");
        let _ = std::fs::create_dir_all(&temp);

        // 创建测试文件
        std::fs::write(temp.join("index.md"),
            "# Welcome\n\nTags: [home, intro]\n\nThis is the home page.\n\nSee [[Getting Started]] for more.")
            .ok();
        std::fs::write(temp.join("getting-started.md"),
            "# Getting Started\n\nTags: [tutorial]\n\nA tutorial page.\n\nCheck [External](https://example.com).")
            .ok();

        let analyzer = ArticleAnalyzer::new();
        let output = temp.join("knowledge-graph.json");
        let result = analyzer.analyze(&temp, &output).await;

        assert!(result.is_ok());
        let graph = result.unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert!(output.exists());

        // 清理
        let _ = std::fs::remove_dir_all(&temp);
    }
}
