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

// ── 增量检查追踪器 ──

/// 增量检查缓存 — 记录每个文件的最后检查时间戳
#[derive(Debug, Default)]
pub struct IncrementalCache {
    timestamps: HashMap<String, SystemTime>,
}

impl IncrementalCache {
    pub fn new() -> Self { Self::default() }

    /// 检查文件是否需要被重新检查
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
                return false; // 文件未修改，跳过
            }
        }
        self.timestamps.insert(path_str, modified);
        true
    }

    /// 清除缓存（强制全量检查）
    pub fn clear(&mut self) {
        self.timestamps.clear();
    }
}

/// 可检查的文件扩展名
fn is_checkable_ext(ext: &str) -> bool {
    matches!(ext, "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go")
}

/// 收集要检查的文件列表
fn collect_checkable_files(root: &str, cache: &mut Option<&mut IncrementalCache>) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() { continue; }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if is_checkable_ext(ext) {
                    if let Some(ref mut c) = cache {
                        if !c.needs_check(&path) {
                            continue; // 增量跳过
                        }
                    }
                    files.push(path);
                }
            }
        }
    }
    files
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

    /// 检查代码中的各种结构问题
    fn check_file_ast(path: &Path, content: &str) -> Vec<crate::Issue> {
        let mut issues = Vec::new();
        let path_str = path.display().to_string();

        // 1. 检查大括号平衡
        let open_braces = content.matches('{').count();
        let close_braces = content.matches('}').count();
        if open_braces != close_braces {
            issues.push(crate::Issue::error_with_fix(
                "ast",
                &format!("{}: unmatched braces (open={}, close={})", path_str, open_braces, close_braces),
                "Check for missing/extra { or } in the file",
            ));
        }

        // 2. 检查圆括号平衡
        let open_parens = content.matches('(').count();
        let close_parens = content.matches(')').count();
        if open_parens != close_parens {
            issues.push(crate::Issue::error_with_fix(
                "ast",
                &format!("{}: unmatched parentheses (open={}, close={})", path_str, open_parens, close_parens),
                "Check for missing/extra ( or ) in the file",
            ));
        }

        // 3. 检查方括号平衡
        let open_brackets = content.matches('[').count();
        let close_brackets = content.matches(']').count();
        if open_brackets != close_brackets {
            issues.push(crate::Issue::error_with_fix(
                "ast",
                &format!("{}: unmatched brackets (open={}, close={})", path_str, open_brackets, close_brackets),
                "Check for missing/extra [ or ] in the file",
            ));
        }

        // 4. 检查尾随空格
        for (line_no, line) in content.lines().enumerate() {
            if line.len() > line.trim_end().len() && line.ends_with(' ') {
                issues.push(crate::Issue::warning_with_fix(
                    "ast",
                    &format!("{}:{} trailing whitespace", path_str, line_no + 1),
                    "Remove trailing whitespace",
                ));
                break; // 每个文件只报告一次
            }
        }

        // 5. 检查 Tab 字符（对于非 Makefile 文件）
        if content.contains('\t') {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: file contains tab characters", path_str),
                "Use spaces instead of tabs (e.g. `rustfmt` or `prettier`)",
            ));
        }

        // 6. 检查文件末尾换行
        if !content.ends_with('\n') {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: missing trailing newline", path_str),
                "Add a newline at the end of the file",
            ));
        }

        // 7. Rust-specific: 检查 println! 是否该换成 logging
        if path.extension().map(|e| e == "rs").unwrap_or(false) {
            if content.contains("println!") {
                issues.push(crate::Issue::warning_with_fix(
                    "ast",
                    &format!("{}: uses println! — consider using tracing::info! instead", path_str),
                    "Replace println! with tracing::info! or similar",
                ));
            }
        }

        issues
    }
}

#[async_trait]
impl AstCheck for RegexAstCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let mut issues = Vec::new();
        let mut files_checked = 0u32;
        let mut cache = self.cache.lock().unwrap();

        let files = collect_checkable_files(root, &mut Some(&mut cache));
        for path in &files {
            files_checked += 1;
            if let Ok(content) = std::fs::read_to_string(path) {
                issues.extend(Self::check_file_ast(path, &content));
            }
        }

        if issues.is_empty() {
            issues.push(crate::Issue::info("ast",
                &format!("AST check passed: {} files checked", files_checked)));
        }
        Ok(issues)
    }
}

/// 基于 git diff 的增量检查（仅检查变更文件）
pub struct GitDiffAstCheck;

impl GitDiffAstCheck {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl AstCheck for GitDiffAstCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let mut issues = Vec::new();

