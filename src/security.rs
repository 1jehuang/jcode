use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityVulnerability {
    pub id: String,
    pub file_path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub severity: VulnerabilitySeverity,
    pub category: VulnerabilityCategory,
    pub description: String,
    pub exploit_scenario: String,
    pub recommendation: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VulnerabilitySeverity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum VulnerabilityCategory {
    SqlInjection,
    CommandInjection,
    XxeInjection,
    TemplateInjection,
    NoSqlInjection,
    PathTraversal,
    AuthenticationBypass,
    PrivilegeEscalation,
    SessionManagement,
    JwtVulnerability,
    AuthorizationBypass,
    HardcodedSecrets,
    WeakCrypto,
    RemoteCodeExecution,
    Deserialization,
    Xss,
    SensitiveDataExposure,
    ApiDataLeakage,
    DebugInfoExposure,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SecurityScanner {
    rules: Vec<SecurityRule>,
    enabled_categories: HashSet<VulnerabilityCategory>,
    min_confidence: f64,
}

impl Default for SecurityScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SecurityScanner {
    pub fn new() -> Self {
        Self {
            rules: Self::default_rules(),
            enabled_categories: Self::default_categories(),
            min_confidence: 0.7,
        }
    }

    pub fn with_config(min_confidence: f64, enabled_categories: HashSet<VulnerabilityCategory>) -> Self {
        Self {
            rules: Self::default_rules(),
            enabled_categories,
            min_confidence,
        }
    }

    fn default_rules() -> Vec<SecurityRule> {
        vec![
            SecurityRule {
                name: "sql_injection_direct".to_string(),
                category: VulnerabilityCategory::SqlInjection,
                pattern: "(?i)(select|insert|update|delete|drop|union)\\s+.*[\\'\\\"].*[%_]".to_string(),
                description: "Potential SQL injection via direct string concatenation".to_string(),
                severity: VulnerabilitySeverity::High,
                confidence: 0.85,
            },
            SecurityRule {
                name: "sql_injection_format".to_string(),
                category: VulnerabilityCategory::SqlInjection,
                pattern: "(?i)format!\\s*\\([^)]*[\\'\\\"].*[+]\\s*[\\w]+\\)".to_string(),
                description: "Potential SQL injection via format string".to_string(),
                severity: VulnerabilitySeverity::High,
                confidence: 0.75,
            },
            SecurityRule {
                name: "command_injection".to_string(),
                category: VulnerabilityCategory::CommandInjection,
                pattern: "(?i)(system|exec|spawn|Command::new)\\s*\\([^)]*[+]\\s*[\\w]+\\)".to_string(),
                description: "Potential command injection".to_string(),
                severity: VulnerabilitySeverity::Critical,
                confidence: 0.9,
            },
            SecurityRule {
                name: "path_traversal".to_string(),
                category: VulnerabilityCategory::PathTraversal,
                pattern: "(?i)\\.\\.[/\\\\]".to_string(),
                description: "Path traversal attempt detected".to_string(),
                severity: VulnerabilitySeverity::High,
                confidence: 0.95,
            },
            SecurityRule {
                name: "hardcoded_password".to_string(),
                category: VulnerabilityCategory::HardcodedSecrets,
                pattern: "(?i)(password|secret|api[_]?key|token)\\s*[=:]\\s*[\\'\\\"][^\\'\\\"]{8,}[\\'\\\"]".to_string(),
                description: "Potential hardcoded secret detected".to_string(),
                severity: VulnerabilitySeverity::Critical,
                confidence: 0.8,
            },
            SecurityRule {
                name: "hardcoded_jwt".to_string(),
                category: VulnerabilityCategory::HardcodedSecrets,
                pattern: "(?i)ey[A-Za-z0-9_-]*\\.[A-Za-z0-9_-]*\\.[A-Za-z0-9_-]*".to_string(),
                description: "Potential hardcoded JWT token".to_string(),
                severity: VulnerabilitySeverity::Critical,
                confidence: 0.95,
            },
            SecurityRule {
                name: "unsafe_eval".to_string(),
                category: VulnerabilityCategory::RemoteCodeExecution,
                pattern: "(?i)eval\\s*\\([^)]*\\)".to_string(),
                description: "Potential unsafe eval usage".to_string(),
                severity: VulnerabilitySeverity::High,
                confidence: 0.85,
            },
            SecurityRule {
                name: "deserialization".to_string(),
                category: VulnerabilityCategory::Deserialization,
                pattern: "(?i)(serde_json::from_str|bincode::deserialize|rmp_serde::from_slice)".to_string(),
                description: "Potential unsafe deserialization".to_string(),
                severity: VulnerabilitySeverity::Medium,
                confidence: 0.6,
            },
            SecurityRule {
                name: "xss_rust_html".to_string(),
                category: VulnerabilityCategory::Xss,
                pattern: "(?i)(Html::from_str|format!\\s*\\([^)]*<[^)]*\\))".to_string(),
                description: "Potential XSS via HTML construction".to_string(),
                severity: VulnerabilitySeverity::Medium,
                confidence: 0.7,
            },
            SecurityRule {
                name: "debug_info".to_string(),
                category: VulnerabilityCategory::DebugInfoExposure,
                pattern: "(?i)(RUST_BACKTRACE=1|debug=true|trace=true)".to_string(),
                description: "Debug mode enabled in production".to_string(),
                severity: VulnerabilitySeverity::Medium,
                confidence: 0.8,
            },
            SecurityRule {
                name: "weak_random".to_string(),
                category: VulnerabilityCategory::WeakCrypto,
                pattern: "(?i)(rand::random|random::thread_rng\\(\\))".to_string(),
                description: "Potential weak random number generation".to_string(),
                severity: VulnerabilitySeverity::Low,
                confidence: 0.65,
            },
            SecurityRule {
                name: "unsafe_block".to_string(),
                category: VulnerabilityCategory::RemoteCodeExecution,
                pattern: "(?i)unsafe\\s*\\{".to_string(),
                description: "Unsafe block usage detected".to_string(),
                severity: VulnerabilitySeverity::Medium,
                confidence: 0.5,
            },
        ]
    }

    fn default_categories() -> HashSet<VulnerabilityCategory> {
        [
            VulnerabilityCategory::SqlInjection,
            VulnerabilityCategory::CommandInjection,
            VulnerabilityCategory::PathTraversal,
            VulnerabilityCategory::HardcodedSecrets,
            VulnerabilityCategory::RemoteCodeExecution,
            VulnerabilityCategory::Deserialization,
            VulnerabilityCategory::Xss,
            VulnerabilityCategory::DebugInfoExposure,
            VulnerabilityCategory::WeakCrypto,
        ]
        .iter()
        .cloned()
        .collect()
    }

    pub fn scan(&self, file_path: &str, content: &str) -> Vec<SecurityVulnerability> {
        let mut vulnerabilities = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        for rule in &self.rules {
            if !self.enabled_categories.contains(&rule.category) {
                continue;
            }

            if rule.confidence < self.min_confidence {
                continue;
            }

            let regex = match regex::Regex::new(&rule.pattern) {
                Ok(r) => r,
                Err(_) => continue,
            };

            for (i, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    let vulnerability = SecurityVulnerability {
                        id: format!("{}-{}", rule.name, i),
                        file_path: file_path.to_string(),
                        line: Some(i + 1),
                        column: None,
                        severity: rule.severity.clone(),
                        category: rule.category.clone(),
                        description: rule.description.clone(),
                        exploit_scenario: self.generate_exploit_scenario(&rule.category),
                        recommendation: self.generate_recommendation(&rule.category),
                        confidence: rule.confidence,
                    };
                    vulnerabilities.push(vulnerability);
                }
            }
        }

        vulnerabilities.sort_by(|a, b| b.severity.cmp(&a.severity));
        vulnerabilities
    }

    pub fn scan_directory(&self, dir_path: &str) -> Vec<SecurityVulnerability> {
        let mut all_vulnerabilities = Vec::new();
        
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if self.is_supported_file(ext)
                        && let Ok(content) = std::fs::read_to_string(&path) {
                            let file_path = path.to_string_lossy().to_string();
                            let vulnerabilities = self.scan(&file_path, &content);
                            all_vulnerabilities.extend(vulnerabilities);
                        }
                } else if path.is_dir() {
                    let sub_vulnerabilities = self.scan_directory(path.to_string_lossy().as_ref());
                    all_vulnerabilities.extend(sub_vulnerabilities);
                }
            }
        }

        all_vulnerabilities.sort_by(|a, b| b.severity.cmp(&a.severity));
        all_vulnerabilities
    }

    fn is_supported_file(&self, ext: &str) -> bool {
        matches!(
            ext,
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "java" | "cpp" | "c" | "rb"
        )
    }

    fn generate_exploit_scenario(&self, category: &VulnerabilityCategory) -> String {
        match category {
            VulnerabilityCategory::SqlInjection => "Attacker crafts malicious SQL input that gets executed without sanitization, potentially exposing or modifying sensitive data.".to_string(),
            VulnerabilityCategory::CommandInjection => "Attacker injects malicious shell commands that execute with the application's privileges, potentially gaining full system access.".to_string(),
            VulnerabilityCategory::PathTraversal => "Attacker navigates to sensitive files outside the intended directory structure, potentially accessing credentials or configuration files.".to_string(),
            VulnerabilityCategory::HardcodedSecrets => "Exposed credentials can be extracted from source code, giving attackers unauthorized access to systems or APIs.".to_string(),
            VulnerabilityCategory::RemoteCodeExecution => "Attacker can execute arbitrary code in the application context, potentially gaining full control over the system.".to_string(),
            VulnerabilityCategory::Deserialization => "Attacker crafts malicious serialized payloads that execute arbitrary code during deserialization.".to_string(),
            VulnerabilityCategory::Xss => "Attacker injects malicious JavaScript that executes in the victim's browser, enabling session hijacking or data theft.".to_string(),
            VulnerabilityCategory::DebugInfoExposure => "Exposed debug information reveals internal system details that can aid attackers in planning more sophisticated attacks.".to_string(),
            VulnerabilityCategory::WeakCrypto => "Weak cryptographic practices allow attackers to easily decrypt sensitive data or forge signatures.".to_string(),
            _ => "Exploit scenario depends on specific implementation details.".to_string(),
        }
    }

    fn generate_recommendation(&self, category: &VulnerabilityCategory) -> String {
        match category {
            VulnerabilityCategory::SqlInjection => "Use parameterized queries or prepared statements instead of string concatenation for SQL queries.".to_string(),
            VulnerabilityCategory::CommandInjection => "Avoid executing shell commands with user input. If necessary, use safe command builders with proper argument separation.".to_string(),
            VulnerabilityCategory::PathTraversal => "Validate and sanitize all user-provided paths. Use path canonicalization and enforce whitelist-based access control.".to_string(),
            VulnerabilityCategory::HardcodedSecrets => "Store secrets in environment variables, secure vaults, or configuration files outside version control.".to_string(),
            VulnerabilityCategory::RemoteCodeExecution => "Avoid using eval() or similar dynamic code execution functions with untrusted input.".to_string(),
            VulnerabilityCategory::Deserialization => "Use safe deserialization libraries that validate input structure. Avoid deserializing untrusted data.".to_string(),
            VulnerabilityCategory::Xss => "Use framework-provided escaping for HTML output. Implement Content Security Policy headers.".to_string(),
            VulnerabilityCategory::DebugInfoExposure => "Disable debug mode in production environments. Use structured logging instead of verbose debug output.".to_string(),
            VulnerabilityCategory::WeakCrypto => "Use cryptographically secure random number generators. Follow established cryptographic best practices.".to_string(),
            _ => "Follow secure coding best practices for this vulnerability category.".to_string(),
        }
    }

    pub fn get_report(&self, vulnerabilities: &[SecurityVulnerability]) -> SecurityReport {
        let mut severity_counts = HashMap::new();
        let mut category_counts = HashMap::new();

        for v in vulnerabilities {
            *severity_counts.entry(v.severity.clone()).or_insert(0) += 1;
            *category_counts.entry(v.category.clone()).or_insert(0) += 1;
        }

        SecurityReport {
            total_vulnerabilities: vulnerabilities.len(),
            critical_count: severity_counts.get(&VulnerabilitySeverity::Critical).copied().unwrap_or(0),
            high_count: severity_counts.get(&VulnerabilitySeverity::High).copied().unwrap_or(0),
            medium_count: severity_counts.get(&VulnerabilitySeverity::Medium).copied().unwrap_or(0),
            low_count: severity_counts.get(&VulnerabilitySeverity::Low).copied().unwrap_or(0),
            vulnerabilities: vulnerabilities.to_vec(),
            severity_distribution: severity_counts,
            category_distribution: category_counts,
        }
    }

    pub fn enable_category(&mut self, category: VulnerabilityCategory) {
        self.enabled_categories.insert(category);
    }

    pub fn disable_category(&mut self, category: VulnerabilityCategory) {
        self.enabled_categories.remove(&category);
    }
}

