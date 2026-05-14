use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

/// 单个检查结果
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub phase: &'static str,
    pub passed: bool,
    pub issues: Vec<crate::Issue>,
}

/// Phase 1: AST 结构检查
#[async_trait]
pub trait AstCheck: Send + Sync {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>>;
}

/// Phase 2: 类型检查
#[async_trait]
pub trait TypeCheck: Send + Sync {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>>;
}

/// Phase 3: AI 逻辑校验
#[async_trait]
pub trait AiLogicCheck: Send + Sync {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>>;
}

// ── 共享文件扫描基础设施 ──

/// 增量检查缓存 — 记录每个文件的最后检查时间戳
#[derive(Debug, Default)]
pub struct IncrementalCache {
    timestamps: HashMap<String, SystemTime>,
}

impl IncrementalCache {
    pub fn new() -> Self { Self::default() }

    pub fn needs_check(&mut self, path: &Path) -> bool {
        let metadata = match path.metadata() {
            Ok(m) => m,
            Err(_) => return true,
        };
        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return true,
        };
        let path_str = path.to_string_lossy().to_string();
        let last_check = self.timestamps.get(&path_str);
        if let Some(last) = last_check {
            if modified <= *last {
                return false;
            }
        }
        self.timestamps.insert(path_str, modified);
        true
    }

    pub fn clear(&mut self) {
        self.timestamps.clear();
    }
}

/// 可检查的文件扩展名（单一定义源，所有 phase 共用）
const CHECKABLE_EXTS: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "py", "go"];

fn is_checkable_ext(ext: &str) -> bool {
    CHECKABLE_EXTS.contains(&ext)
}

/// 应跳过的目录
fn is_skippable_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "dist" | "build")
}

/// 收集要检查的文件列表
fn collect_checkable_files(root: &str, cache: &mut Option<&mut IncrementalCache>) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let dir = match std::fs::read_dir(root) {
        Ok(d) => d,
        Err(_) => return files,
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if is_skippable_dir(dir_name) {
                continue;
            }
            let sub_root = path.to_string_lossy().to_string();
            files.extend(collect_checkable_files(&sub_root, cache));
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if !is_checkable_ext(ext) {
                continue;
            }
            if let Some(ref mut c) = cache {
                if !c.needs_check(&path) {
                    continue;
                }
            }
            files.push(path);
        }
    }
    files
}

/// 扫描文件并对每个文件应用检查闭包（消除三个 phase 中重复的遍历逻辑）
fn scan_files<F>(root: &str, cache: &mut Option<&mut IncrementalCache>, phase: &str, mut check_file: F) -> Vec<crate::Issue>
where
    F: FnMut(&Path, &str) -> Vec<crate::Issue>,
{
    let mut issues = Vec::new();
    let mut files_checked = 0u32;
    for path in collect_checkable_files(root, cache) {
        files_checked += 1;
        if let Ok(content) = std::fs::read_to_string(&path) {
            issues.extend(check_file(&path, &content));
        }
    }
    if issues.is_empty() {
        issues.push(crate::Issue::info(phase,
            &format!("{} check passed: {} files checked", phase, files_checked)));
    }
    issues
}

// ── 默认实现 ──

/// 增强型 AST 结构检查器
pub struct RegexAstCheck {
    cache: std::sync::Mutex<IncrementalCache>,
}

impl Default for RegexAstCheck {
    fn default() -> Self { Self { cache: std::sync::Mutex::new(IncrementalCache::new()) } }
}

impl RegexAstCheck {
    pub fn new() -> Self { Self::default() }

