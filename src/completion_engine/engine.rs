use lsp_types::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use tracing::info;
use super::context::{CodeContext, ContextAnalyzer, SymbolInfo};
use super::providers::{
    CompletionProvider, CompletionProviderType, CompletionItemEnhanced,
    LspCompletionProvider, AiCompletionProvider, BuiltinCompletionProvider,
    SnippetCompletionProvider, LspManager, AiApiClient, Snippet,
};
use super::ranking::CompletionRanker;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionConfig {
    pub max_completions: usize,
    pub min_score: f64,
    pub enable_ai: bool,
    pub enable_lsp: bool,
    pub enable_builtin: bool,
    pub enable_snippets: bool,
    pub enable_fuzzy_matching: bool,
    pub cache_ttl_seconds: u64,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            max_completions: 50,
            min_score: 0.3,
            enable_ai: true,
            enable_lsp: true,
            enable_builtin: true,
            enable_snippets: true,
            enable_fuzzy_matching: true,
            cache_ttl_seconds: 300,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub items: Vec<CompletionItem>,
    pub context: CodeContext,
    pub provider_stats: HashMap<CompletionProviderType, usize>,
}

pub struct CompletionEngine {
    config: CompletionConfig,
    context_analyzer: Arc<ContextAnalyzer>,
    providers: Arc<RwLock<Vec<Arc<dyn CompletionProvider + Send + Sync>>>>,
    ranker: Arc<CompletionRanker>,
    cache: Arc<RwLock<HashMap<String, CachedCompletion>>>,
    project_symbols: Arc<RwLock<Vec<SymbolInfo>>>,
}

#[derive(Debug, Clone)]
struct CachedCompletion {
    result: CompletionResult,
    timestamp: std::time::Instant,
}

impl CompletionEngine {
    pub fn new(config: CompletionConfig) -> Self {
        Self {
            config,
            context_analyzer: Arc::new(ContextAnalyzer::new()),
            providers: Arc::new(RwLock::new(Vec::new())),
            ranker: Arc::new(CompletionRanker::new()),
            cache: Arc::new(RwLock::new(HashMap::new())),
            project_symbols: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn add_provider(&self, provider: Arc<dyn CompletionProvider + Send + Sync>) {
        let mut providers = self.providers.write().await;
        providers.push(provider);
    }

    pub async fn set_project_symbols(&self, symbols: Vec<SymbolInfo>) {
        let mut project_symbols = self.project_symbols.write().await;
        *project_symbols = symbols;
    }

    pub async fn complete(
        &self,
        file_path: &PathBuf,
        content: &str,
        position: Position,
    ) -> CompletionResult {
        let cache_key = self.generate_cache_key(file_path, content, position);
        
        if let Some(cached) = self.get_cached(&cache_key).await {
            return cached;
        }

        let project_symbols = self.project_symbols.read().await.clone();
        let context = self.context_analyzer.analyze_context(
            file_path,
            content,
            position,
            &project_symbols,
        ).await;

        let mut all_items: Vec<CompletionItemEnhanced> = Vec::new();
        let mut provider_stats: HashMap<CompletionProviderType, usize> = HashMap::new();

        let providers = self.providers.read().await;
        for provider in providers.iter() {
            let items = provider.provide_completions(&context).await;
            
            *provider_stats.entry(provider.get_type()).or_insert(0) += items.len();
            all_items.extend(items);
        }

        let ranked = self.ranker.rank(all_items).await;
        
        let filtered: Vec<CompletionItemEnhanced> = ranked
            .into_iter()
            .filter(|item| item.score >= self.config.min_score)
            .take(self.config.max_completions)
            .collect();

        let items: Vec<CompletionItem> = filtered
            .into_iter()
            .map(|e| e.item)
            .collect();

        let result = CompletionResult {
            items,
            context,
            provider_stats,
        };

        self.cache_result(&cache_key, &result).await;
        result
    }

    fn generate_cache_key(&self, file_path: &PathBuf, content: &str, position: Position) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let content_hash = hasher.finalize();
        format!(
            "{}_{}_{}_{:x}",
            file_path.display(),
            position.line,
            position.character,
            content_hash
        )
    }

    async fn get_cached(&self, key: &str) -> Option<CompletionResult> {
        let cache = self.cache.read().await;
        if let Some(cached) = cache.get(key) {
            if cached.timestamp.elapsed().as_secs() < self.config.cache_ttl_seconds {
                return Some(cached.result.clone());
            }
        }
        None
    }

    async fn cache_result(&self, key: &str, result: &CompletionResult) {
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), CachedCompletion {
            result: result.clone(),
            timestamp: std::time::Instant::now(),
        });
    }

    pub async fn record_selection(&self, label: &str) {
        self.ranker.record_usage(label).await;
    }
}

