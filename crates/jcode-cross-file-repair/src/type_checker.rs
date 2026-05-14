/// Type-checker bridge — runs `cargo check` for Rust, delegates to LSP for others.

#[cfg(feature = "lsp-bridge")]
use jcode_lsp::LspServerManager;
#[cfg(feature = "lsp-bridge")]
use std::sync::Arc;

pub struct TypeChecker {
    #[cfg(feature = "lsp-bridge")]
    lsp_manager: Option<Arc<LspServerManager>>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "lsp-bridge")]
            lsp_manager: None,
        }
    }

    /// Create a TypeChecker with LSP support enabled.
    #[cfg(feature = "lsp-bridge")]
    pub fn with_lsp(manager: Arc<LspServerManager>) -> Self {
        Self {
            lsp_manager: Some(manager),
        }
    }

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
            if line.contains("error[E") || line.contains("error[") {
                if let Some(err) = self.parse_error(line) {
                    errors.push(err);
                }
            }
        }
        Ok(errors)
    }

    /// Parse a single error line from cargo check output.
    ///
    /// Handles both Unix and Windows paths:
    /// - Unix: `src/main.rs:42:18: error[E0308]: mismatched types`
    /// - Windows: `src\main.rs:42:18: error[E0308]: mismatched types`
    /// - Windows with drive: `C:\src\main.rs:42:18: error[E0308]: mismatched types`
    fn parse_error(&self, line: &str) -> Option<TypeError> {
        let error_idx = line.find("error[")?;

        let prefix = &line[..error_idx].trim_end();

        let parts: Vec<&str> = prefix.rsplitn(3, ':').collect();
        if parts.len() >= 3 {
            let column = parts[0].trim().parse().ok().unwrap_or(0);
            let line_num = parts[1].trim().parse().ok().unwrap_or(0);
            let file = parts[2].trim().to_string();

            Some(TypeError {
                file,
                line: line_num,
                message: line.to_string(),
                error_code: self.extract_error_code(line),
            })
        } else if parts.len() == 2 {
            let line_num = parts[0].trim().parse().ok().unwrap_or(0);
            let file = parts[1].trim().to_string();

            Some(TypeError {
                file,
                line: line_num,
                message: line.to_string(),
                error_code: self.extract_error_code(line),
            })
        } else {
            None
        }
    }

    /// Extract error code like "E0308" from "error[E0308]"
    fn extract_error_code(&self, line: &str) -> String {
        if let Some(start) = line.find("error[E") {
            let bracket_start = start + 6;
            let remaining = &line[bracket_start..];
            if let Some(end) = remaining.find(']') {
                return format!("E{}", &remaining[..end]);
            }
        }
        if let Some(start) = line.find("error[") {
            let bracket_start = start + 6;
            let remaining = &line[bracket_start..];
            if let Some(end) = remaining.find(']') {
                return remaining[..end].to_string();
            }
        }
        String::new()
    }

    /// Check using LSP diagnostics (for non-Rust projects or enhanced Rust diagnostics).
    ///
    /// When the `lsp-bridge` feature is enabled and a LSP manager has been configured,
    /// this queries the LSP server for diagnostics. Otherwise returns an empty list.
    pub async fn check_with_lsp(&self, file: &str) -> anyhow::Result<Vec<TypeError>> {
        #[cfg(feature = "lsp-bridge")]
        {
            if let Some(lsp) = &self.lsp_manager {
                let diags = lsp.get_diagnostics(file).await.map_err(|e| {
                    anyhow::anyhow!("LSP diagnostics failed: {}", e)
                })?;

                let errors = diags.into_iter()
                    .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
                    .map(|d| TypeError::from_lsp_diagnostic(d, file))
                    .collect();
                return Ok(errors);
            }
        }

        // No LSP manager configured or feature disabled
        Ok(Vec::new())
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
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

impl TypeError {
    /// Convert an LSP Diagnostic into a TypeError.
    #[cfg(feature = "lsp-bridge")]
    fn from_lsp_diagnostic(diag: lsp_types::Diagnostic, file: &str) -> Self {
        Self {
            file: file.to_string(),
            line: diag.range.start.line,
            message: diag.message,
            error_code: diag.code.map(|c| match c {
                lsp_types::NumberOrString::Number(n) => n.to_string(),
                lsp_types::NumberOrString::String(s) => s,
            }).unwrap_or_default(),
        }
    }
}