    fn check_file_ast(path: &Path, content: &str) -> Vec<crate::Issue> {
        let mut issues = Vec::new();
        let path_str = path.display().to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        // 括号/花括号/方括号平衡
        for (open_c, close_c, name) in [('{', '}', "braces"), ('(', ')', "parentheses"), ('[', ']', "brackets")] {
            let open = content.matches(open_c).count();
            let close = content.matches(close_c).count();
            if open != close {
                issues.push(crate::Issue::error_with_fix(
                    "ast",
                    &format!("{}: unmatched {} (open={}, close={})", path_str, name, open, close),
                    &format!("Check for missing/extra {} or {} in the file", open_c, close_c),
                ));
            }
        }

        // 尾随空格
        for (line_no, line) in content.lines().enumerate() {
            if line.len() > line.trim_end().len() && line.ends_with(' ') {
                issues.push(crate::Issue::warning_with_fix(
                    "ast",
                    &format!("{}:{} trailing whitespace", path_str, line_no + 1),
                    "Remove trailing whitespace (auto-fix available)",
                ));
                break;
            }
        }

        // Tab 字符
        if content.contains('\t') {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: file contains tab characters", path_str),
                "Use spaces instead of tabs (auto-fix available)",
            ));
        }

        // 文件末尾换行
        if !content.ends_with('\n') {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: missing trailing newline", path_str),
                "Add a newline at the end of the file (auto-fix available)",
            ));
        }

        // Rust: println! → tracing
        if ext == "rs" && content.contains("println!") {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: uses println! — consider using tracing::info! instead", path_str),
                "Replace println! with tracing::info! or similar (auto-fix available)",
            ));
        }

        // TODO/FIXME/XXX 注释
        for (line_no, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if (trimmed.contains("TODO") || trimmed.contains("FIXME") || trimmed.contains("XXX"))
                && (trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*"))
            {
                let tag = if trimmed.contains("FIXME") { "FIXME" } else if trimmed.contains("TODO") { "TODO" } else { "XXX" };
                issues.push(crate::Issue::info("ast", &format!("{}:{} {} found", path_str, line_no + 1, tag)));
            }
        }

        // 命名规范 (Rust: snake_case, JS/TS: camelCase)
        if ext == "rs" {
            let fn_re = regex::Regex::new(r"(?m)^\s*(pub\s+)?(async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
            for cap in fn_re.captures_iter(content) {
                let fn_name = cap.get(3).map(|m| m.as_str()).unwrap_or("");
                if !fn_name.starts_with("test_") && !fn_name.starts_with("should_") && fn_name.contains(|c: char| c.is_uppercase()) {
                    let line_no = content[..cap.get(0).unwrap().start()].lines().count() + 1;
                    issues.push(crate::Issue::warning_with_fix(
                        "ast",
                        &format!("{}:{} function '{}' should use snake_case", path_str, line_no, fn_name),
                        "Rename to snake_case format",
                    ));
                }
            }
        } else if matches!(ext, "ts" | "tsx" | "js" | "jsx") {
            let fn_re = regex::Regex::new(r"(?m)^\s*(export\s+)?(async\s+)?function\s+([a-zA-Z_$][a-zA-Z0-9_$]*)").unwrap();
            for cap in fn_re.captures_iter(content) {
                let fn_name = cap.get(3).map(|m| m.as_str()).unwrap_or("");
                if !fn_name.starts_with('_') && fn_name.to_uppercase() != fn_name && fn_name.contains('_') {
                    let line_no = content[..cap.get(0).unwrap().start()].lines().count() + 1;
                    issues.push(crate::Issue::info(
                        "ast",
                        &format!("{}:{} function '{}' may want camelCase (JS/TS convention)", path_str, line_no, fn_name),
                    ));
                }
            }
        }

        // 魔法数字
        if matches!(ext, "rs" | "ts" | "tsx" | "js" | "jsx") {
            for (line_no, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                    continue;
                }
                let magic_re = regex::Regex::new(r"(?:==|!=|<=|>=|<|>|=)\s*(\d{3,})").unwrap();
                for cap in magic_re.captures_iter(line) {
                    let num_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                    let num: i64 = num_str.parse().unwrap_or(0);
                    if num > 10 && !matches!(num, 100 | 200 | 201 | 204 | 300 | 301 | 302 | 400 | 401 | 403 | 404 | 500 | 502 | 503) {
                        issues.push(crate::Issue::info(
                            "ast",
                            &format!("{}:{} magic number {} — consider using a named constant", path_str, line_no + 1, num_str),
                        ));
                        break;
                    }
                }
            }
        }

        issues
    }
}

#[async_trait]
impl AstCheck for RegexAstCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        Ok(scan_files(root, &mut Some(&mut cache), "ast", Self::check_file_ast))
    }
}

/// 基于 git diff 的增量检查
pub struct GitDiffAstCheck;

impl GitDiffAstCheck {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl AstCheck for GitDiffAstCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let output = tokio::process::Command::new("git")
            .args(["diff", "--cached", "--name-only", "--diff-filter=ACM"])
            .current_dir(root)
            .output()
            .await;

        let files_str = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => return Ok(vec![crate::Issue::info("ast", "No staged changes or not a git repo; skipping git-diff AST check")]),
        };

        let mut issues = Vec::new();
        let mut files_checked = 0u32;
        for line in files_str.lines() {
            let path = Path::new(root).join(line);
            if !path.exists() { continue; }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if is_checkable_ext(ext) {
                    files_checked += 1;
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        issues.extend(RegexAstCheck::check_file_ast(&path, &content));
                    }
                }
            }
        }

        if issues.is_empty() {
            issues.push(crate::Issue::info("ast",
                &format!("Git-diff AST check passed: {} staged files checked", files_checked)));
        }
        Ok(issues)
    }
}

/// 基于 cargo check / tsc 的类型检查器
pub struct CargoTypeCheck {
    last_check_result: std::sync::Mutex<Option<(SystemTime, Vec<crate::Issue>)>>,
}

impl Default for CargoTypeCheck {
    fn default() -> Self { Self { last_check_result: std::sync::Mutex::new(None) } }
}

impl CargoTypeCheck {
    pub fn new() -> Self { Self::default() }