#[derive(Debug, Clone)]
pub struct SecurityRule {
    pub name: String,
    pub category: VulnerabilityCategory,
    pub pattern: String,
    pub description: String,
    pub severity: VulnerabilitySeverity,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityReport {
    pub total_vulnerabilities: usize,
    pub critical_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    pub vulnerabilities: Vec<SecurityVulnerability>,
    pub severity_distribution: HashMap<VulnerabilitySeverity, usize>,
    pub category_distribution: HashMap<VulnerabilityCategory, usize>,
}

impl SecurityReport {
    pub fn to_markdown(&self) -> String {
        let mut markdown = String::from("# Security Scan Report\n\n");
        markdown.push_str(&format!("**Total Vulnerabilities:** {}\n\n", self.total_vulnerabilities));
        markdown.push_str("## Severity Distribution\n\n");
        markdown.push_str("| Severity | Count |\n");
        markdown.push_str("|----------|-------|\n");
        markdown.push_str(&format!("| Critical | {} |\n", self.critical_count));
        markdown.push_str(&format!("| High | {} |\n", self.high_count));
        markdown.push_str(&format!("| Medium | {} |\n", self.medium_count));
        markdown.push_str(&format!("| Low | {} |\n\n", self.low_count));

        markdown.push_str("## Vulnerabilities\n\n");
        for (i, vuln) in self.vulnerabilities.iter().enumerate() {
            markdown.push_str(&format!(
                "### Vuln {}: {} (`{}:{}`)\n\n",
                i + 1,
                self.category_to_string(&vuln.category),
                vuln.file_path,
                vuln.line.unwrap_or(0)
            ));
            markdown.push_str(&format!("* **Severity:** {}\n", self.severity_to_string(&vuln.severity)));
            markdown.push_str(&format!("* **Description:** {}\n", vuln.description));
            markdown.push_str(&format!("* **Exploit Scenario:** {}\n", vuln.exploit_scenario));
            markdown.push_str(&format!("* **Recommendation:** {}\n\n", vuln.recommendation));
        }

        markdown
    }