pub struct DefaultCompletionEngineBuilder {
    config: CompletionConfig,
    lsp_manager: Option<Arc<dyn LspManager + Send + Sync>>,
    ai_api_client: Option<Arc<dyn AiApiClient + Send + Sync>>,
    builtin_symbols: HashMap<String, Vec<SymbolInfo>>,
    snippets: HashMap<String, Vec<Snippet>>,
}

impl DefaultCompletionEngineBuilder {
    pub fn new() -> Self {
        Self {
            config: CompletionConfig::default(),
            lsp_manager: None,
            ai_api_client: None,
            builtin_symbols: Self::load_default_builtin_symbols(),
            snippets: Self::load_default_snippets(),
        }
    }

    pub fn with_config(mut self, config: CompletionConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_lsp_manager(mut self, manager: Arc<dyn LspManager + Send + Sync>) -> Self {
        self.lsp_manager = Some(manager);
        self
    }

    pub fn with_ai_api_client(mut self, client: Arc<dyn AiApiClient + Send + Sync>) -> Self {
        self.ai_api_client = Some(client);
        self
    }

    pub async fn build(self) -> CompletionEngine {
        let engine = CompletionEngine::new(self.config);

        if self.config.enable_lsp && self.lsp_manager.is_some() {
            let provider = Arc::new(LspCompletionProvider {
                config: providers::CompletionProviderConfig {
                    provider_type: CompletionProviderType::Lsp,
                    priority: 2,
                    enabled: true,
                    config: serde_json::Value::Null,
                },
                lsp_manager: self.lsp_manager.unwrap(),
            });
            engine.add_provider(provider).await;
        }

        if self.config.enable_ai && self.ai_api_client.is_some() {
            let provider = Arc::new(AiCompletionProvider {
                config: providers::CompletionProviderConfig {
                    provider_type: CompletionProviderType::Ai,
                    priority: 1,
                    enabled: true,
                    config: serde_json::Value::Null,
                },
                api_client: self.ai_api_client.unwrap(),
            });
            engine.add_provider(provider).await;
        }

        if self.config.enable_builtin {
            let provider = Arc::new(BuiltinCompletionProvider {
                config: providers::CompletionProviderConfig {
                    provider_type: CompletionProviderType::Builtin,
                    priority: 3,
                    enabled: true,
                    config: serde_json::Value::Null,
                },
                builtin_symbols: Arc::new(RwLock::new(self.builtin_symbols)),
            });
            engine.add_provider(provider).await;
        }

        if self.config.enable_snippets {
            let provider = Arc::new(SnippetCompletionProvider {
                config: providers::CompletionProviderConfig {
                    provider_type: CompletionProviderType::Snippet,
                    priority: 4,
                    enabled: true,
                    config: serde_json::Value::Null,
                },
                snippet_store: Arc::new(RwLock::new(self.snippets)),
            });
            engine.add_provider(provider).await;
        }

        engine
    }

    fn load_default_builtin_symbols() -> HashMap<String, Vec<SymbolInfo>> {
        let mut symbols = HashMap::new();
        
        symbols.insert("rust".to_string(), vec![
            SymbolInfo { name: "String".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "Vec".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "Result".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "Option".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "HashMap".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "VecDeque".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
        ]);
        
        symbols.insert("python".to_string(), vec![
            SymbolInfo { name: "str".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "list".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "dict".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
            SymbolInfo { name: "int".to_string(), kind: SymbolKind::CLASS, location: Location { uri: Url::parse("file:///").unwrap(), range: Range::default() }, documentation: None, signature: None },
        ]);

        symbols
    }

    fn load_default_snippets() -> HashMap<String, Vec<Snippet>> {
        let mut snippets = HashMap::new();
        
        snippets.insert("rust".to_string(), vec![
            Snippet {
                name: "fn main".to_string(),
                prefix: "fn main".to_string(),
                body: "fn main() {\n    $0\n}".to_string(),
                description: "Main function".to_string(),
                language: "rust".to_string(),
            },
            Snippet {
                name: "async fn".to_string(),
                prefix: "async fn".to_string(),
                body: "async fn ${1:name}() -> ${2:Result} {\n    $0\n}".to_string(),
                description: "Async function".to_string(),
                language: "rust".to_string(),
            },
        ]);
        
        snippets.insert("python".to_string(), vec![
            Snippet {
                name: "def".to_string(),
                prefix: "def".to_string(),
                body: "def ${1:name}(${2:args}):\n    \"\"\"${3:docstring}\"\"\"\n    $0".to_string(),
                description: "Function definition".to_string(),
                language: "python".to_string(),
            },
            Snippet {
                name: "class".to_string(),
                prefix: "class".to_string(),
                body: "class ${1:Name}:\n    \"\"\"${2:docstring}\"\"\"\n    def __init__(self${3:, args}):\n        $0".to_string(),
                description: "Class definition".to_string(),
                language: "python".to_string(),
            },
        ]);

        snippets
    }
}