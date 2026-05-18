use std::path::PathBuf;
use std::collections::HashSet;
use std::sync::LazyLock;
use regex::Regex;
use crate::tui::ui_blocks::{CommandBlock, ActionType};

pub struct ContextActionGenerator {
    file_registry: FileRegistry,
    git_status: GitStatusCache,
    recent_paths: LruSet<PathBuf>,
    config: ContextActionConfig,
}

#[derive(Debug, Clone)]
pub struct ContextActionConfig {
    pub max_suggestions: usize,
    pub min_confidence: f64,
    pub enable_git_actions: bool,
    pub enable_file_actions: bool,
    pub enable_error_fixes: bool,
    pub enable_url_actions: bool,
}

impl Default for ContextActionConfig {
    fn default() -> Self {
        Self {
            max_suggestions: 8,
            min_confidence: 0.5,
            enable_git_actions: true,
            enable_file_actions: true,
            enable_error_fixes: true,
            enable_url_actions: true,
        }
    }
}

struct FileRegistry {
    known_files: HashSet<PathBuf>,
}

impl Default for FileRegistry {
    fn default() -> Self {
        Self { known_files: HashSet::new() }
    }
}

struct GitStatusCache {
    branches: Vec<String>,
    dirty_files: HashSet<PathBuf>,
}

impl Default for GitStatusCache {
    fn default() -> Self {
        Self { branches: vec![], dirty_files: HashSet::new() }
    }
}

struct LruSet<T> {
    capacity: usize,
    items: Vec<T>,
}

impl<T: PartialEq + Clone> LruSet<T> {
    fn new(capacity: usize) -> Self {
        Self { capacity, items: Vec::new() }
    }

    fn touch(&mut self, item: T) {
        if let Some(pos) = self.items.iter().position(|x| x == &item) {
            self.items.remove(pos);
        } else if self.items.len() >= self.capacity {
            self.items.remove(0);
        }
        self.items.push(item);
    }

    fn contains(&self, item: &T) -> bool {
        self.items.contains(item)
    }
}

#[derive(Debug, Clone)]
pub struct AnalyzedContext {
    pub file_paths: Vec<PathWithContext>,
    pub urls: Vec<UrlWithContext>,
    pub git_refs: Vec<GitRefWithContext>,
    pub errors: Vec<ErrorWithContext>,
    pub code_symbols: Vec<CodeSymbolRef>,
    pub package_refs: Vec<PackageRef>,
    pub docker_images: Vec<DockerImageRef>,
    pub commands: Vec<RecognizedCommand>,
}

/// 带上下文的文件路径
#[derive(Debug, Clone)]
pub struct PathWithContext {
    pub path: PathBuf,
    pub exists: bool,
    pub confidence: f64,
    pub context: String,
}

#[derive(Debug, Clone)]
pub struct UrlWithContext {
    pub url: url::Url,
    pub context: String,
    pub is_documentation: bool,
    pub is_api_endpoint: bool,
}

