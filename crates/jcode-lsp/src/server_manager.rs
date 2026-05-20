//! LSP Server Manager — 多语言 Server 生命周期管理
//!
//! ## 整合来源
//! - **jcode-lsp/server_manager.rs**: LanguageId 枚举、ServerConfig、LspOperations trait
//! - **ide-integration/lsp_client.rs**: 12 种语言内置配置、扩展名路由
//! - **src/lsp_client.rs**: server_for_file 路由逻辑
//!
//! ## 支持的语言服务器 (对标 Claude Code)
//! ✅ Rust: rust-analyzer
//! ✅ TypeScript/JavaScript: typescript-language-server
//! ✅ Python: pylsp / pyright-langserver
//! ✅ Go: gopls
//! ✅ C/C++: clangd
//! ✅ Java: jdtls
//! ✅ JSON/HTML/CSS/YAML/Markdown/TOML: vscode 语言服务器

use crate::client::{LspClient, LspError, LspResult};
use lsp_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// 语言 ID (用于文档关联和服务器路由)
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LanguageId {
    TypeScript,
    JavaScript,
    Rust,
    Python,
    Go,
    Cpp,
    C,
    Java,
    Json,
    Markdown,
    Html,
    Css,
    Shell,
    Yaml,
    Toml,
    Unknown(String),
}

impl AsRef<str> for LanguageId {
    fn as_ref(&self) -> &str {
        match self {
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
            Self::Cpp => "cpp",
            Self::C => "c",
            Self::Java => "java",
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Css => "css",
            Self::Shell => "shellscript",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Unknown(s) => s.as_str(),
        }
    }
}

impl From<&str> for LanguageId {
    fn from(s: &str) -> Self {
        match s {
            "typescript" | "ts" | "tsx" => Self::TypeScript,
            "javascript" | "js" | "jsx" => Self::JavaScript,
            "rust" | "rs" => Self::Rust,
            "python" | "py" => Self::Python,
            "go" => Self::Go,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hxx" => Self::Cpp,
            "c" | "h" => Self::C,
            "java" => Self::Java,
            "json" => Self::Json,
            "markdown" | "md" => Self::Markdown,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "less" => Self::Css,
            "shell" | "sh" | "bash" | "zsh" => Self::Shell,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// Server 启动配置
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 命令名
    pub command: String,
    
    /// 命令参数
    pub args: Vec<String>,
    
    /// 环境变量
    pub env_vars: Vec<(String, String)>,
    
    /// 关联的语言
    pub languages: Vec<LanguageId>,
    
    /// 文件扩展名模式 (用于自动选择 server)
    pub file_patterns: Vec<String>,
    
    /// 是否按需启动 (懒加载)
    pub lazy_start: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: vec![],
            env_vars: vec![],
            languages: vec![],
            file_patterns: vec![],
            lazy_start: true,
        }
    }
}

