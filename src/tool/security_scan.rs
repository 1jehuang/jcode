use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

pub struct SecurityScanTool;

impl SecurityScanTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct SecurityScanInput {
    #[serde(default = "default_scan_path")]
    path: String,
    #[serde(default = "default_scan_type")]
    scan_type: String,
    #[serde(default)]
    severity_threshold: Option<String>,
    #[serde(default)]
    output_format: Option<String>,
}

fn default_scan_path() -> String {
    ".".to_string()
}

fn default_scan_type() -> String {
    "all".to_string()
}

#[derive(Debug)]
struct SecurityIssue {
    severity: String,
    title: String,
    description: String,
    file: String,
    line: Option<usize>,
    cwe_id: Option<String>,
    cve_id: Option<String>,
}

impl SecurityIssue {
    fn to_markdown(&self) -> String {
        let severity_icon = match self.severity.as_str() {
            "critical" => "🔴",
            "high" => "🟠",
            "medium" => "🟡",
            "low" => "🔵",
            _ => "⚪",
        };

        let mut output = format!(
            "### {} {} [{}]\n\n{}\n\n",
            severity_icon, self.title, self.severity, self.description
        );

        if let Some(line) = self.line {
            output.push_str(&format!("**Location:** `{}` (line {})\n\n", self.file, line));
        } else {
            output.push_str(&format!("**Location:** `{}`\n\n", self.file));
        }

        if let Some(cwe) = &self.cwe_id {
            output.push_str(&format!("- **CWE:** {}\n", cwe));
        }

        if let Some(cve) = &self.cve_id {
            output.push_str(&format!("- **CVE:** {}\n", cve));
        }

        output
    }
}

#[async_trait]
impl Tool for SecurityScanTool {
    fn name(&self) -> &str {
        "security_scan"
    }

