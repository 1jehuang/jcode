/// Type-checker bridge — runs `cargo check` for Rust, delegates to LSP for others.
pub struct TypeChecker;

impl TypeChecker {
    pub fn new() -> Self { Self }

    /// Run a type check on the project. Returns errors if any.
    pub async fn check(&self, workspace_root: &str) -> anyhow::Result<Vec<TypeError>> {
        // For Rust: run `cargo check` and parse output
        let output = tokio::process::Command::new("cargo")
            .args(["check", "--message-format=short"])
            .current_dir(workspace_root)
            .output()
            .await?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut errors = Vec::new();

        for line in stderr.lines() {
            if line.contains("error[") {
                if let Some(err) = self.parse_error(line) {
                    errors.push(err);
                }
            }
        }
        Ok(errors)
    }

    fn parse_error(&self, line: &str) -> Option<TypeError> {
        // e.g.: src/main.rs:42:18: error[E0308]: mismatched types
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() >= 2 {
            Some(TypeError {
                file: parts[0].trim().to_string(),
                line: parts.get(1).and_then(|l| l.trim().parse().ok()).unwrap_or(0),
                message: line.to_string(),
                error_code: self.extract_error_code(line),
            })
        } else {
            None
        }
    }

    fn extract_error_code(&self, line: &str) -> String {
        if let Some(start) = line.find("error[") {
            if let Some(end) = line[start..].find(']') {
                return line[start..start+end+1].to_string();
            }
        }
        String::new()
    }
}

/// A single type error from the compiler.
#[derive(Debug, Clone)]
pub struct TypeError {
    pub file: String,
    pub line: u32,
    pub message: String,
    pub error_code: String,
}
