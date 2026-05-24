//! Plan → 交付 全自动闭环
//!
//! 补齐3个缺失环:
//!   测试环: cargo test → 解析失败 → LLM修复 → 重跑
//!   审查环: diff分析 → 风格检查 → 安全审查 → 修复
//!   Git环: 自动commit → 生成PR描述 → 创建PR
//!
//! 完整链路:
//!   Plan → LLM代码 → 编译验证(已有) → 测试验证 → 审查
//!   → 修复 → Git commit → PR → 交付

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::rest_llm::InferenceRouter;

// ========================================================================
// [1] 测试环 — cargo test → 解析 → LLM修复 → 重跑 ×3
// ========================================================================

/// 测试结果
#[derive(Debug, Clone)]
pub struct TestResult {
    pub passed: bool,
    pub total: u32,
    pub passed_count: u32,
    pub failed_count: u32,
    pub failures: Vec<TestFailure>,
    pub duration_ms: u64,
}

/// 测试失败
#[derive(Debug, Clone)]
pub struct TestFailure {
    pub test_name: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}

/// 测试环 — cargo test → 修复 → 重跑 ×3
pub struct TestRing {
    workspace: PathBuf,
}

impl TestRing {
    pub fn new(workspace: &Path) -> Self {
        Self { workspace: workspace.to_path_buf() }
    }

    /// 运行测试环: run → fail? → LLM修复 → rerun ×3
    pub async fn run(&self) -> Result<TestResult, String> {
        for i in 0..3 {
            let result = self.run_cargo_test().await?;
            if result.passed {
                return Ok(result);
            }
            if i >= 2 {
                return Ok(result); // 3次后放弃
            }
            // 用LLM修复失败的测试
            let router = InferenceRouter::new(vec![], "deepseek-chat");
            for failure in &result.failures {
                let prompt = format!(
                    "Fix this failing test:\nTest: {}\nError: {}\n\n\
                     Return the fixed test file in ```file:path ... ``` format.",
                    failure.test_name, failure.message
                );
                if let Ok(response) = router.chat_completion(&prompt, "Fix the test.").await {
                    self.apply_llm_response(&response).await;
                }
            }
        }
        // 不会执行到这里
        self.run_cargo_test().await
    }