    fn description(&self) -> &str {
        "Perform security vulnerability scanning on code. Supports multiple scan types including static analysis, dependency checks, and secret detection."
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "intent": super::intent_schema_property(),
                "path": {
                    "type": "string",
                    "description": "Path to scan (default: current directory)"
                },
                "scan_type": {
                    "type": "string",
                    "enum": ["all", "static", "dependency", "secret", "config"],
                    "description": "Type of security scan to perform"
                },
                "severity_threshold": {
                    "type": "string",
                    "enum": ["critical", "high", "medium", "low"],
                    "description": "Minimum severity level to report (default: low)"
                },
                "output_format": {
                    "type": "string",
                    "enum": ["markdown", "json", "text"],
                    "description": "Output format (default: markdown)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SecurityScanInput = serde_json::from_value(input)?;

        let scan_path = ctx.resolve_path(Path::new(&params.path));

        if !scan_path.exists() {
            return Ok(ToolOutput::error(format!(
                "Path does not exist: {}",
                params.path
            )));
        }

        let mut issues = Vec::new();

        match params.scan_type.as_str() {
            "all" => {
                self.scan_static_analysis(scan_path, &mut issues)?;
                self.scan_dependencies(scan_path, &mut issues)?;
                self.scan_secrets(scan_path, &mut issues)?;
                self.scan_config(scan_path, &mut issues)?;
            }
            "static" => {
                self.scan_static_analysis(scan_path, &mut issues)?;
            }
            "dependency" => {
                self.scan_dependencies(scan_path, &mut issues)?;
            }
            "secret" => {
                self.scan_secrets(scan_path, &mut issues)?;
            }
            "config" => {
                self.scan_config(scan_path, &mut issues)?;
            }
            _ => {
                return Ok(ToolOutput::error(format!(
                    "Unknown scan type: {}. Supported types: all, static, dependency, secret, config",
                    params.scan_type
                )));
            }
        }

        // Filter by severity threshold
        if let Some(threshold) = &params.severity_threshold {
            issues.retain(|issue| {
                self.severity_meets_threshold(&issue.severity, threshold)
            });
        }

        // Sort by severity (critical first)
        issues.sort_by(|a, b| {
            self.severity_order(&b.severity).cmp(&self.severity_order(&a.severity))
        });

        let output_format = params.output_format.unwrap_or_else(|| "markdown".to_string());

        let output = match output_format.as_str() {
            "json" => self.format_json(&issues),
            "text" => self.format_text(&issues),
            _ => self.format_markdown(&issues),
        };

        let summary = format!(
            "Security scan completed. Found {} issue(s): {} critical, {} high, {} medium, {} low.",
            issues.len(),
            issues.iter().filter(|i| i.severity == "critical").count(),
            issues.iter().filter(|i| i.severity == "high").count(),
            issues.iter().filter(|i| i.severity == "medium").count(),
            issues.iter().filter(|i| i.severity == "low").count(),
        );

        Ok(ToolOutput::success(json!({
            "summary": summary,
            "total_issues": issues.len(),
            "details": output,
            "scan_path": params.path,
            "scan_type": params.scan_type,
        })))
    }
}

impl SecurityScanTool {
    fn scan_static_analysis(&self, path: &Path, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Check for common security anti-patterns using agentgrep
        let patterns = vec![
            (
                r"(?i)(eval|exec|system|shell_exec|passthru)\s*\(",
                "Dangerous function call detected",
                "Use of eval/exec/system can lead to code injection vulnerabilities",
                "high",
                Some("CWE-78"),
            ),
            (
                r"(?i)(SELECT|INSERT|UPDATE|DELETE).*\+.*",
                "Potential SQL injection",
                "String concatenation in SQL queries can lead to SQL injection",
                "critical",
                Some("CWE-89"),
            ),
            (
                r"(?i)<script[^>]*>.*\+.*</script>",
                "Potential XSS vulnerability",
                "Unsanitized user input in HTML output can lead to XSS",
                "high",
                Some("CWE-79"),
            ),
            (
                r"(?i)(password|secret|token|api_key)\s*=\s*['\"][^'\"]+['\"]",
                "Hardcoded credentials detected",
                "Hardcoded secrets should be moved to environment variables or secure storage",
                "critical",
                Some("CWE-798"),
            ),
            (
                r"(?i)md5\s*\(|sha1\s*\(",
                "Weak cryptographic hash",
                "MD5 and SHA1 are considered weak for security purposes",
                "medium",
                Some("CWE-328"),
            ),
            (
                r"(?i)openssl_verify_mode.*VERIFY_NONE",
                "SSL verification disabled",
                "Disabling SSL verification allows man-in-the-middle attacks",
                "high",
                Some("CWE-295"),
            ),
        ];

        for (pattern, title, description, severity, cwe) in patterns {
            self.grep_pattern(path, pattern, title, description, severity, cwe, issues)?;
        }

        Ok(())
    }

    fn scan_dependencies(&self, path: &Path, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Check for known vulnerable dependencies
        if path.join("Cargo.toml").exists() {
            self.check_rust_dependencies(path, issues)?;
        }

        if path.join("package.json").exists() {
            self.check_node_dependencies(path, issues)?;
        }

        if path.join("requirements.txt").exists() || path.join("setup.py").exists() {
            self.check_python_dependencies(path, issues)?;
        }

        Ok(())
    }

    fn check_rust_dependencies(&self, path: &Path, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Run cargo audit if available
        let output = Command::new("cargo")
            .args(&["audit"])
            .current_dir(path)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                // No vulnerabilities found
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);