/// 内置 Server 配置表 — 整合自 ide-integration 和 jcode-lsp
///
/// 对标 Claude Code 的自动发现机制，但使用静态配置更可靠
fn builtin_server_configs() -> Vec<ServerConfig> {
    vec![
        // === Rust: rust-analyzer ===
        ServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            env_vars: vec![],
            languages: vec![LanguageId::Rust],
            file_patterns: vec!["*.rs".into()],
            lazy_start: true,
        },
        
        // === TypeScript/JavaScript: typescript-language-server ===
        ServerConfig {
            command: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::TypeScript, LanguageId::JavaScript],
            file_patterns: vec![
                "*.ts".into(), "*.tsx".into(), "*.js".into(), "*.jsx".into(),
                "*.mjs".into(), "*.cjs".into(),
            ],
            lazy_start: true,
        },
        
        // === Python: pylsp (python-lsp-server) ===
        ServerConfig {
            command: "pylsp".into(),
            args: vec![],
            env_vars: vec![],
            languages: vec![LanguageId::Python],
            file_patterns: vec!["*.py".into(), "*.pyi".into()],
            lazy_start: true,
        },
        
        // === Go: gopls ===
        ServerConfig {
            command: "gopls".into(),
            args: vec!["serve".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Go],
            file_patterns: vec!["*.go".into()],
            lazy_start: true,
        },
        
        // === C/C++: clangd ===
        ServerConfig {
            command: "clangd".into(),
            args: vec![
                "--background-index".into(),
                "--clang-tidy".into(),
                "--completion-style=detailed".into(),
            ],
            env_vars: vec![],
            languages: vec![LanguageId::Cpp, LanguageId::C],
            file_patterns: vec![
                "*.cpp".into(), "*.c".into(), "*.cc".into(),
                "*.h".into(), "*.hpp".into(), "*.hxx".into(),
            ],
            lazy_start: true,
        },
        
        // === Java: jdtls ===
        ServerConfig {
            command: "jdtls".into(),
            args: vec![],
            env_vars: vec![],
            languages: vec![LanguageId::Java],
            file_patterns: vec!["*.java".into()],
            lazy_start: true,
        },
        
        // === HTML: html-languageserver ===
        ServerConfig {
            command: "html-languageserver".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Html],
            file_patterns: vec!["*.html".into(), "*.htm".into(), "*.vue".into(), "*.svelte".into()],
            lazy_start: false,
        },
        
        // === CSS: css-languageserver ===
        ServerConfig {
            command: "css-languageserver".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Css],
            file_patterns: vec!["*.css".into(), "*.scss".into(), "*.less".into()],
            lazy_start: false,
        },
        
        // === JSON: json-languageserver ===
        ServerConfig {
            command: "json-languageserver".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Json],
            file_patterns: vec!["*.json".into()],
            lazy_start: false,
        },
        
        // === YAML: yaml-language-server ===
        ServerConfig {
            command: "yaml-language-server".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Yaml],
            file_patterns: vec!["*.yml".into(), "*.yaml".into()],
            lazy_start: false,
        },
        
        // === Markdown: marksman ===
        ServerConfig {
            command: "marksman".into(),
            args: vec!["server".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Markdown],
            file_patterns: vec!["*.md".into(), "*.markdown".into()],
            lazy_start: false,
        },
        
        // === TOML: taplo ===
        ServerConfig {
            command: "taplo".into(),
            args: vec!["lsp".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Toml],
            file_patterns: vec!["*.toml".into()],
            lazy_start: false,
        },
    ]
}

/// LSP Server 管理器
///
/// 核心职责:
/// 1. 根据文件路径路由到正确的 LSP Server
/// 2. 懒加载启动（按需初始化）
/// 3. 多实例生命周期管理
/// 4. 扩展名 -> 语言 -> Server 映射
#[allow(dead_code)]
pub struct LspServerManager {
    /// language_id -> LspClient (Arc 共享)
    servers: RwLock<HashMap<String, Arc<RwLock<LspClient>>>>,
    
    /// 已知的 Server 配置
    configs: Vec<ServerConfig>,
    
    /// workspace root URI
    workspace_root: String,
    
    /// 扩展名 -> language_id 映射 (快速查找)
    ext_to_lang: HashMap<String, String>,
    
    /// language_id -> config index 映射
    lang_to_config: HashMap<String, usize>,
}

impl Default for LspServerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LspServerManager {
    pub fn new() -> Self {
        let configs = builtin_server_configs();
        
        let mut ext_to_lang = HashMap::new();
        let mut lang_to_config = HashMap::new();
        
        for (idx, cfg) in configs.iter().enumerate() {
            for lang in &cfg.languages {
                lang_to_config.insert(lang.as_ref().to_string(), idx);
            }
            
            for pattern in &cfg.file_patterns {
                if let Some(ext) = pattern.strip_prefix('*') {
                    ext_to_lang.insert(ext.to_lowercase(), 
                        cfg.languages.first()
                            .map(|l| l.as_ref().to_string())
                            .unwrap_or_default());
                }
            }
        }
        
        Self {
            servers: RwLock::new(HashMap::new()),
            configs,
            workspace_root: ".".into(),
            ext_to_lang,
            lang_to_config,
        }
    }

    pub fn with_workspace(mut self, root: &str) -> Self {
        self.workspace_root = root.to_string();
        self
    }