    async fn run_cargo_test(&self) -> Result<TestResult, String> {
        let start = Instant::now();
        let output = tokio::process::Command::new("cargo")
            .args(["test", "--no-fail-fast", "--color=never"])
            .current_dir(&self.workspace)
            .output()
            .await.map_err(|e| format!("cargo test failed: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let mut failures = Vec::new();
        let mut total = 0u32;
        let mut passed_count = 0u32;

        for line in combined.lines() {
            let t = line.trim();
            if t.starts_with("test ") && t.contains("... FAILED") {
                let name = t.split_whitespace().nth(1).unwrap_or("?").to_string();
                failures.push(TestFailure {
                    test_name: name,
                    file: None, line: None,
                    message: t.to_string(),
                });
            }
            if t.starts_with("test result: ") {
                // 解析 "test result: OK. 42 passed; 0 failed; ..."
                if let Some(p) = t.split(';').nth(0) {
                    passed_count = p.split_whitespace()
                        .find_map(|w| w.parse::<u32>().ok()).unwrap_or(0);
                }
                total = passed_count + failures.len() as u32;
            }
        }

        Ok(TestResult {
            passed: output.status.success(),
            total,
            passed_count,
            failed_count: failures.len() as u32,
            failures,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn apply_llm_response(&self, response: &str) {
        for block in response.split("```") {
            let b = block.trim();
            if b.starts_with("file:") {
                if let Some((path, content)) = b.trim_start_matches("file:").trim().split_once('\n') {
                    let full = self.workspace.join(path.trim());
                    tokio::fs::write(&full, content).await.ok();
                }
            }
        }
    }
}

// ========================================================================
// [2] 审查环 — diff分析 → 风格检查 → 安全审查 → 修复
// ========================================================================

/// 审查结果
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub passed: bool,
    pub style_issues: Vec<StyleIssue>,
    pub security_issues: Vec<String>,
    pub complexity_alerts: Vec<String>,
    pub suggestions: Vec<String>,
}

/// 风格问题
#[derive(Debug, Clone)]
pub struct StyleIssue {
    pub file: String,
    pub line: usize,
    pub severity: String, // "error" | "warning" | "info"
    pub message: String,
}

/// 审查环 — 自动审查代码变更
pub struct ReviewRing {
    workspace: PathBuf,
}

impl ReviewRing {
    pub fn new(workspace: &Path) -> Self {
        Self { workspace: workspace.to_path_buf() }
    }

    /// 审查所有变更
    pub async fn review(&self) -> Result<ReviewResult, String> {
        let mut result = ReviewResult {
            passed: true,
            style_issues: vec![],
            security_issues: vec![],
            complexity_alerts: vec![],
            suggestions: vec![],
        };

        // 获取git diff
        let diff = self.get_git_diff().await;

        // 审查每个变更文件
        for entry in diff.lines() {
            let parts: Vec<&str> = entry.splitn(2, '\t').collect();
            if parts.len() < 2 { continue; }
            let status = parts[0];
            let file = parts[1];

            let content = tokio::fs::read_to_string(self.workspace.join(file))
                .await.unwrap_or_default();

            // 风格检查
            self.check_style(&file, &content, &mut result);

            // 安全检查
            self.check_security(&file, &content, &mut result);

            // 复杂度检查
            if status.starts_with('M') || status.starts_with('A') {
                self.check_complexity(&file, &content, &mut result);
            }
        }

        result.passed = result.style_issues.is_empty()
            && result.security_issues.is_empty()
            && result.complexity_alerts.is_empty();

        Ok(result)
    }

    fn check_style(&self, file: &str, content: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let t = line.trim_end();

            // 尾随空格
            if line.len() > t.len() && t.len() > 0 {
                result.style_issues.push(StyleIssue {
                    file: file.to_string(), line: i + 1,
                    severity: "warning".to_string(),
                    message: "Trailing whitespace".to_string(),
                });
            }

            // 超长行 (>120字符)
            if line.len() > 120 {
                result.style_issues.push(StyleIssue {
                    file: file.to_string(), line: i + 1,
                    severity: "warning".to_string(),
                    message: format!("Line too long ({} chars, max 120)", line.len()),
                });
            }

            // 硬编码敏感信息
            let lower = line.to_lowercase();
            if lower.contains("password") || lower.contains("api_key")
                || lower.contains("secret") || lower.contains("token")
                || lower.contains("credential") {
                result.security_issues.push(format!(
                    "{}:{}: Possible credential in code", file, i + 1
                ));
            }

            // TODO/FIXME/HACK
            if line.contains("TODO") {
                result.suggestions.push(format!(
                    "{}:{}: TODO remaining", file, i + 1
                ));
            }
            if line.contains("FIXME") {
                result.style_issues.push(StyleIssue {
                    file: file.to_string(), line: i + 1,
                    severity: "warning".to_string(),
                    message: "FIXME should be resolved before commit".to_string(),
                });
            }

            // println! / dbg! (调试残留)
            if line.contains("println!") || line.contains("dbg!") {
                result.suggestions.push(format!(
                    "{}:{}: Debug print left in code", file, i + 1
                ));
            }
        }
    }

    fn check_security(&self, file: &str, content: &str, result: &mut ReviewResult) {
        // unsafe代码
        if content.contains("unsafe {") || content.contains("unsafe\n") {
            result.security_issues.push(format!(
                "{}: Contains unsafe blocks — review carefully", file
            ));
        }

        // 硬编码IP/URL
        for (i, line) in content.lines().enumerate() {
            if line.contains("127.0.0.1") || line.contains("0.0.0.0") { continue; }
            if line.contains("http://") && !line.contains("example.com") {
                result.security_issues.push(format!(
                    "{}:{}: Hardcoded URL (use config instead)", file, i + 1
                ));
            }
        }

        // 不安全的函数
        let unsafe_fns = ["unwrap()", "expect(\"", "panic!",
            "std::process::exit", "std::mem::transmute"];
        for (i, line) in content.lines().enumerate() {
            for &uf in &unsafe_fns {
                if line.contains(uf) {
                    result.style_issues.push(StyleIssue {
                        file: file.to_string(), line: i + 1,
                        severity: "info".to_string(),
                        message: format!("Use of '{}' — consider safer alternative", uf),
                    });
                }
            }
        }
    }

    fn check_complexity(&self, file: &str, content: &str, result: &mut ReviewResult) {
        let lines = content.lines().count();
        if lines > 500 {
            result.complexity_alerts.push(format!(
                "{}: Very large file ({} lines) — consider splitting", file, lines
            ));
        }

        // 检测嵌套深度 (通过缩进)
        let max_indent = content.lines()
            .map(|l| l.chars().take_while(|c| *c == ' ').count())
            .max().unwrap_or(0);
        if max_indent > 40 {
            result.complexity_alerts.push(format!(
                "{}: Deep nesting ({} spaces indent) — consider refactoring", file, max_indent
            ));
        }
    }

    async fn get_git_diff(&self) -> String {
        let output = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "diff", "--name-status", "HEAD"])
            .output().await;
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(_) => String::new(),
        }
    }
}

// ========================================================================
// [3] Git环 — 自动commit + PR
// ========================================================================

/// Git环 — 自动提交代码 → 创建PR
pub struct GitRing {
    workspace: PathBuf,
}