/// 带上下文的 Git 引用
#[derive(Debug, Clone)]
pub struct GitRefWithContext {
    pub ref_type: GitRefType,
    pub value: String,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GitRefType { CommitHash, BranchName, TagName, RemoteUrl }

#[derive(Debug, Clone)]
pub struct ErrorWithContext {
    pub pattern: String,
    pub error_type: ErrorType,
    pub severity: ErrorSeverity,
    pub suggested_fixes: Vec<SuggestedFix>,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorType { CompilationError, RuntimeError, NetworkError, PermissionError, NotFound, Timeout }

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorSeverity { Critical, Warning, Info }

#[derive(Debug, Clone)]
pub struct SuggestedFix {
    pub command: Option<String>,
    pub description: String,
    pub auto_applicable: bool,
    pub confidence: f64,
}

/// 代码符号引用
#[derive(Debug, Clone)]
pub struct CodeSymbolRef {
    pub name: String,
    pub kind: SymbolKind,
    pub language: Option<String>,
    pub is_defined: bool,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind { Function, Class, Variable, Module, Type, Method, Trait, Interface }

/// 包引用
#[derive(Debug, Clone)]
pub struct PackageRef {
    pub name: String,
    pub manager: PackageManager,
    pub is_installed: Option<bool>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageManager { Npm, Pip, Cargo, Brew, Apt, Yarn, Pnpm, GoMod, Nuget, Gem }

/// Docker 镜像引用
#[derive(Debug, Clone)]
pub struct DockerImageRef {
    pub image_name: String,
    pub tag: Option<String>,
    pub is_local: Option<bool>,
}

/// 识别的命令
#[derive(Debug, Clone)]
pub struct RecognizedCommand {
    pub command: String,
    pub category: CommandCategory,
    pub risk_level: RiskLevel,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandCategory { FileOperation, Git, Docker, Network, Process, Build, Test, Deployment }

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq)]
pub enum RiskLevel { Safe, Low, Medium, High, Destructive }

pub struct SuggestedAction {
    pub label: String,
    pub icon: char,
    pub action: ActionType,
    pub confidence: f64,
    pub source: ActionSource,
    pub reason: String,
    pub group: ActionGroup,
    pub shortcut_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq)]
pub enum ActionSource { PatternMatch, LlmGenerated, HistoryBased, CommunityPopular }

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ActionGroup { FileOperations, Navigation, FixActions, GitActions, SearchActions, ExternalTools }

struct ErrorFixRule {
    pattern: &'static str,
    fixes: Box<[SuggestedFixTemplate]>,
}

struct SuggestedFixTemplate {
    command: Option<&'static str>,
    description: &'static str,
    auto_applicable: bool,
    confidence: f64,
}

static ERROR_FIX_RULES: LazyLock<Vec<ErrorFixRule>> = LazyLock::new(|| vec![
    ErrorFixRule {
        pattern: r"(?i)(EADDRINUSE|address already in use|port.*in use)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("lsof -i :{port} | grep LISTEN | awk '{print $2}' | xargs kill"), description: "Kill process using port", auto_applicable: false, confidence: 0.9 },
            SuggestedFixTemplate { command: Some("npx kill-port {port}"), description: "Use kill-port tool", auto_applicable: false, confidence: 0.85 },
            SuggestedFixTemplate { command: Some("fuser -k {port}/tcp"), description: "Kill via fuser", auto_applicable: false, confidence: 0.8 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(permission denied|EACCES|access denied)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("sudo {original_command}"), description: "Run with sudo", auto_applicable: false, confidence: 0.85 },
            SuggestedFixTemplate { command: Some("chmod +x {file} && {original_command}"), description: "Fix permissions then run", auto_applicable: false, confidence: 0.75 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(ENOENT|no such file or directory|file not found)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("ls -la $(dirname {file})"), description: "Check parent directory", auto_applicable: false, confidence: 0.7 },
            SuggestedFixTemplate { command: Some("touch {file}"), description: "Create missing file", auto_applicable: false, confidence: 0.6 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(ENOTDIR|not a directory|is a file)",
        fixes: Box::new([
            SuggestedFixTemplate { command: None, description: "Verify path is not used as directory", auto_applicable: false, confidence: 0.75 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(EISDIR|is a directory|operation on directory)",
        fixes: Box::new([
            SuggestedFixTemplate { command: None, description: "Target path is a directory, use -r for recursive", auto_applicable: false, confidence: 0.78 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(ENOMEM|out of memory|cannot allocate|OOM killed)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("free -h"), description: "Check memory usage", auto_applicable: true, confidence: 0.85 },
            SuggestedFixTemplate { command: None, description: "Close other applications or increase swap", auto_applicable: false, confidence: 0.7 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(ECONNREFUSED|connection refused|could not connect)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("nc -zv {host} {port}"), description: "Test connectivity to host:port", auto_applicable: true, confidence: 0.88 },
            SuggestedFixTemplate { command: Some("curl -I http://{host}:{port}/health"), description: "Health check endpoint", auto_applicable: true, confidence: 0.82 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(ETIMEDOUT|timed out|connection timed out|timeout exceeded)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("ping -c 4 {host}"), description: "Ping target to check latency", auto_applicable: true, confidence: 0.83 },
            SuggestedFixTemplate { command: None, description: "Increase timeout or check network/firewall", auto_applicable: false, confidence: 0.72 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(module not found|cannot find module|unresolved dependency)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("npm install {module_name}"), description: "Install missing npm package", auto_applicable: false, confidence: 0.88 },
            SuggestedFixTemplate { command: Some("pip install {module_name}"), description: "Install missing Python package", auto_applicable: false, confidence: 0.86 },
            SuggestedFixTemplate { command: Some("cargo add {module_name}"), description: "Add missing Cargo dependency", auto_applicable: false, confidence: 0.87 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(cannot compile|compilation failed|build error|syntax error)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("cargo build 2>&1 | head -50"), description: "View detailed compilation errors", auto_applicable: true, confidence: 0.9 },
            SuggestedFixTemplate { command: Some("npm run build -- --verbose"), description: "Verbose build output", auto_applicable: true, confidence: 0.82 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(certificate.*error|SSL.*error|self signed certificate|CERT_)",
        fixes: Box::new([
            SuggestedFixTemplate { command: None, description: "Check system CA certificates or use NODE_TLS_REJECT_UNAUTHORIZED=0 for dev", auto_applicable: false, confidence: 0.73 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(disk full|no space left|quota exceeded|ENOSPC)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("df -h"), description: "Check disk space usage", auto_applicable: true, confidence: 0.92 },
            SuggestedFixTemplate { command: Some("du -sh * | sort -hr | head -10"), description: "Find largest files/dirs", auto_applicable: true, confidence: 0.88 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(exit status 1|exit code 1|non-zero exit code)",
        fixes: Box::new([
            SuggestedFixTemplate { command: None, description: "Check stderr output for specific error details", auto_applicable: false, confidence: 0.65 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(git.*conflict|merge conflict|both modified)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("git diff --name-only --diff-filter=U"), description: "List conflicted files", auto_applicable: true, confidence: 0.91 },
            SuggestedFixTemplate { command: Some("git checkout --theirs {file}"), description: "Accept theirs version", auto_applicable: false, confidence: 0.75 },
            SuggestedFixTemplate { command: Some("git checkout --ours {file}"), description: "Accept ours version", auto_applicable: false, confidence: 0.75 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(detached HEAD|detached head state)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("git checkout {branch}"), description: "Checkout a branch", auto_applicable: false, confidence: 0.88 },
            SuggestedFixTemplate { command: Some("git switch -"), description: "Switch back to previous branch", auto_applicable: false, confidence: 0.85 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(docker.*not running|docker daemon|Cannot connect to Docker)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("docker info"), description: "Check Docker daemon status", auto_applicable: true, confidence: 0.89 },
            SuggestedFixTemplate { command: Some("systemctl start docker"), description: "Start Docker service", auto_applicable: false, confidence: 0.84 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(image not found|pull access denied|manifest unknown)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("docker pull {image}"), description: "Pull Docker image", auto_applicable: false, confidence: 0.87 },
            SuggestedFixTemplate { command: Some("docker images | grep {image}"), description: "Check local images", auto_applicable: true, confidence: 0.8 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(EPIPE|broken pipe|signal SIGPIPE)",
        fixes: Box::new([
            SuggestedFixTemplate { command: None, description: "Pipe reader closed early; check downstream command exit", auto_applicable: false, confidence: 0.7 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(too many open files|EMFILE|ulimit|file descriptor limit)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("ulimit -n"), description: "Check current file descriptor limit", auto_applicable: true, confidence: 0.86 },
            SuggestedFixTemplate { command: Some("ulimit -n 65535"), description: "Increase fd limit (requires shell restart)", auto_applicable: false, confidence: 0.78 },
        ]),
    },
    ErrorFixRule {
        pattern: r"(?i)(command not found|executable not found|is not recognized)",
        fixes: Box::new([
            SuggestedFixTemplate { command: Some("which {command_name} || where {command_name}"), description: "Locate executable in PATH", auto_applicable: true, confidence: 0.84 },
            SuggestedFixTemplate { command: Some("apt install {command_name} || brew install {command_name}"), description: "Install missing tool", auto_applicable: false, confidence: 0.76 },
        ]),
    },
]);

impl ContextActionGenerator {
    pub fn new(config: ContextActionConfig) -> Self {
        Self {
            file_registry: FileRegistry::default(),
            git_status: GitStatusCache::default(),
            recent_paths: LruSet::new(50),
            config,
        }
    }

    pub fn analyze(&self, block_content: &str) -> AnalyzedContext {
        AnalyzedContext {
            file_paths: Self::extract_file_paths(block_content),
            urls: Self::extract_urls(block_content),
            git_refs: Self::extract_git_refs(block_content),
            errors: Self::detect_error_patterns(block_content),
            code_symbols: Self::extract_code_symbols(block_content),
            package_refs: Self::detect_packages(block_content),
            docker_images: Self::detect_docker_images(block_content),
            commands: Self::recognize_commands(block_content),
        }
    }

    fn extract_file_paths(text: &str) -> Vec<PathWithContext> {
        let mut results = Vec::new();
        let patterns = [
            (r#"[A-Z]:\\[^\s"'`<>]+\.\w+"#, 0.95),
            (r"/(?:[\w\-./]+/)*[\w\-./]+\.\w+", 0.92),
            (r#"\./[^\s"'`<>"+"#, 0.88),
            (r#"\.\./[^\s"'`<>"+"#, 0.88),
            (r"~/(?:[\w\-./]+/)*[\w\-./]+", 0.85),
            (r#"[^\s"'`<>@:/]+\.(?:rs|py|js|ts|go|java|rb|sh|yaml|yml|toml|json|md|html|css|sql|mod|lock)"#, 0.80),
        ];
        let seen: HashSet<String> = HashSet::new();
        for (pat, base_conf) in patterns {
            if let Ok(re) = Regex::new(pat) {
                for cap in re.captures_iter(text) {
                    let raw = cap.get(0).unwrap().as_str().to_string();
                    if !seen.contains(&raw) && raw.len() < 1024 {
                        let path = PathBuf::from(&raw);
                        let exists = path.exists();
                        let conf: f64 = if exists { base_conf + 0.04 } else { base_conf };
                        results.push(PathWithContext {
                            path,
                            exists,
                            confidence: (conf).min(1.0_f64),
                            context: "mentioned in content".to_string(),
                        });
                    }
                }
            }
        }
        results
    }

    fn extract_urls(text: &str) -> Vec<UrlWithContext> {
        let mut results = Vec::new();
        let re = Regex::new(r#"https?://[^\s"'<>]+"#).unwrap_or_else(|_| Regex::new("").unwrap());
        for cap in re.captures_iter(text) {
            let raw = cap.get(0).unwrap().as_str();
            if let Ok(parsed) = url::Url::parse(raw) {
                let host = parsed.host_str().unwrap_or("");
                let is_doc = host.contains("docs") || host.contains("stackoverflow") || host.contains("dev.to") || parsed.path().contains("/doc");
                let is_api = raw.contains("/api/") || raw.contains("/v1") || raw.contains("/v2") || raw.contains("rest");
                results.push(UrlWithContext {
                    url: parsed,
                    context: "found in text".to_string(),
                    is_documentation: is_doc,
                    is_api_endpoint: is_api,
                });
            }
        }
        results
    }

    fn extract_git_refs(text: &str) -> Vec<GitRefWithContext> {
        let mut results = Vec::new();
        let commit_re = Regex::new(r"\b([a-fA-F0-9]{7,40})\b").unwrap();
        for cap in commit_re.captures_iter(text) {
            let val = cap.get(1).unwrap().as_str().to_string();
            if !text.contains(&format!("{}://", &val[..val.len().min(4)])) {
                results.push(GitRefWithContext {
                    ref_type: GitRefType::CommitHash,
                    value: val,
                    context: "hash reference".to_string(),
                });
            }
        }
        let branch_re = Regex::new(r"(?:branch|checkout|switch|merge into|from)\s+([\w\-./]+)").unwrap();
        for cap in branch_re.captures_iter(text) {
            results.push(GitRefWithContext {
                ref_type: GitRefType::BranchName,
                value: cap.get(1).unwrap().as_str().to_string(),
                context: "branch operation".to_string(),
            });
        }
        let tag_re = Regex::new(r"(?:tag|version|v)\s*=?\s*(v?[\d][\w.]*)").unwrap();
        for cap in tag_re.captures_iter(text) {
            results.push(GitRefWithContext {
                ref_type: GitRefType::TagName,
                value: cap.get(1).unwrap().as_str().to_string(),
                context: "tag reference".to_string(),
            });
        }
        let remote_re = Regex::new(r"(github\.com|gitlab\.com|bitbucket\.org)[/\w\-._]+").unwrap();
        for cap in remote_re.captures_iter(text) {
            results.push(GitRefWithContext {
                ref_type: GitRefType::RemoteUrl,
                value: cap.get(0).unwrap().as_str().to_string(),
                context: "remote repository".to_string(),
            });
        }
        results
    }

    fn detect_error_patterns(text: &str) -> Vec<ErrorWithContext> {
        let mut errors = Vec::new();
        for rule in ERROR_FIX_RULES.iter() {
            if let Ok(re) = Regex::new(rule.pattern) {
                if re.is_match(text) {
                    let severity = if re.as_str().contains("critical") || re.as_str().contains("fatal") {
                        ErrorSeverity::Critical
                    } else if re.as_str().contains("warn") {
                        ErrorSeverity::Warning
                    } else {
                        ErrorSeverity::Info
                    };
                    let error_type = Self::classify_error(rule.pattern);
                    let suggested_fixes: Vec<SuggestedFix> = rule.fixes.iter().map(|tmpl| SuggestedFix {
                        command: tmpl.command.map(|c| c.to_string()),
                        description: tmpl.description.to_string(),
                        auto_applicable: tmpl.auto_applicable,
                        confidence: tmpl.confidence,
                    }).collect();
                    errors.push(ErrorWithContext {
                        pattern: rule.pattern.to_string(),
                        error_type,
                        severity,
                        suggested_fixes,
                        context: "detected in text".to_string(),
                    });
                }
            }
        }
        errors
    }

    fn classify_error(pattern: &str) -> ErrorType {
        let lower = pattern.to_lowercase();
        if lower.contains("compile") || lower.contains("syntax") || lower.contains("build error") {
            ErrorType::CompilationError
        } else if lower.contains("econn") || lower.contains("timeout") || lower.contains("network") || lower.contains("ssl") || lower.contains("cert") {
            ErrorType::NetworkError
        } else if lower.contains("permission") || lower.contains("eacces") || lower.contains("denied") {
            ErrorType::PermissionError
        } else if lower.contains("enotdir") || lower.contains("enoent") || lower.contains("not found") {
            ErrorType::NotFound
        } else if lower.contains("timeout") || lower.contains("etimedout") {
            ErrorType::Timeout
        } else {
            ErrorType::RuntimeError
        }
    }

    fn extract_code_symbols(text: &str) -> Vec<CodeSymbolRef> {
        let mut symbols = Vec::new();
        let func_re = Regex::new(r"(?:fn|def|function|func)\s+(\w+)").unwrap();
        for cap in func_re.captures_iter(text) {
            symbols.push(CodeSymbolRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                kind: SymbolKind::Function,
                language: Self::infer_language(text),
                is_defined: true,
                context: "function definition".to_string(),
            });
        }
        let class_re = Regex::new(r"(?:class|struct|interface|type\s+\w+\s+interface)\s+(\w+)").unwrap();
        for cap in class_re.captures_iter(text) {
            symbols.push(CodeSymbolRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                kind: SymbolKind::Class,
                language: Self::infer_language(text),
                is_defined: true,
                context: "class/struct definition".to_string(),
            });
        }
        let var_re = Regex::new(r"(?:let|const|var|mut)\s+(\w+)").unwrap();
        for cap in var_re.captures_iter(text) {
            symbols.push(CodeSymbolRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                kind: SymbolKind::Variable,
                language: Self::infer_language(text),
                is_defined: true,
                context: "variable declaration".to_string(),
            });
        }
        let import_re = Regex::new(r#"(?:import|use|require|from)\s+['"]?([\w/:.]+)['"]?"#).unwrap();
        for cap in import_re.captures_iter(text) {
            symbols.push(CodeSymbolRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                kind: SymbolKind::Module,
                language: Self::infer_language(text),
                is_defined: false,
                context: "import statement".to_string(),
            });
        }
        symbols
    }

    fn infer_language(text: &str) -> Option<String> {
        if text.contains("fn ") && (text.contains("let ") || text.contains("mut ")) { return Some("rust".into()); }
        if text.contains("def ") && text.contains(":") { return Some("python".into()); }
        if text.contains("function ") || text.contains("const ") && text.contains("=>") { return Some("javascript".into()); }
        if text.contains("func ") && text.contains("package ") { return Some("go".into()); }
        if text.contains("public class") || text.contains("private ") { return Some("java".into()); }
        None
    }

    fn detect_packages(text: &str) -> Vec<PackageRef> {
        let mut pkgs = Vec::new();
        let npm_re = Regex::new(r####"""(?:"|')(@?[\w\-./]+@?[\w\-./]*)["']\s*:?\s*["'][^"']*["']|(?:npm install|yarn add|pnpm add)\s+(@?[\w\-./@]+))"""####).unwrap();
        for cap in npm_re.captures_iter(text) {
            let name = cap.get(1).or_else(|| cap.get(2)).map(|m| m.as_str()).unwrap_or_default().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() && !name.starts_with("http") {
                pkgs.push(PackageRef { name, manager: PackageManager::Npm, is_installed: None, version: None });
            }
        }
        let pip_re = Regex::new(r"(?:pip install|pip3 install)\s+([\w\-\._=<>!]+)").unwrap();
        for cap in pip_re.captures_iter(text) {
            pkgs.push(PackageRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                manager: PackageManager::Pip,
                is_installed: None,
                version: None,
            });
        }
        let cargo_re = Regex::new(r#"(\w+(?:-\w+)*)\s*=\s*["']([^"']+)["']"#).unwrap();
        for cap in cargo_re.captures_iter(text) {
            pkgs.push(PackageRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                manager: PackageManager::Cargo,
                is_installed: None,
                version: Some(cap.get(2).unwrap().as_str().to_string()),
            });
        }
        let brew_re = Regex::new(r"(?:brew install)\s+([\w@./]+)").unwrap();
        for cap in brew_re.captures_iter(text) {
            pkgs.push(PackageRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                manager: PackageManager::Brew,
                is_installed: None,
                version: None,
            });
        }
        let go_re = Regex::new(r####"""(?:(?:go get|import)\s+)"([^"]+)""""####).unwrap();
        for cap in go_re.captures_iter(text) {
            pkgs.push(PackageRef {
                name: cap.get(1).unwrap().as_str().to_string(),
                manager: PackageManager::GoMod,
                is_installed: None,
                version: None,
            });
        }
        pkgs
    }

    fn detect_docker_images(text: &str) -> Vec<DockerImageRef> {
        let mut images = Vec::new();
        let re = Regex::new(r"(?:docker pull|docker run|image:\s*|FROM\s+)?([\w\-./]+(?::[\w.\-]+)?)").unwrap();
        for cap in re.captures_iter(text) {
            let full = cap.get(1).unwrap().as_str().to_string();
            if let Some((name, tag)) = full.split_once(':') {
                if !name.contains('/') || name.contains('/') && name.split('/').count() <= 3 {
                    images.push(DockerImageRef {
                        image_name: name.to_string(),
                        tag: Some(tag.to_string()),
                        is_local: None,
                    });
                }
            } else if full.contains('/') && full.matches('/').count() <= 2 {
                images.push(DockerImageRef {
                    image_name: full,
                    tag: None,
                    is_local: None,
                });
            }
        }
        images
    }

    fn recognize_commands(text: &str) -> Vec<RecognizedCommand> {
        let mut cmds = Vec::new();
        let patterns: &[(&str, CommandCategory, RiskLevel)] = &[
            (r"(?i)(rm\s+-rf|--force)\s+(/|[~$])", CommandCategory::FileOperation, RiskLevel::Destructive),
            (r"(?i)\b(git\s+(commit|push|pull|merge|rebase|reset|stash|checkout|log|status|diff|add|branch|fetch))\b", CommandCategory::Git, RiskLevel::Low),
            (r"(?i)\b(docker\s+(run|build|push|pull|compose|ps|logs|exec|stop|start|rm|rmi))\b", CommandCategory::Docker, RiskLevel::Medium),
            (r"(?i)\b(curl|wget|httpie|ssh|scp|rsync|ftp)\b", CommandCategory::Network, RiskLevel::Safe),
            (r"(?i)\b(kill|pkill|killall|systemctl|service)\b", CommandCategory::Process, RiskLevel::Medium),
            (r"(?i)\b(cargo build|make|cmake|npm run|gradle|maven|gcc|g\+\+)\b", CommandCategory::Build, RiskLevel::Safe),
            (r"(?i)\b(npm test|pytest|cargo test|go test|jest|mocha|vitest)\b", CommandCategory::Test, RiskLevel::Safe),
            (r"(?i)\b(kubectl|helm|terraform|ansible|cdk|pulumi)\b", CommandCategory::Deployment, RiskLevel::High),
        ];
        for (pat, cat, risk) in patterns {
            if let Ok(re) = Regex::new(pat) {
                for cap in re.captures_iter(text) {
                    cmds.push(RecognizedCommand {
                        command: cap.get(0).unwrap().as_str().to_string(),
                        category: cat.clone(),
                        risk_level: risk.clone(),
                    });
                }
            }
        }
        cmds
    }

    pub fn suggest_actions(&self, context: &AnalyzedContext, _block: &CommandBlock) -> Vec<SuggestedAction> {
        let mut actions = Vec::new();
        if self.config.enable_file_actions {
            actions.extend(self.generate_file_actions(&context.file_paths));
        }
        if self.config.enable_error_fixes {
            actions.extend(self.generate_error_fixes(&context.errors));
        }
        if self.config.enable_git_actions {
            actions.extend(self.generate_git_actions(&context.git_refs));
        }
        if self.config.enable_url_actions {
            actions.extend(self.generate_url_actions(&context.urls));
        }
        actions.extend(self.generate_package_actions(&context.package_refs));
        actions.extend(self.generate_search_actions(&context.code_symbols));
        Self::deduplicate_actions(&mut actions);
        Self::rank_and_filter(&mut actions, self.config.max_suggestions, self.config.min_confidence);
        actions
    }

    fn generate_file_actions(&self, paths: &[PathWithContext]) -> Vec<SuggestedAction> {
        paths.iter().take(5).filter(|p| p.confidence >= 0.5).map(|p| {
            let label = if p.exists { format!("Open {}", p.path.display()) } else { format!("Create {}", p.path.file_name().unwrap_or_default().to_string_lossy()) };
            SuggestedAction {
                label,
                icon: if p.exists { '📄' } else { '✏' },
                action: ActionType::Edit,
                confidence: p.confidence,
                source: ActionSource::PatternMatch,
                reason: format!("File path detected ({})", p.context),
                group: ActionGroup::FileOperations,
                shortcut_hint: Some("Enter".to_string()),
            }
        }).collect()
    }

    fn generate_error_fixes(&self, errors: &[ErrorWithContext]) -> Vec<SuggestedAction> {
        let mut actions = Vec::new();
        for err in errors.iter().take(5) {
            for fix in err.suggested_fixes.iter().take(2) {
                actions.push(SuggestedAction {
                    label: fix.description.clone(),
                    icon: '🔧',
                    action: ActionType::Custom(if let Some(cmd) = &fix.command { cmd.clone() } else { "fix".into() }),
                    confidence: fix.confidence * match err.severity {
                        ErrorSeverity::Critical => 1.1,
                        ErrorSeverity::Warning => 1.0,
                        ErrorSeverity::Info => 0.9,
                    },
                    source: ActionSource::PatternMatch,
                    reason: format!("Error: {:?} ({:?})", err.error_type, err.severity),
                    group: ActionGroup::FixActions,
                    shortcut_hint: None,
                });
            }
        }
        actions
    }

    fn generate_git_actions(&self, refs: &[GitRefWithContext]) -> Vec<SuggestedAction> {
        refs.iter().take(4).map(|r| {
            let (label, action) = match r.ref_type {
                GitRefType::CommitHash => (format!("View commit {}", &r.value[..r.value.len().min(8)]), ActionType::Custom(format!("git show {}", r.value))),
                GitRefType::BranchName => (format!("Switch to {}", r.value), ActionType::Custom(format!("git checkout {}", r.value))),
                GitRefType::TagName => (format!("Checkout tag {}", r.value), ActionType::Custom(format!("git checkout tags/{}", r.value))),
                GitRefType::RemoteUrl => (format!("Open remote {}", r.value), ActionType::Custom(format!("open {}", r.value))),
            };
            SuggestedAction {
                label,
                icon: '🔀',
                action,
                confidence: 0.8,
                source: ActionSource::PatternMatch,
                reason: format!("Git {:?}: {}", r.ref_type, r.context),
                group: ActionGroup::GitActions,
                shortcut_hint: None,
            }
        }).collect()
    }

    fn generate_url_actions(&self, urls: &[UrlWithContext]) -> Vec<SuggestedAction> {
        urls.iter().take(3).map(|u| {
            let label = if u.is_api_endpoint { format!("API: {}", u.url.host_str().unwrap_or("?")) }
                         else if u.is_documentation { format!("Docs: {}", u.url.host_str().unwrap_or("?")) }
                         else { format!("Open link") };
            SuggestedAction {
                label,
                icon: if u.is_api_endpoint { '🔗' } else if u.is_documentation { '📖' } else { '🌐' },
                action: ActionType::Custom(u.url.as_str().to_string()),
                confidence: 0.85,
                source: ActionSource::PatternMatch,
                reason: u.context.clone(),
                group: ActionGroup::ExternalTools,
                shortcut_hint: Some("Ctrl+O".to_string()),
            }
        }).collect()
    }

    fn generate_package_actions(&self, packages: &[PackageRef]) -> Vec<SuggestedAction> {
        packages.iter().take(3).map(|pkg| {
            let install_cmd = match pkg.manager {
                PackageManager::Npm => format!("npm install {}", pkg.name),
                PackageManager::Pip => format!("pip install {}", pkg.name),
                PackageManager::Cargo => format!("cargo add {}", pkg.name),
                PackageManager::Brew => format!("brew install {}", pkg.name),
                PackageManager::Yarn => format!("yarn add {}", pkg.name),
                PackageManager::Pnpm => format!("pnpm add {}", pkg.name),
                PackageManager::GoMod => format!("go get {}", pkg.name),
                PackageManager::Apt => format!("apt install {}", pkg.name),
                PackageManager::Nuget => format!("dotnet add package {}", pkg.name),
                PackageManager::Gem => format!("gem install {}", pkg.name),
            };
            SuggestedAction {
                label: format!("Install {} ({:?})", pkg.name, pkg.manager),
                icon: '📦',
                action: ActionType::Custom(install_cmd),
                confidence: 0.82,
                source: ActionSource::CommunityPopular,
                reason: format!("{:?} package detected", pkg.manager),
                group: ActionGroup::ExternalTools,
                shortcut_hint: None,
            }
        }).collect()
    }

    fn generate_search_actions(&self, symbols: &[CodeSymbolRef]) -> Vec<SuggestedAction> {
        symbols.iter().take(4).filter(|s| s.kind != SymbolKind::Variable).map(|sym| {
            let label = match sym.kind {
                SymbolKind::Function => format!("Go to function {}", sym.name),
                SymbolKind::Class => format!("Go to class {}", sym.name),
                SymbolKind::Module => format!("Open module {}", sym.name),
                SymbolKind::Method => format!("Go to method {}", sym.name),
                SymbolKind::Trait => format!("Go to trait {}", sym.name),
                SymbolKind::Interface => format!("Go to interface {}", sym.name),
                _ => format!("Find {}", sym.name),
            };
            SuggestedAction {
                label,
                icon: match sym.kind {
                    SymbolKind::Function => 'ƒ',
                    SymbolKind::Class => 'C',
                    SymbolKind::Module => 'M',
                    SymbolKind::Method => 'm',
                    SymbolKind::Trait => 'T',
                    SymbolKind::Interface => 'I',
                    _ => 'S',
                },
                action: ActionType::Search,
                confidence: if sym.is_defined { 0.78 } else { 0.65 },
                source: ActionSource::PatternMatch,
                reason: format!("{:?} symbol in {:?}", sym.kind, sym.language.clone().unwrap_or_default()),
                group: ActionGroup::SearchActions,
                shortcut_hint: Some("Ctrl+F".to_string()),
            }
        }).collect()
    }

    fn rank_and_filter(actions: &mut Vec<SuggestedAction>, max_count: usize, min_confidence: f64) {
        actions.sort_by(|a, b| {
            b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.source.clone().cmp(&a.source))
        });
        actions.retain(|a| a.confidence >= min_confidence);
        actions.truncate(max_count);
    }

    fn deduplicate_actions(actions: &mut Vec<SuggestedAction>) {
        let mut seen: HashSet<String> = HashSet::new();
        actions.retain(|a| seen.insert(a.label.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_absolute_windows_path() {
        let paths = ContextActionGenerator::extract_file_paths(r"Error at C:\Users\test\src\main.rs line 42");
        assert!(paths.iter().any(|p| p.path.to_string_lossy().contains("main.rs")));
    }

    #[test]
    fn test_extract_unix_absolute_path() {
        let paths = ContextActionGenerator::extract_file_paths("See /home/user/project/src/lib.rs for details");
        assert!(paths.iter().any(|p| p.path.to_string_lossy().contains("lib.rs")));
    }

    #[test]
    fn test_extract_relative_path() {
        let paths = ContextActionGenerator::extract_file_paths("editing ./src/utils/helpers.ts now");
        assert!(paths.iter().any(|p| p.path.to_string_lossy().contains("helpers.ts")));
    }

    #[test]
    fn test_extract_parent_relative_path() {
        let paths = ContextActionGenerator::extract_file_paths("check ../config/app.yaml settings");
        assert!(paths.iter().any(|p| p.path.to_string_lossy().contains("app.yaml")));
    }

    #[test]
    fn test_url_extraction_and_classification() {
        let urls = ContextActionGenerator::extract_urls("See https://docs.rust-lang.org/std/ and https://api.github.com/repos");
        assert_eq!(urls.len(), 2);
        assert!(urls[0].is_documentation);
        assert!(urls[1].is_api_endpoint);
    }

    #[test]
    fn test_git_commit_hash_extraction() {
        let refs = ContextActionGenerator::extract_git_refs("Commit a1b2c3d4e5f6 was pushed to main");
        assert!(refs.iter().any(|r| r.ref_type == GitRefType::CommitHash && r.value == "a1b2c3d4e5f6"));
    }

    #[test]
    fn test_git_branch_name_extraction() {
        let refs = ContextActionGenerator::extract_git_refs("Switch to feature/new-login-page branch");
        assert!(refs.iter().any(|r| r.ref_type == GitRefType::BranchName && r.value.contains("feature")));
    }

    #[test]
    fn test_git_tag_extraction() {
        let refs = ContextActionGenerator::extract_git_refs("Release v2.1.0 is ready for deployment");
        assert!(refs.iter().any(|r| r.ref_type == GitRefType::TagName && r.value.contains("2.1.0")));
    }

    #[test]
    fn test_error_pattern_port_in_use() {
        let errors = ContextActionGenerator::detect_error_patterns("Error: EADDRINUSE address already in use, port 3000 is occupied");
        assert!(!errors.is_empty());
        assert!(errors[0].suggested_fixes.iter().any(|f| f.description.contains("port")));
    }

    #[test]
    fn test_error_pattern_permission_denied() {
        let errors = ContextActionGenerator::detect_error_patterns("Error: Permission denied (EACCES) when opening /etc/config");
        assert!(!errors.is_empty());
        assert!(errors[0].error_type == ErrorType::PermissionError);
    }

    #[test]
    fn test_error_pattern_module_not_found() {
        let errors = ContextActionGenerator::detect_error_patterns("Error: Cannot find module 'lodash'");
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.suggested_fixes.iter().any(|f| f.command.as_deref().unwrap_or("").contains("npm"))));
    }

    #[test]
    fn test_error_pattern_disk_full() {
        let errors = ContextActionGenerator::detect_error_patterns("Write failed: No space left on device (ENOSPC)");
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.suggested_fixes.iter().any(|f| f.command.as_deref().unwrap_or("").contains("df"))));
    }

    #[test]
    fn test_code_symbol_function_extraction() {
        let symbols = ContextActionGenerator::extract_code_symbols("fn calculate_total(items: Vec<Item>) -> f64 { ... }");
        assert!(symbols.iter().any(|s| s.name == "calculate_total" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_code_symbol_class_extraction() {
        let symbols = ContextActionGenerator::extract_code_symbols("class UserManager { constructor() {} }");
        assert!(symbols.iter().any(|s| s.name == "UserManager" && s.kind == SymbolKind::Class));
    }

    #[test]
    fn test_npm_package_detection() {
        let pkgs = ContextActionGenerator::detect_packages("\"react\": \"^18.2.0\", \"lodash\": \"^4.17.21\"");
        assert!(pkgs.iter().any(|p| p.name == "react" && p.manager == PackageManager::Npm));
    }

    #[test]
    fn test_pip_package_detection() {
        let pkgs = ContextActionGenerator::detect_packages("pip install requests numpy pandas");
        assert!(pkgs.iter().any(|p| p.name == "requests" && p.manager == PackageManager::Pip));
    }

    #[test]
    fn test_cargo_package_detection() {
        let pkgs = ContextActionGenerator::detect_packages("serde = { version = \"1.0\", features = [\"derive\"] }");
        assert!(pkgs.iter().any(|p| p.name == "serde" && p.manager == PackageManager::Cargo && p.version.as_deref() == Some("1.0")));
    }

    #[test]
    fn test_docker_image_detection() {
        let images = ContextActionGenerator::detect_docker_images("FROM node:18-alpine\nRUN docker pull postgres:15");
        assert!(images.iter().any(|i| i.image_name == "node" && i.tag.as_deref() == Some("18-alpine")));
    }

    #[test]
    fn test_action_generation_priority_sorting() {
        let r#gen = ContextActionGenerator::new(ContextActionConfig::default());
        let ctx = r#gen.analyze("Error: EADDRINUSE port 3000 in use. See /tmp/log.txt for details.");
        let block = CommandBlock::new(BlockType::UserInput, "test");
        let actions = r#gen.suggest_actions(&ctx, &block);
        assert!(!actions.is_empty());
        for w in actions.windows(2) {
            assert!(w[0].confidence >= w[1].confidence, "actions should be sorted by confidence descending");
        }
    }

    #[test]
    fn test_deduplicate_actions_removes_duplicates() {
        let mut actions = vec![
            SuggestedAction { label: "fix port".to_string(), icon: '🔧', action: ActionType::Custom("kill".into()), confidence: 0.9, source: ActionSource::PatternMatch, reason: "".into(), group: ActionGroup::FixActions, shortcut_hint: None },
            SuggestedAction { label: "fix port".to_string(), icon: '🔧', action: ActionType::Custom("kill".into()), confidence: 0.8, source: ActionSource::LlmGenerated, reason: "".into(), group: ActionGroup::FixActions, shortcut_hint: None },
            SuggestedAction { label: "open file".to_string(), icon: '📄', action: ActionType::Edit, confidence: 0.7, source: ActionSource::PatternMatch, reason: "".into(), group: ActionGroup::FileOperations, shortcut_hint: None },
        ];
        ContextActionGenerator::deduplicate_actions(&mut actions);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().filter(|a| &a.label == "fix port").count() == 1);
    }

    #[test]
    fn test_empty_input_returns_empty_context() {
        let r#gen = ContextActionGenerator::new(ContextActionConfig::default());
        let ctx = r#gen.analyze("");
        assert!(ctx.file_paths.is_empty());
        assert!(ctx.urls.is_empty());
        assert!(ctx.errors.is_empty());
        assert!(ctx.code_symbols.is_empty());
    }

    #[test]
    fn test_no_matching_input_returns_no_actions() {
        let r#gen = ContextActionGenerator::new(ContextActionConfig::default());
        let ctx = r#gen.analyze("hello world, just plain text with no special patterns");
        let block = CommandBlock::new(BlockType::UserInput, "plain");
        let actions = r#gen.suggest_actions(&ctx, &block);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_config_max_suggestions_limits_output() {
        let cfg = ContextActionConfig { max_suggestions: 2, min_confidence: 0.0, ..Default::default() };
        let r#gen = ContextActionGenerator::new(cfg);
        let ctx = r#gen.analyze("edit /tmp/a.rs, /tmp/b.rs, /tmp/c.rs, /tmp/d.rs, /tmp/e.rs");
        let block = CommandBlock::new(BlockType::UserInput, "multi");
        let actions = r#gen.suggest_actions(&ctx, &block);
        assert!(actions.len() <= 2);
    }

    #[test]
    fn test_lru_set_basic_operations() {
        let mut lru = LruSet::new(3);
        lru.touch("a".to_string());
        lru.touch("b".to_string());
        lru.touch("c".to_string());
        assert!(lru.contains(&"c".to_string()));
        lru.touch("d".to_string());
        assert!(!lru.contains(&"a".to_string()));
        assert!(lru.contains(&"d".to_string()));
    }

    #[test]
    fn test_recognize_destructive_rm_rf() {
        let cmds = ContextActionGenerator::recognize_commands("rm -rf /important/data");
        assert!(cmds.iter().any(|c| c.risk_level == RiskLevel::Destructive && c.category == CommandCategory::FileOperation));
    }

    #[test]
    fn test_full_analyze_pipeline_integration() {
        let r#gen = ContextActionGenerator::new(ContextActionConfig::default());
        let input = r#"
Error: EADDRINUSE port 8080 already in use.
Check src/main.rs and https://docs.example.com/api/v1/endpoints.
Run git checkout feature/auth and install lodash from npm.
"#;
        let ctx = r#gen.analyze(input);
        assert!(!ctx.errors.is_empty(), "should detect port error");
        assert!(!ctx.urls.is_empty(), "should extract URL");
        assert!(!ctx.git_refs.is_empty(), "should find git ref");
        assert!(!ctx.package_refs.is_empty(), "should find npm package");
        assert!(!ctx.file_paths.is_empty(), "should find file path");

        let block = CommandBlock::new(BlockType::UserInput, "integration");
        let actions = r#gen.suggest_actions(&ctx, &block);
        assert!(!actions.is_empty(), "should generate suggestions");
    }
}
