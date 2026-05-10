use crate::ast_context::CompletionContext;
use async_trait::async_trait;

/// LLM 生成的完成候选
#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub text: String,
    pub label: String,
    pub detail: Option<String>,
    pub kind: CandidateKind,
    pub score: f64,
}

/// 候选类型 (供编辑器显示图标)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Function, Method, Variable, Field, Keyword, Snippet, Module, Type,
}

/// LLM 候选生成器 trait — 供 jcode-completion 内部使用
#[async_trait]
pub trait CandidateGenerator: Send + Sync {
    async fn generate(&self, context: &CompletionContext) -> Vec<CompletionCandidate>;
}

/// Provider 补全接口 — 宿主 jcode 应用实现此 trait 来接入 Qwen 3.6
#[async_trait]
pub trait CompletionProvider: Send + Sync {
    /// 简单文本补全，无工具调用、无流式
    async fn complete_simple(&self, prompt: &str, system: &str) -> anyhow::Result<String>;
}

/// 基于真实 LLM Provider 的候选生成器
pub struct ProviderCandidateGenerator {
    provider: Box<dyn CompletionProvider>,
}

impl ProviderCandidateGenerator {
    pub fn new(provider: Box<dyn CompletionProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl CandidateGenerator for ProviderCandidateGenerator {
    async fn generate(&self, context: &CompletionContext) -> Vec<CompletionCandidate> {
        let prompt = format!(
            "Complete the code at cursor position.\n\
             File: {file}\n\
             Expected type: {type_:?}\n\
             Scope: {scope:?}\n\
             Parent symbol: {parent:?}\n\
             Current line: {line}\n\
             Cursor prefix: '{prefix}'\n\
             \n\
             Provide the single most likely completion text (no explanation):",
            file = context.file_path,
            type_ = context.expected_type,
            scope = context.scope,
            parent = context.parent_symbol,
            line = context.file_path.lines().nth(context.line).unwrap_or(""),
            prefix = context.prefix,
        );

        match self.provider.complete_simple(&prompt, "You are a code completion engine. Output ONLY the completion text.").await {
            Ok(text) => {
                let cleaned = text.trim().to_string();
                vec![CompletionCandidate {
                    label: cleaned.clone(),
                    text: cleaned,
                    detail: context.expected_type.clone(),
                    kind: CandidateKind::Snippet,
                    score: 0.95,
                }]
            }
            Err(_) => vec![],
        }
    }
}

/// 默认实现: 基于上下文的模板生成 + 规则匹配 (保底)
pub struct DefaultCandidateGenerator;

impl DefaultCandidateGenerator {
    pub fn new() -> Self { Self }

    fn candidates_for_context(&self, ctx: &CompletionContext) -> Vec<CompletionCandidate> {
        let mut candidates = Vec::new();
        match ctx.scope {
            crate::ast_context::ScopeKind::MethodChain => {
                for method in &["map()", "filter()", "collect()", "for_each()", "fold()"] {
                    candidates.push(CompletionCandidate {
                        text: method.to_string(),
                        label: method.trim_end_matches("()").to_string(),
                        detail: Some(method.to_string()),
                        kind: CandidateKind::Method, score: 0.8,
                    });
                }
            }
            crate::ast_context::ScopeKind::Assignment => {
                candidates.push(CompletionCandidate {
                    text: format!("{}::new()", ctx.prefix),
                    label: format!("{}::new()", ctx.prefix),
                    detail: Some("Construct a new instance".into()),
                    kind: CandidateKind::Function, score: 0.9,
                });
            }
            crate::ast_context::ScopeKind::Import => {
                candidates.push(CompletionCandidate {
                    text: "crate::".into(), label: "crate::".into(),
                    detail: Some("current crate".into()), kind: CandidateKind::Module, score: 0.9,
                });
                candidates.push(CompletionCandidate {
                    text: "std::".into(), label: "std::".into(),
                    detail: Some("standard library".into()), kind: CandidateKind::Module, score: 0.8,
                });
            }
            _ => {
                for kw in &["let ", "fn ", "pub ", "struct ", "enum ", "impl ", "match ", "if ", "for ", "while "] {
                    if kw.starts_with(&ctx.prefix) || ctx.prefix.is_empty() {
                        candidates.push(CompletionCandidate {
                            text: kw.to_string(), label: kw.trim().to_string(),
                            detail: None, kind: CandidateKind::Keyword, score: 0.6,
                        });
                    }
                }
            }
        }
        candidates
    }
}

#[async_trait]
impl CandidateGenerator for DefaultCandidateGenerator {
    async fn generate(&self, ctx: &CompletionContext) -> Vec<CompletionCandidate> {
        self.candidates_for_context(ctx)
    }
}
