use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
pub enum DiagramDisplayMode {
    #[default]
    None,
    Margin,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
pub enum MarkdownSpacingMode {
    #[default]
    Compact,
    Document,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyTargetKind {
    CodeBlock { language: Option<String> },
    Error,
    ToolOutput,
}

impl CopyTargetKind {
    pub fn label(&self) -> String {
        match self {
            Self::CodeBlock { language } => language
                .as_deref()
                .filter(|lang| !lang.is_empty())
                .unwrap_or("code")
                .to_string(),
            Self::Error => "error".to_string(),
            Self::ToolOutput => "output".to_string(),
        }
    }

    pub fn copied_notice(&self) -> String {
        match self {
            Self::CodeBlock { language } => {
                let label = language
                    .as_deref()
                    .filter(|lang| !lang.is_empty())
                    .unwrap_or("code block");
                format!("Copied {}", label)
            }
            Self::Error => "Copied error".to_string(),
            Self::ToolOutput => "Copied output".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RawCopyTarget {
    pub kind: CopyTargetKind,
    pub content: String,
    pub start_raw_line: usize,
    pub end_raw_line: usize,
    pub badge_raw_line: usize,
}