        // 使用 git diff --name-only 获取变更文件
        let output = tokio::process::Command::new("git")
            .args(["diff", "--cached", "--name-only", "--diff-filter=ACM"])
            .current_dir(root)
            .output()
            .await;

        let files_str = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => return Ok(vec![crate::Issue::info("ast", "No staged changes or not a git repo; skipping git-diff AST check")]),
        };

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

/// 基于 cargo check 的类型检查器（带输出缓存）
pub struct CargoTypeCheck {
    last_check_result: std::sync::Mutex<Option<(SystemTime, Vec<crate::Issue>)>>,
}

impl Default for CargoTypeCheck {
    fn default() -> Self { Self { last_check_result: std::sync::Mutex::new(None) } }
}

impl CargoTypeCheck {
    pub fn new() -> Self { Self::default() }

    /// 解析 cargo check 输出行
    fn parse_cargo_line(line: &str) -> Option<crate::Issue> {
        if line.contains("error[") {
            Some(crate::Issue::error_with_fix("type", line, "Fix the reported error"))
        } else if line.contains("warning[") {
            let fix_suggestion = if line.contains("unused") {
                "Remove unused code or prefix with _"
            } else if line.contains("dead_code") {
                "Remove dead code or add #[allow(dead_code)]"
            } else if line.contains("unreachable") {
                "Remove unreachable code or restructure logic"
            } else {
                "Review and fix the warning"
            };
            Some(crate::Issue::warning_with_fix("type", line, fix_suggestion))
        } else {
            None
        }
    }
}

#[async_trait]
impl TypeCheck for CargoTypeCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let mut issues = Vec::new();

        let output = tokio::process::Command::new("cargo")
            .args(["check", "--message-format=short"])
            .current_dir(root)
            .output()
            .await;

        match output {
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                for line in stderr.lines() {
                    if let Some(issue) = Self::parse_cargo_line(line) {
                        issues.push(issue);
                    }
                }
                // 缓存检查结果
                if let Ok(mut cache) = self.last_check_result.lock() {
                    *cache = Some((SystemTime::now(), issues.clone()));
                }
            }
            Err(e) => {
                // 如果 cargo 不可用（非 Rust 项目），尝试使用 tsc
                if let Ok(tsc_output) = tokio::process::Command::new("npx")
                    .args(["tsc", "--noEmit"])
                    .current_dir(root)
                    .output()
                    .await
                {
                    let stdout = String::from_utf8_lossy(&tsc_output.stdout);
                    for line in stdout.lines() {
                        if line.contains("error TS") {
                            issues.push(crate::Issue::error("type", line));
                        }
                    }
                } else {
                    issues.push(crate::Issue::info("type", &format!("No type checker available: {}", e)));
                }
            }
        }

        if issues.is_empty() {
            issues.push(crate::Issue::info("type", "Type check passed"));
        }
        Ok(issues)
    }
}

/// 基于规则的 AI 逻辑校验器（增强版）
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
            ("println_in_prod", r"^\s*println!", "warning",
             "Using println! in production code — use a logger (unless in test code)"),
        ]
    }
}

#[async_trait]
impl AiLogicCheck for RuleBasedAiCheck {
    async fn check(&self, root: &str) -> anyhow::Result<Vec<crate::Issue>> {
        let mut issues = Vec::new();
        let patterns = Self::patterns_to_check();
        let mut cache = self.cache.lock().unwrap();

        let files = collect_checkable_files(root, &mut Some(&mut cache));
        for path in &files {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "rs" | "ts" | "tsx") {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        for (id, pattern, severity, msg) in &patterns {
                            if let Ok(re) = regex::Regex::new(pattern) {
                                for cap in re.find_iter(&content) {
                                    let line = content[..cap.start()].lines().count();
                                    let issue = if *severity == "error" {
                                        crate::Issue::error_with_fix(
                                            "ai",
                                            &format!("{}:{} [{}] {}", path.display(), line + 1, id, msg),
                                            &format!("Replace {} pattern at line {}", id, line + 1),
                                        )
                                    } else {
                                        crate::Issue::warning_with_fix(
                                            "ai",
                                            &format!("{}:{} [{}] {}", path.display(), line + 1, id, msg),
                                            &format!("Review {} pattern at line {}", id, line + 1),
                                        )
                                    };
                                    issues.push(issue);
                                }
                            }
                        }
                    }
                }
            }
        }

        if issues.is_empty() {
            issues.push(crate::Issue::info("ai", "AI logic check passed"));
        }
        Ok(issues)
    }
}