    fn severity_to_string(&self, severity: &VulnerabilitySeverity) -> String {
        match severity {
            VulnerabilitySeverity::Critical => "Critical",
            VulnerabilitySeverity::High => "High",
            VulnerabilitySeverity::Medium => "Medium",
            VulnerabilitySeverity::Low => "Low",
        }
        .to_string()
    }

    fn category_to_string(&self, category: &VulnerabilityCategory) -> String {
        match category {
            VulnerabilityCategory::SqlInjection => "SQL Injection",
            VulnerabilityCategory::CommandInjection => "Command Injection",
            VulnerabilityCategory::XxeInjection => "XXE Injection",
            VulnerabilityCategory::TemplateInjection => "Template Injection",
            VulnerabilityCategory::NoSqlInjection => "NoSQL Injection",
            VulnerabilityCategory::PathTraversal => "Path Traversal",
            VulnerabilityCategory::AuthenticationBypass => "Authentication Bypass",
            VulnerabilityCategory::PrivilegeEscalation => "Privilege Escalation",
            VulnerabilityCategory::SessionManagement => "Session Management",
            VulnerabilityCategory::JwtVulnerability => "JWT Vulnerability",
            VulnerabilityCategory::AuthorizationBypass => "Authorization Bypass",
            VulnerabilityCategory::HardcodedSecrets => "Hardcoded Secrets",
            VulnerabilityCategory::WeakCrypto => "Weak Cryptography",
            VulnerabilityCategory::RemoteCodeExecution => "Remote Code Execution",
            VulnerabilityCategory::Deserialization => "Deserialization",
            VulnerabilityCategory::Xss => "XSS",
            VulnerabilityCategory::SensitiveDataExposure => "Sensitive Data Exposure",
            VulnerabilityCategory::ApiDataLeakage => "API Data Leakage",
            VulnerabilityCategory::DebugInfoExposure => "Debug Info Exposure",
            VulnerabilityCategory::Unknown => "Unknown",
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_injection_detection() {
        let scanner = SecurityScanner::new();
        let code = "let query = format!(\"SELECT * FROM users WHERE id = '{}'\", user_input);";
        let vulnerabilities = scanner.scan("test.rs", code);
        
        assert!(vulnerabilities.len() > 0);
        assert!(vulnerabilities.iter().any(|v| v.category == VulnerabilityCategory::SqlInjection));
    }

    #[test]
    fn test_hardcoded_secret_detection() {
        let scanner = SecurityScanner::new();
        let code = "let api_key = \"sk-1234567890abcdef1234567890abcdef\";";
        let vulnerabilities = scanner.scan("test.rs", code);
        
        assert!(vulnerabilities.len() > 0);
        assert!(vulnerabilities.iter().any(|v| v.category == VulnerabilityCategory::HardcodedSecrets));
    }

    #[test]
    fn test_path_traversal_detection() {
        let scanner = SecurityScanner::new();
        let code = "let path = format!(\"/var/data/{}\", user_input);";
        let vulnerabilities = scanner.scan("test.rs", code);
        
        assert!(vulnerabilities.len() > 0);
        assert!(vulnerabilities.iter().any(|v| v.category == VulnerabilityCategory::PathTraversal));
    }

    #[test]
    fn test_report_generation() {
        let scanner = SecurityScanner::new();
        let code = "let password = \"secret123\";";
        let vulnerabilities = scanner.scan("test.rs", code);
        let report = scanner.get_report(&vulnerabilities);
        
        assert_eq!(report.total_vulnerabilities, vulnerabilities.len());
        assert!(report.to_markdown().contains("Security Scan Report"));
    }

    #[test]
    fn test_category_filtering() {
        let mut scanner = SecurityScanner::new();
        scanner.disable_category(VulnerabilityCategory::HardcodedSecrets);
        
        let code = "let password = \"secret123\";";
        let vulnerabilities = scanner.scan("test.rs", code);
        
        assert!(vulnerabilities.is_empty());
    }
}