    /// 根据文件路径获取或启动对应的 LSP Server
    ///
    /// 这是核心路由方法！整合了三套实现的逻辑：
    /// - src/lsp_client.rs: server_for_file() 扩展名匹配
    /// - ide-integration: get_client_for_file() 路由
    /// - jcode-lsp: get_or_start_server_for_file() 懒加载
    pub async fn get_or_start_server_for_file(&self, file_path: &str) -> Option<Arc<RwLock<LspClient>>> {
        let lang = Self::detect_language_from_path(file_path);
        
        let client = {
            let servers = self.servers.read().await;
            servers.get(lang.as_ref()).cloned()
        };

        if let Some(client) = client {
            return Some(client);
        }

        // 尝试启动新的 Server
        if let Some(config) = self.find_config_for_language(&lang).await {
            match self.start_server(config.clone()).await {
                Ok(client) => Some(client),
                Err(e) => {
                    warn!(error = %e, lang = %lang.as_ref(), "Failed to start LSP server");
                    None
                }
            }
        } else {
            warn!(lang = %lang.as_ref(), "No LSP server configured");
            None
        }
    }

    /// 手动启动一个 LSP Server
    ///
    /// 真正的实现！（不再是 TODO 或被注释）
    pub async fn start_server(&self, config: ServerConfig) -> Result<Arc<RwLock<LspClient>>, String> {
        let server_name = config.command.clone();
        let lang_id = config.languages.first()
            .map(|l| l.as_ref().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        
        info!("Starting LSP server: {} ({}) for language {}", server_name, lang_id, 
               config.languages.iter().map(|l| l.as_ref()).collect::<Vec<_>>().join(","));
        
        // 创建 Client
        let client = LspClient::new(server_name.clone());
        let client_arc = Arc::new(RwLock::new(client));
        
        // 启动进程并建立连接（需要 write lock）
        {
            let c = client_arc.write().await;
            c.start(
                &config.command,
                &config.args,
                Some(&self.workspace_root),
            ).await.map_err(|e| e.to_string())?;
            
            // 初始化 LSP 协议
            let _root_uri = Url::from_file_path(&self.workspace_root).ok();
            c.initialize().await.map_err(|e| e.to_string())?;
        }
        
        // 注册到所有关联语言
        {
            let mut servers = self.servers.write().await;
            for lang in &config.languages {
                servers.insert(lang.as_ref().to_string(), client_arc.clone());
            }
        }

        info!("LSP server {} started and initialized successfully", server_name);
        Ok(client_arc)
    }

    /// 停止所有 Servers
    pub async fn shutdown_all(&self) {
        let servers = self.servers.read().await;
        for (_lang, client) in servers.iter() {
            let c = client.write().await;
            if let Err(e) = c.shutdown().await {
                warn!("Error shutting down LSP server: {}", e);
            }
        }

        drop(servers); // 释放读锁
        self.servers.write().await.clear();
        info!("All LSP servers shut down");
    }

    /// 列出所有已启动的 Servers
    pub async fn list_running_servers(&self) -> Vec<(String, bool)> {
        let servers = self.servers.read().await;
        servers.keys().map(|name| (name.clone(), true))
            .collect()
    }

    /// 检查是否有指定语言的 Server 在运行
    pub async fn is_server_running(&self, language: &LanguageId) -> bool {
        let servers = self.servers.read().await;
        servers.contains_key(language.as_ref())
    }

    /// 获取已注册的服务器数量
    pub fn registered_count(&self) -> usize {
        self.configs.len()
    }

    /// 获取活跃的客户端数量
    pub async fn active_count(&self) -> usize {
        self.servers.read().await.len()
    }

    // --- 内部方法 -------------------------

    async fn find_config_for_language(&self, lang: &LanguageId) -> Option<&ServerConfig> {
        self.lang_to_config.get(lang.as_ref())
            .and_then(|&idx| self.configs.get(idx))
    }

    /// 从文件路径推断语言
    ///
    /// 整合了三套实现的语言检测逻辑：
    /// - jcode-lsp: detect_language_from_path()
    /// - ide-integration: get_builtin_lsp_servers() 扩展名映射
    /// - src/lsp_client.rs: server_for_file() 扩展名查找
    fn detect_language_from_path(path: &str) -> LanguageId {
        // 从文件扩展名推断
        if let Some(ext) = path.rsplit('.').next() {
            let ext_lower = ext.to_lowercase();
            
            // 快速路径：直接查扩展名映射
            if let Some(lang) = Self::ext_to_lang_static(&ext_lower) {
                return lang;
            }
        }

        // 特殊文件名检测
        let filename = path.rsplit('/').next()
            .or_else(|| path.rsplit('\\').next())
            .unwrap_or(path);
        
        match filename {
            "Dockerfile" | "Containerfile" => LanguageId::Shell,
            "Makefile" | "makefile" => LanguageId::Shell,
            "Cargo.toml" | "Cargo.lock" => LanguageId::Toml,
            _ => LanguageId::Unknown("plaintext".into()),
        }
    }

    /// 静态扩展名 -> 语言映射（避免每次创建实例）
    fn ext_to_lang_static(ext: &str) -> Option<LanguageId> {
        match ext {
            "rs" => Some(LanguageId::Rust),
            "ts" | "tsx" => Some(LanguageId::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(LanguageId::JavaScript),
            "py" | "pyi" => Some(LanguageId::Python),
            "go" => Some(LanguageId::Go),
            "cpp" | "cxx" | "cc" | "hpp" | "hxx" => Some(LanguageId::Cpp),
            "c" | "h" => Some(LanguageId::C),
            "java" => Some(LanguageId::Java),
            "json" => Some(LanguageId::Json),
            "md" | "markdown" => Some(LanguageId::Markdown),
            "html" | "htm" | "vue" | "svelte" => Some(LanguageId::Html),
            "css" | "scss" | "less" => Some(LanguageId::Css),
            "sh" | "bash" | "zsh" => Some(LanguageId::Shell),
            "yml" | "yaml" => Some(LanguageId::Yaml),
            "toml" => Some(LanguageId::Toml),
            _ => None,
        }
    }
}

// ============================================================================
// LspOperations trait 实现 — 统一的高层 API
// ============================================================================

#[async_trait::async_trait]
impl super::LspOperations for LspServerManager {
    async fn goto_definition(
        &self, 
        file: &str, 
        line: u32, 
        character: u32
    ) -> LspResult<Vec<Location>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.goto_definition(file, line, character).await
    }

    async fn find_references(
        &self, 
        file: &str, 
        line: u32, 
        character: u32
    ) -> LspResult<Vec<Location>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.find_references(file, line, character).await
    }