                // Parse vulnerability information
                for line in stderr.lines().chain(stdout.lines()) {
                    if line.contains("vulnerability") || line.contains("RUSTSEC") {
                        issues.push(SecurityIssue {
                            severity: "high".to_string(),
                            title: "Dependency vulnerability found".to_string(),
                            description: line.trim().to_string(),
                            file: "Cargo.lock".to_string(),
                            line: None,
                            cwe_id: None,
                            cve_id: None,
                        });
                    }
                }
            }
            Err(_) => {
                // cargo audit not installed, skip
            }
        }

        Ok(())
    }

    fn check_node_dependencies(&self, path: &Path, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Run npm audit if available
        let output = Command::new("npm")
            .args(&["audit", "--json"])
            .current_dir(path)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                // Parse npm audit JSON output
                if let Ok(audit_result) = serde_json::from_slice::<Value>(&output.stdout) {
                    if let Some(vulnerabilities) = audit_result.get("vulnerabilities") {
                        if let Some(obj) = vulnerabilities.as_object() {
                            for (pkg, vuln_info) in obj {
                                let severity = vuln_info
                                    .get("severity")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("medium");

                                let via = vuln_info
                                    .get("via")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| arr.len())
                                    .unwrap_or(0);

                                issues.push(SecurityIssue {
                                    severity: severity.to_string(),
                                    title: format!("Vulnerable dependency: {}", pkg),
                                    description: format!(
                                        "Package '{}' has {} known vulnerability/ies",
                                        pkg, via
                                    ),
                                    file: "package.json".to_string(),
                                    line: None,
                                    cwe_id: None,
                                    cve_id: None,
                                });
                            }
                        }
                    }
                }
            }
            _ => {
                // npm audit failed or not installed, skip
            }
        }

        Ok(())
    }

    fn check_python_dependencies(
        &self,
        path: &Path,
        issues: &mut Vec<SecurityIssue>,
    ) -> Result<()> {
        // Run pip-audit if available
        let output = Command::new("pip-audit")
            .args(&["--format", "json"])
            .current_dir(path)
            .output();

        match output {
            Ok(output) if output.status.success() => {
                if let Ok(audit_result) = serde_json::from_slice::<Value>(&output.stdout) {
                    if let Some(vulnerabilities) = audit_result.as_array() {
                        for vuln in vulnerabilities {
                            let name = vuln
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");

                            let version = vuln
                                .get("version")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");

                            let id = vuln
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");

                            issues.push(SecurityIssue {
                                severity: "high".to_string(),
                                title: format!("Vulnerable Python package: {}", name),
                                description: format!(
                                    "Package {} {} has known vulnerability: {}",
                                    name, version, id
                                ),
                                file: "requirements.txt".to_string(),
                                line: None,
                                cwe_id: None,
                                cve_id: Some(id.to_string()),
                            });
                        }
                    }
                }
            }
            _ => {
                // pip-audit not installed, skip
            }
        }

        Ok(())
    }

    fn scan_secrets(&self, path: &Path, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Check for common secret patterns
        let secret_patterns = vec![
            (
                r"(?i)(aws_access_key_id|aws_secret_access_key)\s*=\s*[A-Z0-9]{20}",
                "AWS credentials detected",
                "AWS access keys should not be hardcoded in source code",
                "critical",
                None,
            ),
            (
                r"(?i)(private_key|BEGIN RSA PRIVATE KEY|BEGIN EC PRIVATE KEY)",
                "Private key detected",
                "Private keys should not be committed to version control",
                "critical",
                None,
            ),
            (
                r"(?i)(github_token|ghp_[a-zA-Z0-9]{36})",
                "GitHub token detected",
                "GitHub tokens should not be hardcoded in source code",
                "critical",
                None,
            ),
            (
                r"(?i)(slack_token|xox[baprs]-[0-9a-zA-Z-]+)",
                "Slack token detected",
                "Slack tokens should not be hardcoded in source code",
                "high",
                None,
            ),
        ];

        for (pattern, title, description, severity, cwe) in secret_patterns {
            self.grep_pattern(path, pattern, title, description, severity, cwe, issues)?;
        }

        Ok(())
    }

    fn scan_config(&self, path: &Path, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Check for insecure configuration patterns
        let config_patterns = vec![
            (
                r"(?i)debug\s*=\s*true",
                "Debug mode enabled",
                "Debug mode should be disabled in production",
                "medium",
                None,
            ),
            (
                r"(?i)cors.*origin.*\*",
                "Wildcard CORS policy",
                "Wildcard CORS policy may expose your API to unauthorized origins",
                "medium",
                None,
            ),
            (
                r"(?i)allow_all_origins|allow_origin.*\*",
                "Permissive CORS configuration",
                "Overly permissive CORS configuration can lead to security issues",
                "medium",
                None,
            ),
        ];

        for (pattern, title, description, severity, cwe) in config_patterns {
            self.grep_pattern(path, pattern, title, description, severity, cwe, issues)?;
        }

        Ok(())
    }

    fn grep_pattern(
        &self,
        path: &Path,
        pattern: &str,
        title: &str,
        description: &str,
        severity: &str,
        cwe: Option<&str>,
        issues: &mut Vec<SecurityIssue>,
    ) -> Result<()> {
        use regex::Regex;
        use walkdir::WalkDir;

        let re = Regex::new(pattern)?;

        for entry in WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .take(1000) // Limit to avoid excessive scanning
        {
            let file_path = entry.path();

            // Skip binary files and common non-source directories
            if let Some(dir_name) = file_path.parent().and_then(|p| p.file_name()) {
                if dir_name == ".git" || dir_name == "node_modules" || dir_name == "target" {
                    continue;
                }
            }

            if let Ok(content) = std::fs::read_to_string(file_path) {
                for (line_num, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        issues.push(SecurityIssue {
                            severity: severity.to_string(),
                            title: title.to_string(),
                            description: description.to_string(),
                            file: file_path
                                .strip_prefix(path)
                                .unwrap_or(file_path)
                                .to_string_lossy()
                                .to_string(),
                            line: Some(line_num + 1),
                            cwe_id: cwe.map(|s| s.to_string()),
                            cve_id: None,
                        });
                        break; // Only report once per file per pattern
                    }
                }
            }
        }

        Ok(())
    }

    fn severity_order(&self, severity: &str) -> u8 {
        match severity {
            "critical" => 4,
            "high" => 3,
            "medium" => 2,
            "low" => 1,
            _ => 0,
        }
    }

    fn severity_meets_threshold(&self, severity: &str, threshold: &str) -> bool {
        self.severity_order(severity) >= self.severity_order(threshold)
    }

    fn format_markdown(&self, issues: &[SecurityIssue]) -> String {
        if issues.is_empty() {
            return "✅ No security issues found.".to_string();
        }

        let mut output = String::from("# Security Scan Results\n\n");

        for issue in issues {
            output.push_str(&issue.to_markdown());
            output.push_str("---\n\n");
        }

        output
    }

    fn format_text(&self, issues: &[SecurityIssue]) -> String {
        if issues.is_empty() {
            return "No security issues found.".to_string();
        }

        let mut output = String::from("Security Scan Results:\n\n");

        for (i, issue) in issues.iter().enumerate() {
            output.push_str(&format!(
                "{}. [{}] {} - {}:{}\n   {}\n\n",
                i + 1,
                issue.severity.to_uppercase(),
                issue.title,
                issue.file,
                issue.line.unwrap_or(0),
                issue.description
            ));
        }

        output
    }

    fn format_json(&self, issues: &[SecurityIssue]) -> String {
        let json_issues: Vec<Value> = issues
            .iter()
            .map(|issue| {
                json!({
                    "severity": issue.severity,
                    "title": issue.title,
                    "description": issue.description,
                    "file": issue.file,
                    "line": issue.line,
                    "cwe_id": issue.cwe_id,
                    "cve_id": issue.cve_id,
                })
            })
            .collect();

        serde_json::to_string_pretty(&json_issues).unwrap_or_else(|_| "[]".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_ordering() {
        let tool = SecurityScanTool::new();
        assert_eq!(tool.severity_order("critical"), 4);
        assert_eq!(tool.severity_order("high"), 3);
        assert_eq!(tool.severity_order("medium"), 2);
        assert_eq!(tool.severity_order("low"), 1);
    }

    #[test]
    fn test_severity_threshold() {
        let tool = SecurityScanTool::new();
        assert!(tool.severity_meets_threshold("critical", "high"));
        assert!(tool.severity_meets_threshold("high", "high"));
        assert!(!tool.severity_meets_threshold("medium", "high"));
    }
}