    fn parse_cargo_line(line: &str) -> Option<crate::Issue> {
        if line.contains("error[") {
            Some(crate::Issue::error_with_fix("type", line, "Fix the reported error"))
        } else if line.contains("warning[") {
            let fix = if line.contains("unused") {
                "Remove unused code or prefix with _"
            } else if line.contains("dead_code") {
                "Remove dead code or add #[allow(dead_code)]"
            } else if line.contains("unreachable") {
                "Remove unreachable code or restructure logic"
            } else {
                "Review and fix the warning"
            };
            Some(crate::Issue::warning_with_fix("type", line, fix))
        } else {
            let lower = line.to_lowercase();
            if lower.contains("error") && !lower.contains("error[") && !lower.starts_with(' ') {
                Some(crate::Issue::error("type", line))
            } else {
                None
            }
        }
    }
}

#[async_trait]
impl TypeCheck for CargoTypeCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let mut issues = Vec::new();

        match tokio::process::Command::new("cargo")
            .args(["check", "--message-format=short"])
            .current_dir(root)
            .output()
            .await
        {
            Ok(o) => {
                for line in String::from_utf8_lossy(&o.stderr).lines() {
                    if let Some(issue) = Self::parse_cargo_line(line) {
                        issues.push(issue);
                    }
                }
                if let Ok(mut cache) = self.last_check_result.lock() {
                    *cache = Some((SystemTime::now(), issues.clone()));
                }
            }
            Err(_) => {
                issues.extend(Self::try_typescript_check(root).await);
            }
        }

        if issues.is_empty() {
            issues.push(crate::Issue::info("type", "Type check passed"));
        }
        Ok(issues)
    }
}

impl CargoTypeCheck {
    async fn try_typescript_check(root: &str) -> Vec<crate::Issue> {
        for cmd in &[&["npx", "tsc", "--noEmit"][..], &["npx", "tsc", "--noEmit", "--project", "tsconfig.json"][..]] {
            if let Ok(o) = tokio::process::Command::new(cmd[0]).args(&cmd[1..]).current_dir(root).output().await {
                let issues: Vec<crate::Issue> = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|line| line.contains("error TS"))
                    .map(|line| crate::Issue::error("type", line))
                    .collect();
                if !issues.is_empty() {
                    return issues;
                }
            }
        }
        vec![crate::Issue::info("type", "TypeScript type check passed or unavailable")]
    }
}

/// 基于规则的 AI 逻辑校验器
pub struct RuleBasedAiCheck {
    cache: std::sync::Mutex<IncrementalCache>,
}

impl Default for RuleBasedAiCheck {
    fn default() -> Self { Self { cache: std::sync::Mutex::new(IncrementalCache::new()) } }
}

impl RuleBasedAiCheck {
    pub fn new() -> Self { Self::default() }

    fn patterns_to_check() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
        vec![
            ("unwrap_risk", r"\.unwrap\(\)", "error",
             "Using unwrap() — replace with ? or match or .context()"),
            ("panic_risk", r"panic!\(", "error",
             "Using panic!() — replace with proper error handling"),
            ("todo_todo", r"todo!\(", "warning",
             "Incomplete implementation — replace todo! with real logic"),
            ("hardcoded_secret", r#"(password|secret|token|api_key|api-key)\s*[:=]\s*['"][^'"]+['"]"#,
             "error", "Possible hardcoded secret — use env variable instead"),
            ("unsafe_code", r"unsafe\s*\{", "warning",
             "Unsafe block — verify memory safety"),
            ("unwrapped_expect", r"\.expect\([^)]+\)", "warning",
             "Using expect() — consider propagating the error with ?"),
            ("dbg_macro", r"dbg!\(", "warning",
             "Using dbg!() — remove before committing"),
            ("eval_danger", r"\beval\s*\(", "error",
             "Using eval() — security risk, find a safer alternative"),
            ("injection_risk", r"\.inner_html\s*=", "warning",
             "Setting inner_html — potential XSS risk, use safe APIs"),
            ("insecure_compare", r#"(==|!=)\s*['\"][^'\"]{10,}['\"]"#, "info",
             "Possible hardcoded comparison string — consider environment variable"),
            ("excessive_complexity", r"if\s*\([^)]{80,}\)", "warning",
             "Overly complex condition — consider extracting into a named variable"),
        ]
    }
}

#[async_trait]
impl AiLogicCheck for RuleBasedAiCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let patterns = Self::patterns_to_check();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        // collect_checkable_files 已经按 is_checkable_ext 过滤，无需二次检查
        Ok(scan_files(root, &mut Some(&mut cache), "ai", |path, content| {
            let mut issues = Vec::new();
            for (id, pattern, severity, msg) in &patterns {
                if let Ok(re) = regex::Regex::new(pattern) {
                    for m in re.find_iter(content) {
                        let line = content[..m.start()].lines().count();
                        issues.push(if *severity == "error" {
                            crate::Issue::error_with_fix("ai",
                                &format!("{}:{} [{}] {}", path.display(), line + 1, id, msg),
                                &format!("Replace {} pattern at line {}", id, line + 1))
                        } else {
                            crate::Issue::warning_with_fix("ai",
                                &format!("{}:{} [{}] {}", path.display(), line + 1, id, msg),
                                &format!("Review {} pattern at line {}", id, line + 1))
                        });
                    }
                }
            }
            issues
        }))
    }
}