    async fn get_diagnostics(
        &self, 
        file: &str
    ) -> LspResult<Vec<Diagnostic>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.get_diagnostics(file).await
    }

    async fn get_completion(
        &self, 
        file: &str, 
        line: u32, 
        character: u32
    ) -> LspResult<Vec<CompletionItem>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.get_completion(file, line, character).await
    }

    async fn hover(
        &self, 
        file: &str, 
        line: u32, 
        character: u32
    ) -> LspResult<Option<Hover>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.hover(file, line, character).await
    }

    // --- Advanced operations (Phase 2) ------------------

    async fn document_symbol(
        &self,
        file: &str,
    ) -> LspResult<Vec<DocumentSymbol>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.document_symbol(file).await
    }

    async fn workspace_symbol(
        &self,
        query: &str,
    ) -> LspResult<Vec<SymbolInformation>> {
        // Workspace symbol doesn't require a specific file, use any running server
        // or start the first available one
        let servers = self.servers.read().await;
        if let Some((_lang, client)) = servers.iter().next() {
            let c = client.read().await;
            c.workspace_symbol(query).await
        } else {
            Err(LspError::NoServer)
        }
    }

    async fn goto_implementation(
        &self,
        file: &str,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<Location>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.goto_implementation(file, line, character).await
    }

    async fn prepare_call_hierarchy(
        &self,
        file: &str,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<CallHierarchyItem>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.prepare_call_hierarchy(file, line, character).await
    }

    // --- New operations — LspOperations enhancement -----

    async fn code_action(
        &self,
        file: &str,
        range: Range,
        context: CodeActionContext,
    ) -> LspResult<Vec<CodeActionOrCommand>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.code_action(file, range, context).await
    }

    async fn rename_symbol_lsp(
        &self,
        file: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> LspResult<WorkspaceEdit> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c = client.read().await;
        c.rename_symbol(file, line, character, new_name).await
    }
}
