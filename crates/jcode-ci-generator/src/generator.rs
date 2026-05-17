use crate::stack_detector::TechStack;
use crate::templates::{CiTemplate, Platform};
use std::collections::HashMap;

/// 生成的配置文件 (文件名 -> 内容)
#[derive(Debug, Clone)]
pub struct GeneratedConfig {
    pub files: HashMap<String, String>,
}

/// CI 配置生成器
pub struct CiGenerator;

impl Default for CiGenerator {
    fn default() -> Self {
        Self
    }
}

impl CiGenerator {
    pub fn new() -> Self { Self }

    /// 为所有支持的平台生成配置
    pub async fn generate_all(&self, stack: &TechStack) -> anyhow::Result<HashMap<String, String>> {
        let mut configs = HashMap::new();

        // GitLab CI
        configs.insert(
            Platform::GitLabCi.filename().to_string(),
            CiTemplate::gitlab_ci(stack),
        );

        // GitHub Actions
        configs.insert(
            Platform::GitHubActions.filename().to_string(),
            CiTemplate::github_actions(stack),
        );

        // Jenkins
        configs.insert(
            Platform::Jenkins.filename().to_string(),
            CiTemplate::jenkinsfile(stack),
        );

        Ok(configs)
    }

    /// 将生成的配置写入项目目录
    pub async fn write_to_project(&self, configs: &HashMap<String, String>, root: &str) -> anyhow::Result<()> {
        for (path, content) in configs {
            let full_path = std::path::Path::new(root).join(path);
            if let Some(parent) = full_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&full_path, content).await?;
            tracing::info!("Generated: {}", full_path.display());
        }
        Ok(())
    }
}
