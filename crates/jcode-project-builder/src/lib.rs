//! # jcode-project-builder
//! Builder 模式项目脚手架 — 代码 + Dockerfile + Swagger + CI/CD 一键生成
//!
//! ## 使用方式
//! ```rust
//! let project = ProjectBuilder::new("my-api", Language::Rust)
//!     .framework(Framework::Axum)
//!     .with_docker(true)
//!     .with_swagger(true)
//!     .with_ci(true)
//!     .build("/path/to/output").await?;
//! ```

use std::collections::HashMap;

mod scaffolder;

pub use scaffolder::{ProjectScaffolder, ScaffoldConfig, GeneratedProject};

/// Builder 模式入口
pub struct ProjectBuilder {
    name: String,
    language: String,
    framework: String,
    with_docker: bool,
    with_swagger: bool,
    with_ci: bool,
    extra_files: HashMap<String, String>,
}

impl ProjectBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            language: String::new(),
            framework: String::new(),
            with_docker: false,
            with_swagger: false,
            with_ci: false,
            extra_files: HashMap::new(),
        }
    }

    pub fn language(mut self, lang: &str) -> Self { self.language = lang.to_string(); self }
    pub fn framework(mut self, fw: &str) -> Self { self.framework = fw.to_string(); self }
    pub fn with_docker(mut self, yes: bool) -> Self { self.with_docker = yes; self }
    pub fn with_swagger(mut self, yes: bool) -> Self { self.with_swagger = yes; self }
    pub fn with_ci(mut self, yes: bool) -> Self { self.with_ci = yes; self }
    pub fn add_file(mut self, path: &str, content: &str) -> Self {
        self.extra_files.insert(path.to_string(), content.to_string());
        self
    }

    /// 执行构建 — 生成完整项目结构
    pub async fn build(self, output_dir: &str) -> anyhow::Result<GeneratedProject> {
        let config = ScaffoldConfig {
            name: self.name,
            language: self.language,
            framework: self.framework,
            with_docker: self.with_docker,
            with_swagger: self.with_swagger,
            with_ci: self.with_ci,
            extra_files: self.extra_files,
        };
        let scaffolder = ProjectScaffolder::new(config);
        scaffolder.scaffold(output_dir).await
    }
}
