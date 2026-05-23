use lsp_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use super::context::{CodeContext, SymbolInfo};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CompletionProviderType {
    Lsp,
    Ai,
    Builtin,
    Snippet,
}

#[derive(Debug, Clone)]
pub struct CompletionProviderConfig {
    pub provider_type: CompletionProviderType,
    pub priority: u32,
    pub enabled: bool,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CompletionItemEnhanced {
    pub item: CompletionItem,
    pub provider: CompletionProviderType,
    pub score: f64,
    pub context_score: f64,
    pub popularity_score: f64,
}

#[async_trait]
pub trait CompletionProvider: Send + Sync {
    fn get_type(&self) -> CompletionProviderType;
    fn get_priority(&self) -> u32;
    async fn provide_completions(
        &self,
        context: &CodeContext,
    ) -> Vec<CompletionItemEnhanced>;
}

pub struct LspCompletionProvider {
    config: CompletionProviderConfig,
    lsp_manager: Arc<dyn LspManager + Send + Sync>,
}

#[async_trait]
pub trait LspManager: Send + Sync {
    async fn get_completions(
        &self,
        uri: &Url,
        position: Position,
        context: Option<CompletionContext>,
    ) -> Result<CompletionResponse, String>;
}

impl CompletionProvider for LspCompletionProvider {
    fn get_type(&self) -> CompletionProviderType {
        CompletionProviderType::Lsp
    }

    fn get_priority(&self) -> u32 {
        self.config.priority
    }

    async fn provide_completions(
        &self,
        context: &CodeContext,
    ) -> Vec<CompletionItemEnhanced> {
        let uri = Url::from_file_path(&context.file_path)
            .unwrap_or_else(|_| Url::parse("file:///unknown").unwrap());
        let result = self.lsp_manager.get_completions(
            &uri,
            context.position,
            None,
        ).await;

        match result {
            Ok(response) => match response {
                CompletionResponse::Array(items) => items
                    .into_iter()
                    .map(|item| CompletionItemEnhanced {
                        item,
                        provider: CompletionProviderType::Lsp,
                        score: 0.7,
                        context_score: 0.8,
                        popularity_score: 0.5,
                    })
                    .collect(),
                CompletionResponse::List(list) => list
                    .items
                    .into_iter()
                    .map(|item| CompletionItemEnhanced {
                        item,
                        provider: CompletionProviderType::Lsp,
                        score: 0.7,
                        context_score: 0.8,
                        popularity_score: 0.5,
                    })
                    .collect(),
            },
            Err(_) => Vec::new(),
        }
    }
}

pub struct AiCompletionProvider {
    config: CompletionProviderConfig,
    api_client: Arc<dyn AiApiClient + Send + Sync>,
}

#[async_trait]
pub trait AiApiClient: Send + Sync {
    async fn generate_completions(
        &self,
        context: &CodeContext,
        max_tokens: usize,
    ) -> Result<Vec<String>, String>;
}

impl CompletionProvider for AiCompletionProvider {
    fn get_type(&self) -> CompletionProviderType {
        CompletionProviderType::Ai
    }

    fn get_priority(&self) -> u32 {
        self.config.priority
    }