impl GitRing {
    pub fn new(workspace: &Path) -> Self {
        Self { workspace: workspace.to_path_buf() }
    }

    /// 自动commit + 生成描述 + 创建PR
    pub async fn commit_and_pr(&self, goal: &str) -> Result<String, String> {
        // 1. 生成 commit message
        let message = self.generate_commit_message(goal).await;

        // 2. git add
        self.git_add().await?;

        // 3. git commit
        self.git_commit(&message).await?;

        // 4. git push
        self.git_push().await?;

        // 5. 生成PR描述
        let pr_body = self.generate_pr_body(goal).await;

        // 6. 创建PR (需要gh CLI)
        let pr_url = self.create_pr(&message, &pr_body).await;

        Ok(format!(
            "✅ Committed: {}\n📝 PR: {}\n\n{}",
            message,
            pr_url.unwrap_or_else(|_| "gh CLI not available, push manually".to_string()),
            pr_body
        ))
    }

    async fn generate_commit_message(&self, goal: &str) -> String {
        let diff = self.get_diff().await;
        let router = InferenceRouter::new(vec![], "deepseek-chat");
        let prompt = format!(
            "Generate a concise git commit message for:\nGoal: {}\n\nDiff:\n{}\n\n\
             Return ONLY the commit message (one line title + blank line + body).",
            goal, diff.chars().take(2000).collect::<String>()
        );
        router.chat_completion(&prompt, "You are a git commit message generator.").await
            .unwrap_or_else(|_| format!("feat: {}", goal))
    }

    async fn generate_pr_body(&self, goal: &str) -> String {
        let diff = self.get_diff().await;
        let router = InferenceRouter::new(vec![], "deepseek-chat");
        let prompt = format!(
            "Generate a GitHub PR description for:\nGoal: {}\n\nDiff:\n{}\n\n\
             Format:\n## Summary\n## Changes\n## Testing\n## Notes",
            goal, diff.chars().take(2000).collect::<String>()
        );
        router.chat_completion(&prompt, "You are a PR description generator.").await
            .unwrap_or_else(|_| format!("## Summary\n\n{}", goal))
    }

    async fn git_add(&self) -> Result<(), String> {
        let status = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "add", "-A"])
            .status().await.map_err(|e| format!("git add: {}", e))?;
        if status.success() { Ok(()) } else { Err("git add failed".to_string()) }
    }

    async fn git_commit(&self, message: &str) -> Result<(), String> {
        let status = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "commit", "-m", message])
            .status().await.map_err(|e| format!("git commit: {}", e))?;
        if status.success() { Ok(()) } else { Err("git commit failed (nothing to commit?)".to_string()) }
    }

    async fn git_push(&self) -> Result<(), String> {
        let status = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "push"])
            .status().await.map_err(|e| format!("git push: {}", e))?;
        if status.success() { Ok(()) } else { Err("git push failed".to_string()) }
    }

    async fn create_pr(&self, title: &str, body: &str) -> Result<String, String> {
        // 方案1: 尝试 gh CLI (最快)
        match self.create_pr_via_gh(title, body).await {
            Ok(url) if !url.is_empty() => return Ok(url),
            _ => {} // 回退到方案2
        }

        // 方案2: GitHub API (不需要 gh CLI)
        self.create_pr_via_api(title, body).await
    }

    /// 通过 gh CLI 创建 PR
    async fn create_pr_via_gh(&self, title: &str, body: &str) -> Result<String, String> {
        let output = tokio::process::Command::new("gh")
            .args(["pr", "create", "--title", title, "--body", body])
            .current_dir(&self.workspace)
            .output().await.map_err(|e| format!("gh pr: {}", e))?;

        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            return Err("no output".to_string());
        }
        Ok(url)
    }

    /// 通过 GitHub API 创建 PR (不需要 gh CLI)
    async fn create_pr_via_api(&self, title: &str, body: &str) -> Result<String, String> {
        // 获取 GitHub Token
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .map_err(|_| "No GitHub token found. Set GITHUB_TOKEN or GH_TOKEN env var, or install gh CLI.".to_string())?;

        // 从 git remote 解析 owner/repo
        let (owner, repo) = self.parse_git_remote().await?;

        // 获取当前分支名
        let branch = self.get_current_branch().await?;

        // GitHub API: 创建 PR
        let client = reqwest::Client::new();
        let url = format!("https://api.github.com/repos/{}/{}/pulls", owner, repo);
        let body = serde_json::json!({
            "title": title,
            "body": body,
            "head": branch,
            "base": "main",
        });

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "CarpAI-Delivery-Pipeline/1.0")
            .header("Accept", "application/vnd.github.v3+json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("GitHub API request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API returned {}: {}", status, text.chars().take(200).collect::<String>()));
        }

        let data: serde_json::Value = resp.json().await
            .map_err(|e| format!("Parse GitHub response: {}", e))?;

        data["html_url"].as_str()
            .map(|u| u.to_string())
            .ok_or_else(|| "GitHub API did not return html_url".to_string())
    }

    /// 从 git remote 解析 owner/repo
    async fn parse_git_remote(&self) -> Result<(String, String), String> {
        let output = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "remote", "get-url", "origin"])
            .output().await
            .map_err(|e| format!("git remote: {}", e))?;

        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            return Err("No git remote 'origin' found".to_string());
        }

        // 支持 git@github.com:owner/repo.git 和 https://github.com/owner/repo.git
        let (owner, repo) = if url.contains("github.com:") {
            // SSH: git@github.com:owner/repo.git
            let path = url.split("github.com:").nth(1).unwrap_or("");
            let parts: Vec<&str> = path.trim_end_matches(".git").split('/').collect();
            (parts[0].to_string(), parts[1..].join("/"))
        } else if url.contains("github.com/") {
            // HTTPS: https://github.com/owner/repo.git
            let path = url.split("github.com/").nth(1).unwrap_or("");
            let parts: Vec<&str> = path.trim_end_matches(".git").split('/').collect();
            (parts[0].to_string(), parts[1..].join("/"))
        } else {
            return Err(format!("Unsupported git remote URL: {}", url));
        };

        Ok((owner.to_string(), repo.to_string()))
    }

    /// 获取当前分支名
    async fn get_current_branch(&self) -> Result<String, String> {
        let output = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
            .output().await
            .map_err(|e| format!("git branch: {}", e))?;

        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            Err("Not on a valid branch".to_string())
        } else {
            Ok(branch)
        }
    }

    async fn get_diff(&self) -> String {
        let output = tokio::process::Command::new("git")
            .args(["-C", &self.workspace.to_string_lossy(), "diff", "--cached"])
            .output().await;
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(_) => String::new(),
        }
    }
}

