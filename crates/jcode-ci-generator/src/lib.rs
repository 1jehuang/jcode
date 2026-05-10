//! # jcode-ci-generator
//! CI 配置生成器 — 自动检测技术栈 → 生成标准 CI 配置
//!
//! ## 支持平台
//! - GitLab CI (.gitlab-ci.yml)
//! - Jenkins (Jenkinsfile)
//! - GitHub Actions (.github/workflows/*.yml)
//!
//! ## 检测流程
//! 扫描项目文件 → 识别语言/框架/构建工具 → 加载模板 → 生成配置

mod stack_detector;
mod templates;
mod generator;

pub use stack_detector::{StackDetector, TechStack, Language, Framework, BuildTool};
pub use templates::{CiTemplate, Platform};
pub use generator::{CiGenerator, GeneratedConfig};

use std::collections::HashMap;

/// 一键生成所有 CI 配置
pub async fn generate_ci(project_root: &str) -> anyhow::Result<HashMap<String, String>> {
    let detector = StackDetector::new();
    let stack = detector.detect(project_root)?;
    let gen = CiGenerator::new();
    gen.generate_all(&stack).await
}
