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
            if path.is_dir() {
                // 跳过隐藏目录和常见构建目录
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if dir_name.starts_with('.') || matches!(dir_name, "node_modules" | "target" | "dist" | "build" | ".git") {
                    continue;
                }
                // 递归检查子目录
                let sub_root = path.to_string_lossy().to_string();
                files.extend(collect_checkable_files(&sub_root, cache));
                continue;
            }
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
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

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
                    "Remove trailing whitespace (auto-fix available)",
                ));
                break; // 每个文件只报告一次
            }
        }

        // 5. 检查 Tab 字符（对于非 Makefile 文件）
        if content.contains('\t') {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: file contains tab characters", path_str),
                "Use spaces instead of tabs (auto-fix available)",
            ));
        }

        // 6. 检查文件末尾换行
        if !content.ends_with('\n') {
            issues.push(crate::Issue::warning_with_fix(
                "ast",
                &format!("{}: missing trailing newline", path_str),
                "Add a newline at the end of the file (auto-fix available)",
            ));
        }

        // 7. Rust-specific: 检查 println! 是否该换成 logging
        if ext == "rs" {
            if content.contains("println!") {
                issues.push(crate::Issue::warning_with_fix(
                    "ast",
                    &format!("{}: uses println! — consider using tracing::info! instead", path_str),
                    "Replace println! with tracing::info! or similar (auto-fix available)",
                ));
            }
        }

        // 8. 检查 TODO/FIXME/XXX 注释
        for (line_no, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.contains("TODO") || trimmed.contains("FIXME") || trimmed.contains("XXX") {
                if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
                    issues.push(crate::Issue::info(
                        "ast",
                        &format!("{}:{} {} found", path_str, line_no + 1,
                            if trimmed.contains("FIXME") { "FIXME" }
                            else if trimmed.contains("TODO") { "TODO" }
                            else { "XXX" }),
                    ));
                }
            }
        }

        // 9. 检查命名规范 (Rust: snake_case 函数, TypeScript/JS: camelCase 函数)
        if ext == "rs" {
            // Rust: 检查函数定义是否是 snake_case
            let fn_re = regex::Regex::new(r"(?m)^\s*(pub\s+)?(async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
            for cap in fn_re.captures_iter(content) {
                let fn_name = cap.get(3).map(|m| m.as_str()).unwrap_or("");
                if !fn_name.starts_with("test_") && !fn_name.starts_with("should_") {
                    if fn_name.contains(|c: char| c.is_uppercase()) {
                        // Find the line number
                        let line_no = content[..cap.get(0).unwrap().start()].lines().count() + 1;
                        issues.push(crate::Issue::warning_with_fix(
                            "ast",
                            &format!("{}:{} function '{}' should use snake_case", path_str, line_no, fn_name),
                            "Rename to snake_case format",
                        ));
                    }
                }
            }
        } else if matches!(ext, "ts" | "tsx" | "js" | "jsx") {
            // TS/JS: 检查 function 定义是否是 camelCase
            let fn_re = regex::Regex::new(r"(?m)^\s*(export\s+)?(async\s+)?function\s+([a-zA-Z_$][a-zA-Z0-9_$]*)").unwrap();
            for cap in fn_re.captures_iter(content) {
                let fn_name = cap.get(3).map(|m| m.as_str()).unwrap_or("");
                if fn_name.starts_with('_') || fn_name.to_uppercase() == fn_name {
                    continue; // 跳过下划线前缀和全大写
                }
                if fn_name.contains('_') && !fn_name.starts_with('_') {
                    let line_no = content[..cap.get(0).unwrap().start()].lines().count() + 1;
                    issues.push(crate::Issue::info(
                        "ast",
                        &format!("{}:{} function '{}' may want camelCase (JS/TS convention)", path_str, line_no, fn_name),
                    ));
                }
            }
        }

        // 10. 魔法数字检查 (Rust + TS/JS)
        if matches!(ext, "rs" | "ts" | "tsx" | "js" | "jsx") {
            // 检查裸数字（不是赋值给常量、不是条件判断中的简单数字）
            // 简化的魔法数字检测，匹配 == number, = number (非 0/1)
            for (line_no, line) in content.lines().enumerate() {
                // 跳过注释行
                let trimmed = line.trim();
                if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                    continue;
                }
                // 检查 == 42, > 42 等
                let magic_re = regex::Regex::new(r"(?:==|!=|<=|>=|<|>|=)\s*(\d{3,})").unwrap();
                for cap in magic_re.captures_iter(line) {
                    let num_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                    let num: i64 = num_str.parse().unwrap_or(0);
                    // 排除常见的 100, 200, 300, 404, 500 等状态码
                    if num > 10 && !matches!(num, 100 | 200 | 201 | 204 | 300 | 301 | 302 | 400 | 401 | 403 | 404 | 500 | 502 | 503) {
                        issues.push(crate::Issue::info(
                            "ast",
                            &format!("{}:{} magic number {} — consider using a named constant", path_str, line_no + 1, num_str),
                        ));
                        break; // 每行只报告一次
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
            // 也捕获不含方括号的错误/警告
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

        // 尝试 Rust cargo check
        let cargo_result = tokio::process::Command::new("cargo")
            .args(["check", "--message-format=short"])
            .current_dir(root)
            .output()
            .await;

        match cargo_result {
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
            Err(_) => {
                // 如果 cargo 不可用（非 Rust 项目），尝试使用 tsc
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
    /// 尝试 TypeScript 类型检查 (tsc --noEmit)
    async fn try_typescript_check(root: &str) -> Vec<crate::Issue> {
        let mut issues = Vec::new();

        for cmd in &[&["npx", "tsc", "--noEmit"][..], &["npx", "tsc", "--noEmit", "--project", "tsconfig.json"][..]] {
            if let Ok(o) = tokio::process::Command::new(cmd[0])
                .args(&cmd[1..])
                .current_dir(root)
                .output()
                .await
            {
                let stdout = String::from_utf8_lossy(&o.stdout);
                for line in stdout.lines() {
                    if line.contains("error TS") {
                        issues.push(crate::Issue::error("type", line));
                    }
                }
                if !issues.is_empty() {
                    break;
                }
            }
        }

        if issues.is_empty() {
            issues.push(crate::Issue::info("type", "TypeScript type check passed or unavailable"));
        }
        issues
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
            // 新增检查模式
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
        let mut issues = Vec::new();
        let patterns = Self::patterns_to_check();
        let mut cache = self.cache.lock().unwrap();

        let files = collect_checkable_files(root, &mut Some(&mut cache));
        for path in &files {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go") {
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
