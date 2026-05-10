// ════════════════════════════════════════════════════════════════
// LSP Server Manager — 多语言 Server 生命周期管理
//
// 支持的 LSP Server:
//   - TypeScript: typescript-language-server (tsserver)
//   - Rust: rust-analyzer
//   - Python: pylsp / pyright
//   - Go: gopls
//   - C/C++: clangd
//   - Java: jdtls
//   - JSON: vscode-json-languageserver
// ════════════════════════════════════════════════════════════════

use crate::client::{LspClient, LspError, LspResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 语言 ID (用于文档关联)
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
}

/// 内置 Server 配置表
fn builtin_server_configs() -> Vec<ServerConfig> {
    vec![
        ServerConfig {
            command: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::TypeScript, LanguageId::JavaScript],
            file_patterns: vec!["*.ts".into(), "*.tsx".into(), "*.js".into(), "*.jsx".into()],
        },
        ServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            env_vars: vec![],
            languages: vec![LanguageId::Rust],
            file_patterns: vec!["*.rs".into()],
        },
        ServerConfig {
            command: "pylsp".into(), // 或 pyright-langserver
            args: vec![],
            env_vars: vec![],
            languages: vec![LanguageId::Python],
            file_patterns: vec!["*.py".into()],
        },
        ServerConfig {
            command: "gopls".into(),
            args: vec!["serve".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Go],
            file_patterns: vec!["*.go".into()],
        },
        ServerConfig {
            command: "clangd".into(),
            args: vec![
                "--background-index".into(),
                "--clang-tidy".into(),
                "--completion-style=detailed".into(),
            ],
            env_vars: vec![],
            languages: vec![LanguageId::Cpp, LanguageId::C],
            file_patterns: vec!["*.cpp".into(), "*.c".into(), "*.cc".into(), "*.h".into(), "*.hpp".into()],
        },
        ServerConfig {
            command: "vscode-json-languageserver".into(),
            args: vec!["--stdio".into()],
            env_vars: vec![],
            languages: vec![LanguageId::Json],
            file_patterns: vec!["*.json".into()],
        },
    ]
}

/// LSP Server 管理器
pub struct LspServerManager {
    /// language_id → LspClient
    servers: RwLock<HashMap<String, Arc<RwLock<LspClient>>>>,

    /// 已知的 Server 配置
    configs: Vec<ServerConfig>,

    /// workspace root URI
    workspace_root: String,
}

impl Default for LspServerManager {
    fn default() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
            configs: builtin_server_configs(),
            workspace_root: ".".into(),
        }
    }
}

impl LspServerManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_workspace(mut self, root: &str) -> Self {
        self.workspace_root = root.to_string();
        self
    }

    /// 根据文件路径获取或启动对应的 LSP Server
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
                    tracing::warn!(error = %e, lang = %lang.as_ref(), "Failed to start LSP server");
                    None
                }
            }
        } else {
            None
        }
    }

    /// 手动启动一个 LSP Server
    pub async fn start_server(&self, config: ServerConfig) -> Result<Arc<RwLock<LspClient>>, String> {
        let server_name = config.command.clone();
        
        // 创建并初始化 Client
        let mut client = LspClient::new(server_name.clone());

        // TODO: 实际启动子进程并连接 stdio
        // child = Command::new(&config.command)
        //     .args(&config.args)
        //     .stdin(Stdio::piped())
        //     .stdout(Stdio::piped())
        //     .spawn()?;

        client.initialize().await.map_err(|e| e.to_string())?;

        let client_arc = Arc::new(RwLock::new(client));

        // 注册到所有关联语言
        {
            let mut servers = self.servers.write().await;
            for lang in &config.languages {
                servers.insert(lang.as_ref().to_string(), client_arc.clone());
            }
        }

        tracing::info!(server = %server_name, "LSP server started");

        Ok(client_arc)
    }

    /// 停止所有 Servers
    pub async fn shutdown_all(&self) {
        let servers = self.servers.read().await;
        for (_lang, client) in servers.iter() {
            let mut c = client.write().await;
            let _ = c.shutdown().await;
        }

        self.servers.write().await.clear();
        tracing::info!("All LSP servers shut down");
    }

    /// 列出所有已启动的 Servers
    pub async fn list_running_servers(&self) -> Vec<(String, bool)> {
        let servers = self.servers.read().await;
        servers.iter()
            .map(|(name, _)| (name.clone(), true))
            .collect()
    }

    // ─── 内部方法 ─────────────────────────────────

    async fn find_config_for_language(&self, lang: &LanguageId) -> Option<&ServerConfig> {
        self.configs.iter().find(|c| c.languages.contains(lang))
    }

    fn detect_language_from_path(path: &str) -> LanguageId {
        // 从文件扩展名推断语言
        if let Some(ext) = path.rsplit('.').next() {
            return LanguageId::from(ext);
        }

        // 特殊文件名检测
        let filename = path.rsplit('/').next().unwrap_or(path).rsplit('\\').next().unwrap_or(path);
        match filename {
            "Dockerfile" | "Containerfile" => LanguageId::Shell,
            "Makefile" | "makefile" => LanguageId::Shell,
            "Cargo.toml" => LanguageId::Toml,
            _ => LanguageId::Unknown("plaintext".into()),
        }
    }
}

#[async_trait::async_trait]
impl super::LspOperations for LspServerManager {
    async fn goto_definition(
        &self, file: &str, line: u32, character: u32
    ) -> LspResult<Vec<lsp_types::Location>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c: tokio::sync::RwLockReadGuard<LspClient> = client.read().await;
        c.goto_definition(file, line, character).await
    }

    async fn find_references(
        &self, file: &str, line: u32, character: u32
    ) -> LspResult<Vec<lsp_types::Location>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c: tokio::sync::RwLockReadGuard<LspClient> = client.read().await;
        c.find_references(file, line, character).await
    }

    async fn get_diagnostics(
        &self, file: &str
    ) -> LspResult<Vec<lsp_types::Diagnostic>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c: tokio::sync::RwLockReadGuard<LspClient> = client.read().await;
        c.get_diagnostics(file).await
    }

    async fn get_completion(
        &self, file: &str, line: u32, character: u32
    ) -> LspResult<Vec<lsp_types::CompletionItem>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c: tokio::sync::RwLockReadGuard<LspClient> = client.read().await;
        c.get_completion(file, line, character).await
    }

    async fn hover(
        &self, file: &str, line: u32, character: u32
    ) -> LspResult<Option<lsp_types::Hover>> {
        let client = self.get_or_start_server_for_file(file).await
            .ok_or(LspError::NoServer)?;
        let c: tokio::sync::RwLockReadGuard<LspClient> = client.read().await;
        c.hover(file, line, character).await
    }
}