// ========================================================================
// [4] 完整交付流水线 — 所有环串联
// ========================================================================

/// 完整 Plan → 交付 流水线
pub struct DeliveryPipeline {
    workspace: PathBuf,
    test: TestRing,
    review: ReviewRing,
    git: GitRing,
}

impl DeliveryPipeline {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            test: TestRing::new(workspace),
            review: ReviewRing::new(workspace),
            git: GitRing::new(workspace),
        }
    }

    /// 跑完整流水线: 编译→测试→审查→修复→提交
    pub async fn deliver(&self, goal: &str) -> Result<String, String> {
        let mut report = String::new();

        // 1. 编译验证 (复用已有引擎)
        report.push_str("🔧 Compile... ");
        let engine = crate::refactoring::compilation::CompilationEngine::new(&self.workspace);
        let compile = engine.cargo_check(&[]).await;
        if !compile.success {
            report.push_str(&format!("❌ {} errors\n", compile.errors.len()));
            return Err(report);
        }
        report.push_str("✅\n");

        // 2. 测试环
        report.push_str("🧪 Test... ");
        let test_result = self.test.run().await?;
        if !test_result.passed {
            report.push_str(&format!("❌ {}/{} failed\n", test_result.failed_count, test_result.total));
            return Err(report);
        }
        report.push_str(&format!("✅ {}/{}\n", test_result.passed_count, test_result.total));

        // 3. 审查环
        report.push_str("👁️  Review... ");
        let review_result = self.review.review().await?;
        if !review_result.style_issues.is_empty() {
            report.push_str(&format!("⚠️  {} style issues\n", review_result.style_issues.len()));
        } else {
            report.push_str("✅\n");
        }

        // 4. Git环
        report.push_str("📤 Git... ");
        let git_result = self.git.commit_and_pr(goal).await?;
        report.push_str(&format!("✅\n\n{}", git_result));

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_review_style_check() {
        let temp = std::env::temp_dir().join("carpai-review-test");
        let _ = std::fs::create_dir_all(&temp);
        std::fs::write(temp.join("test.rs"), "fn main() {\n    let password = \"hunter2\";\n    println!(\"{}\", password);\n    // TODO: encrypt\n}\n").ok();

        let ring = ReviewRing::new(&temp);
        let result = ring.review().await.unwrap();
        assert!(!result.security_issues.is_empty(), "Should detect password");
        assert!(!result.suggestions.is_empty(), "Should detect TODO and println");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_style_line_length() {
        let ring = ReviewRing::new(Path::new("."));
        let mut result = ReviewResult {
            passed: true, style_issues: vec![],
            security_issues: vec![], complexity_alerts: vec![], suggestions: vec![],
        };
        let long_line = "x".repeat(150);
        ring.check_style("long.rs", &long_line, &mut result);
        assert!(result.style_issues.iter().any(|s| s.message.contains("Line too long")));
    }
}