    async fn provide_completions(
        &self,
        context: &CodeContext,
    ) -> Vec<CompletionItemEnhanced> {
        let result = self.api_client.generate_completions(context, 512).await;
        
        match result {
            Ok(completions) => completions
                .into_iter()
                .enumerate()
                .map(|(i, text)| CompletionItemEnhanced {
                    item: CompletionItem {
                        label: text.lines().next().unwrap_or(&text).to_string(),
                        kind: Some(CompletionItemKind::TEXT),
                        documentation: Some(Documentation::String(format!(
                            "AI generated completion #{}",
                            i + 1
                        ))),
                        text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                            range: Range {
                                start: context.position,
                                end: context.position,
                            },
                            new_text: text,
                        })),
                        ..Default::default()
                    },
                    provider: CompletionProviderType::Ai,
                    score: 0.9,
                    context_score: 0.95,
                    popularity_score: 0.3,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

pub struct BuiltinCompletionProvider {
    config: CompletionProviderConfig,
    builtin_symbols: Arc<RwLock<HashMap<String, Vec<SymbolInfo>>>>,
}

impl CompletionProvider for BuiltinCompletionProvider {
    fn get_type(&self) -> CompletionProviderType {
        CompletionProviderType::Builtin
    }

    fn get_priority(&self) -> u32 {
        self.config.priority
    }

    async fn provide_completions(
        &self,
        context: &CodeContext,
    ) -> Vec<CompletionItemEnhanced> {
        let symbols = self.builtin_symbols.read().await;
        let empty_vec = Vec::new();
        let lang_symbols = symbols.get(&context.language).unwrap_or(&empty_vec);
        
        lang_symbols
            .iter()
            .map(|symbol| CompletionItemEnhanced {
                item: CompletionItem {
                    label: symbol.name.clone(),
                    kind: Some(match symbol.kind {
                        SymbolKind::FILE => CompletionItemKind::FILE,
                        SymbolKind::MODULE => CompletionItemKind::MODULE,
                        SymbolKind::NAMESPACE => CompletionItemKind::MODULE,
                        SymbolKind::PACKAGE => CompletionItemKind::MODULE,
                        SymbolKind::CLASS => CompletionItemKind::CLASS,
                        SymbolKind::METHOD => CompletionItemKind::METHOD,
                        SymbolKind::PROPERTY => CompletionItemKind::PROPERTY,
                        SymbolKind::FIELD => CompletionItemKind::FIELD,
                        SymbolKind::CONSTRUCTOR => CompletionItemKind::CONSTRUCTOR,
                        SymbolKind::ENUM => CompletionItemKind::ENUM,
                        SymbolKind::INTERFACE => CompletionItemKind::INTERFACE,
                        SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
                        SymbolKind::VARIABLE => CompletionItemKind::VARIABLE,
                        SymbolKind::CONSTANT => CompletionItemKind::CONSTANT,
                        SymbolKind::STRING => CompletionItemKind::TEXT,
                        SymbolKind::NUMBER => CompletionItemKind::VALUE,
                        SymbolKind::BOOLEAN => CompletionItemKind::VALUE,
                        SymbolKind::ARRAY => CompletionItemKind::VALUE,
                        SymbolKind::OBJECT => CompletionItemKind::VALUE,
                        SymbolKind::KEY => CompletionItemKind::KEYWORD,
                        SymbolKind::NULL => CompletionItemKind::VALUE,
                        SymbolKind::ENUM_MEMBER => CompletionItemKind::ENUM_MEMBER,
                        SymbolKind::STRUCT => CompletionItemKind::STRUCT,
                        SymbolKind::EVENT => CompletionItemKind::EVENT,
                        SymbolKind::OPERATOR => CompletionItemKind::OPERATOR,
                        SymbolKind::TYPE_PARAMETER => CompletionItemKind::TYPE_PARAMETER,
                        _ => CompletionItemKind::TEXT,
                    }),
                    documentation: symbol.documentation.clone().map(Documentation::String),
                    ..Default::default()
                },
                provider: CompletionProviderType::Builtin,
                score: 0.6,
                context_score: 0.7,
                popularity_score: 0.8,
            })
            .collect()
    }
}

pub struct SnippetCompletionProvider {
    config: CompletionProviderConfig,
    snippet_store: Arc<RwLock<HashMap<String, Vec<Snippet>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub name: String,
    pub prefix: String,
    pub body: String,
    pub description: String,
    pub language: String,
}

impl CompletionProvider for SnippetCompletionProvider {
    fn get_type(&self) -> CompletionProviderType {
        CompletionProviderType::Snippet
    }

    fn get_priority(&self) -> u32 {
        self.config.priority
    }

    async fn provide_completions(
        &self,
        context: &CodeContext,
    ) -> Vec<CompletionItemEnhanced> {
        let snippets = self.snippet_store.read().await;
        let default_snippets = Vec::new();
        let lang_snippets = snippets.get(&context.language).unwrap_or(&default_snippets);
        
        lang_snippets
            .iter()
            .map(|snippet| CompletionItemEnhanced {
                item: CompletionItem {
                    label: snippet.name.clone(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    documentation: Some(Documentation::String(snippet.description.clone())),
                    insert_text: Some(snippet.body.clone()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                },
                provider: CompletionProviderType::Snippet,
                score: 0.5,
                context_score: 0.6,
                popularity_score: 0.7,
            })
            .collect()
    }
}
