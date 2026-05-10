// ════════════════════════════════════════════════════════════════
// Mermaid 图表终端渲染器
//
// 支持:
//   - flowchart (流程图)
//   - sequenceDiagram (时序图)
//   - classDiagram (类图)
//   - stateDiagram (状态机)
//   - erDiagram (ER图)
//   - gantt (甘特图)
//   - pie (饼图)
//
// 输出: ASCII art / Unicode box-drawing 字符
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiagramType {
    Flowchart,
    SequenceDiagram,
    ClassDiagram,
    StateDiagram,
    ErDiagram,
    Gantt,
    Pie,
    MindMap,
    GitGraph,
}

impl AsRef<str> for DiagramType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Flowchart => "flowchart",
            Self::SequenceDiagram => "sequenceDiagram",
            Self::ClassDiagram => "classDiagram",
            Self::StateDiagram => "stateDiagram",
            Self::ErDiagram => "erDiagram",
            Self::Gantt => "gantt",
            Self::Pie => "pie",
            Self::MindMap => "mindmap",
            Self::GitGraph => "gitGraph",
        }
    }
}

/// Mermaid 图表定义
#[derive(Debug, Clone)]
pub struct MermaidDiagram {
    pub diagram_type: DiagramType,
    pub title: Option<String>,
    pub source: String,
}

impl MermaidDiagram {
    /// 从 Mermaid DSL 语法解析
    pub fn from_mermaid(source: &str) -> Result<Self, String> {
        // 检测图表类型
        let dt = if source.contains("sequenceDiagram") || source.contains("participant") {
            DiagramType::SequenceDiagram
        } else if source.contains("classDiagram") {
            DiagramType::ClassDiagram
        } else if source.contains("stateDiagram") {
            DiagramType::StateDiagram
        } else if source.contains("erDiagram") {
            DiagramType::ErDiagram
        } else if source.contains("gantt") {
            DiagramType::Gantt
        } else if source.contains("pie") {
            DiagramType::Pie
        } else if source.contains("flowchart") || source.contains("-->") {
            DiagramType::Flowchart
        } else if source.contains("gitGraph") {
            DiagramType::GitGraph
        } else {
            DiagramType::Flowchart // 默认
        };

        Ok(Self { diagram_type: dt, title: None, source: source.to_string() })
    }

    /// 创建流程图
    pub fn flowchart(title: impl Into<String>, source: &str) -> Self {
        Self { diagram_type: DiagramType::Flowchart, title: Some(title.into()), source: format!("flowchart\n{}", source) }
    }

    /// 创建时序图
    pub fn sequence(source: &str) -> Self {
        Self { diagram_type: DiagramType::SequenceDiagram, title: None, source: source.to_string() }
    }
}

/// 渲染结果
#[derive(Debug, Clone)]
pub struct RenderedDiagram {
    /// ASCII/Unicode 文本表示
    pub ascii_art: String,
    /// 估计的行数
    pub height: usize,
    /// 估计的最大宽度
    pub width: usize,
}

/// Mermaid 渲染器
pub struct MermaidRenderer {
    max_width: usize,
}

impl Default for MermaidRenderer {
    fn default() -> Self { Self::new() }
}

impl MermaidRenderer {
    pub fn new() -> Self {
        Self { max_width: 120 }
    }

    pub fn with_max_width(max_width: usize) -> Self {
        Self { max_width }
    }

    /// 渲染为 ASCII Art (简化版 — 实际生产环境可调用 mermaid-cli 或 WASM 渲染引擎)
    pub fn render(&self, diagram: &MermaidDiagram) -> RenderedDiagram {
        let art = self.render_to_ascii(diagram);
        let height = art.lines().count();
        let width = art.lines().map(|l| l.len()).max().map(|m| m.min(self.max_width)).unwrap_or(self.max_width);

        RenderedDiagram {
            ascii_art: art,
            height,
            width,
        }
    }

    fn render_to_ascii(&self, diag: &MermaidDiagram) -> String {
        // 简化的 ASCII 渲染 (实际实现应使用 mermaid.ink 或 mmdc CLI)
        let mut lines = vec![];

        // 标题
        if let Some(ref title) = diag.title {
            lines.push(format!("╔{}╗", "═".repeat(title.len().max(20))));
            lines.push(format!("║ {:width$} ║", title, width = self.max_width.saturating_sub(4)));
            lines.push(format!("╚{}╝", "═".repeat(title.len().max(20))));
            lines.push(String::new());
        }

        // 类型标识
        lines.push(format!("[ {} Diagram ]", diag.diagram_type.as_ref()));
        lines.push(String::new());

        // 源码预览 (缩进显示)
        for line in diag.source.lines().take(30) {
            if line.len() > self.max_width {
                lines.push(format!("  {}...", &line[..self.max_width - 3]));
            } else {
                lines.push(format!("  {}", line));
            }
        }

        if diag.source.lines().count() > 30 {
            lines.push("  ... (truncated)".to_string());
        }

        lines.push(String::new());
        lines.push(format!("╚{}╝", "═".repeat(self.max_width.min(40))));

        lines.join("\n")
    }
}
