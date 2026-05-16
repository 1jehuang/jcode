//! # Deploy Skill — 将构建产物部署到目标环境
//!
//! 支持多种部署目标:
//! 1. SSH 远程部署 (rsync/scp)
//! 2. Docker 镜像部署
//! 3. Kubernetes 部署 (apply/helm)
//! 4. 本地目录部署
//!
//! ## 修复说明
//! - 修复 deploy_docker 中的 `Vec<&str>` 可变借用生命周期问题
//! - Kubernetes 部署支持目录/文件/helm 三种模式
//! - 增强错误处理与前置检查

use crate::skill::{Skill, SkillDef, SkillInput, SkillOutput, SkillStatus};
use async_trait::async_trait;
use std::collections::HashMap;

/// 部署目标类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeployTarget {
    Local,
    Ssh,
    Docker,
    Kubernetes,
}

impl DeployTarget {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "ssh" => DeployTarget::Ssh,
            "docker" => DeployTarget::Docker,
            "kubernetes" | "k8s" => DeployTarget::Kubernetes,
            _ => DeployTarget::Local,
        }
    }
}

/// Kubernetes 部署模式
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum K8sDeployMode {
    /// kubectl apply -f <path> (单个文件或目录)
    #[default]
    Apply,
    /// helm upgrade --install
    Helm,
}

impl K8sDeployMode {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "helm" => K8sDeployMode::Helm,
            _ => K8sDeployMode::Apply,
        }
    }
}

/// 部署技能
pub struct DeploySkill;

impl DeploySkill {
    /// 执行本地部署
    async fn deploy_local(artifact_path: &str, target_dir: &str) -> anyhow::Result<String> {
        let status = tokio::process::Command::new("cp")
            .args(["-r", artifact_path, target_dir])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("Local deploy failed: {}", e))?;
        if !status.success() {
            anyhow::bail!("cp exited with status: {}", status);
        }
        Ok(format!("Deployed {} -> {}", artifact_path, target_dir))
    }

    /// 执行 SSH 远程部署
    async fn deploy_ssh(
        artifact_path: &str,
        host: &str,
        remote_path: &str,
        port: u16,
    ) -> anyhow::Result<String> {
        // Use rsync over SSH for efficient incremental deployment
        let status = tokio::process::Command::new("rsync")
            .args([
                "-avz",
                "--delete",
                "-e",
                &format!("ssh -p {}", port),
                artifact_path,
                &format!("{}:{}", host, remote_path),
            ])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("SSH deploy failed: {}", e))?;

        if !status.success() {
            anyhow::bail!("rsync exited with status: {}", status);
        }
        Ok(format!("Deployed {} -> {}:{} (port {})", artifact_path, host, remote_path, port))
    }

    /// 执行 Docker 部署
    async fn deploy_docker(
        image_name: &str,
        container_name: &str,
        ports: &[String],
    ) -> anyhow::Result<String> {
        // Pull latest image
        let pull_status = tokio::process::Command::new("docker")
            .args(["pull", image_name])
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("Docker pull failed: {}", e))?;
        if !pull_status.success() {
            anyhow::bail!("Docker pull failed: image '{}' not found or unavailable", image_name);
        }

        // Stop & remove existing container (best-effort)
        let _ = tokio::process::Command::new("docker")
            .args(["stop", container_name])
            .status()
            .await;
        let _ = tokio::process::Command::new("docker")
            .args(["rm", container_name])
            .status()
            .await;

        // Build docker run args using owned Strings
        let mut args: Vec<String> = vec!["run", "-d", "--name", container_name, "--restart=unless-stopped"]
            .into_iter().map(String::from).collect();
        for port_mapping in ports {
            args.push("-p".to_string());
            args.push(port_mapping.clone());
        }
        args.push(image_name.to_string());

        let run_status = tokio::process::Command::new("docker")
            .args(&args)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("Docker run failed: {}", e))?;

        if !run_status.success() {
            anyhow::bail!("Docker run exited with status: {}", run_status);
        }

        Ok(format!("Docker container '{}' started from '{}'", container_name, image_name))
    }

    /// Kubernetes 部署（支持 apply/helm 两种模式）
    async fn deploy_kubernetes(
        manifest_path: &str,
        kube_config: &KuDeployConfig,
    ) -> anyhow::Result<String> {
        match kube_config.mode {
            K8sDeployMode::Helm => {
                let release_name = kube_config.helm_release_name.as_deref()
                    .unwrap_or("my-release");
                let mut args: Vec<String> = vec![
                    "upgrade".to_string(),
                    "--install".to_string(),
                    release_name.to_string(),
                    manifest_path.to_string(),
                ];
                if let Some(ref ns) = kube_config.namespace {
                    args.push("--namespace".to_string());
                    args.push(ns.clone());
                }
                let status = tokio::process::Command::new("helm")
                    .args(&args)
                    .status()
                    .await
                    .map_err(|e| anyhow::anyhow!("Helm deploy failed: {}", e))?;
                if !status.success() {
                    anyhow::bail!("helm upgrade exited with status: {}", status);
                }
                Ok(format!("Helm release '{}' deployed from {}", release_name, manifest_path))
            }
            K8sDeployMode::Apply => {
                let mut args: Vec<String> = vec!["apply".to_string(), "-f".to_string(), manifest_path.to_string()];
                if let Some(ref ns) = kube_config.namespace {
                    args.push("--namespace".to_string());
                    args.push(ns.clone());
                }
                let status = tokio::process::Command::new("kubectl")
                    .args(&args)
                    .status()
                    .await
                    .map_err(|e| anyhow::anyhow!("Kubectl apply failed: {}", e))?;
                if !status.success() {
                    anyhow::bail!("kubectl apply exited with status: {}", status);
                }
                Ok(format!("Kubernetes manifests applied from {}", manifest_path))
            }
        }
    }

    /// 解析部署配置
    fn parse_deploy_config(params: &HashMap<String, String>) -> DeployConfig {
        DeployConfig {
            target: DeployTarget::from_str(
                params.get("deploy_target").map(|s| s.as_str()).unwrap_or("local"),
            ),
            artifact_path: params
                .get("artifact_path")
                .cloned()
                .unwrap_or_else(|| "./dist".to_string()),
            host: params.get("host").cloned().unwrap_or_default(),
            remote_path: params
                .get("remote_path")
                .cloned()
                .unwrap_or_else(|| "/opt/app".to_string()),
            port: params
                .get("port")
                .and_then(|s| s.parse().ok())
                .unwrap_or(22u16),
            docker_image: params.get("docker_image").cloned().unwrap_or_default(),
            container_name: params.get("container_name").cloned().unwrap_or_default(),
            port_mappings: params
                .get("port_mappings")
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default(),
        }
    }

    /// 获取部署前置条件检查列表
    fn prerequisites(target: DeployTarget) -> Vec<&'static str> {
        match target {
            DeployTarget::Local => vec!["cp"],
            DeployTarget::Ssh => vec!["rsync", "ssh"],
            DeployTarget::Docker => vec!["docker"],
            DeployTarget::Kubernetes => vec!["kubectl"],
        }
    }

    /// 检查前置条件是否满足
    async fn check_prerequisites(target: DeployTarget) -> anyhow::Result<()> {
        for cmd in Self::prerequisites(target) {
            let output = tokio::process::Command::new("which")
                .arg(cmd)
                .output()
                .await
                .map_err(|_| anyhow::anyhow!("Failed to check prerequisite: {}", cmd))?;
            if !output.status.success() {
                anyhow::bail!("Required command not found: {}. Please install it first.", cmd);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct DeployConfig {
    target: DeployTarget,
    artifact_path: String,
    host: String,
    remote_path: String,
    port: u16,
    docker_image: String,
    container_name: String,
    port_mappings: Vec<String>,
}

/// Kubernetes 详细配置
#[derive(Debug, Default)]
struct KuDeployConfig {
    mode: K8sDeployMode,
    namespace: Option<String>,
    helm_release_name: Option<String>,
}

impl KuDeployConfig {
    fn from_params(params: &HashMap<String, String>) -> Self {
        Self {
            mode: K8sDeployMode::from_str(
                params.get("k8s_mode").map(|s| s.as_str()).unwrap_or("apply"),
            ),
            namespace: params.get("k8s_namespace").cloned().filter(|s| !s.is_empty()),
            helm_release_name: params.get("helm_release_name").cloned().filter(|s| !s.is_empty()),
        }
    }
}

#[async_trait]
impl Skill for DeploySkill {
    fn name(&self) -> &'static str {
        "deploy"
    }

    fn description(&self) -> &'static str {
        "部署技能：将构建产物部署到本地/SSH/Docker/Kubernetes（支持 Helm）"
    }

    fn definition(&self) -> SkillDef {
        SkillDef {
            name: "deploy",
            description: self.description(),
            version: "1.1",
            required_params: &["artifact_path"],
        }
    }

    async fn execute(&self, input: SkillInput) -> anyhow::Result<SkillOutput> {
        tracing::info!("[Deploy] Starting deployment...");

        let config = Self::parse_deploy_config(&input.parameters);

        // 检查前置条件
        Self::check_prerequisites(config.target).await?;

        let message = match config.target {
            DeployTarget::Local => {
                let target_dir = input.parameters.get("target_dir").cloned()
                    .unwrap_or_else(|| "./deploy".to_string());
                Self::deploy_local(&config.artifact_path, &target_dir).await?
            }
            DeployTarget::Ssh => {
                if config.host.is_empty() {
                    anyhow::bail!("host required for SSH deploy");
                }
                Self::deploy_ssh(&config.artifact_path, &config.host, &config.remote_path, config.port).await?
            }
            DeployTarget::Docker => {
                if config.docker_image.is_empty() || config.container_name.is_empty() {
                    anyhow::bail!("docker_image and container_name required for Docker deploy");
                }
                Self::deploy_docker(&config.docker_image, &config.container_name, &config.port_mappings).await?
            }
            DeployTarget::Kubernetes => {
                let kube_cfg = KuDeployConfig::from_params(&input.parameters);
                Self::deploy_kubernetes(&config.artifact_path, &kube_cfg).await?
            }
        };

        let metrics: HashMap<String, f64> = [
            ("deploy_target".into(), config.target as usize as f64),
        ]
        .into();

        tracing::info!("[Deploy] {}", message);

        Ok(SkillOutput {
            status: SkillStatus::Success,
            message,
            artifacts: vec![config.artifact_path],
            metrics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deploy_target_from_str() {
        assert_eq!(DeployTarget::from_str("ssh"), DeployTarget::Ssh);
        assert_eq!(DeployTarget::from_str("docker"), DeployTarget::Docker);
        assert_eq!(DeployTarget::from_str("k8s"), DeployTarget::Kubernetes);
        assert_eq!(DeployTarget::from_str("local"), DeployTarget::Local);
        assert_eq!(DeployTarget::from_str("unknown"), DeployTarget::Local);
    }

    #[test]
    fn test_parse_deploy_config() {
        let mut params = HashMap::new();
        params.insert("deploy_target".into(), "ssh".into());
        params.insert("artifact_path".into(), "./build".into());
        params.insert("host".into(), "example.com".into());
        params.insert("port".into(), "2222".into());

        let config = DeploySkill::parse_deploy_config(&params);
        assert_eq!(config.target, DeployTarget::Ssh);
        assert_eq!(config.artifact_path, "./build");
        assert_eq!(config.host, "example.com");
        assert_eq!(config.port, 2222);
    }

    #[test]
    fn test_k8s_mode_from_str() {
        assert_eq!(K8sDeployMode::from_str("helm"), K8sDeployMode::Helm);
        assert_eq!(K8sDeployMode::from_str("apply"), K8sDeployMode::Apply);
        assert_eq!(K8sDeployMode::from_str("unknown"), K8sDeployMode::Apply);
    }

    #[test]
    fn test_ku_deploy_config_from_params() {
        let mut params = HashMap::new();
        params.insert("k8s_mode".into(), "helm".into());
        params.insert("k8s_namespace".into(), "prod".into());
        params.insert("helm_release_name".into(), "my-app".into());
        let cfg = KuDeployConfig::from_params(&params);
        assert_eq!(cfg.mode, K8sDeployMode::Helm);
        assert_eq!(cfg.namespace.as_deref(), Some("prod"));
        assert_eq!(cfg.helm_release_name.as_deref(), Some("my-app"));
    }

    #[test]
    fn test_k8s_config_defaults() {
        let params = HashMap::new();
        let cfg = KuDeployConfig::from_params(&params);
        assert_eq!(cfg.mode, K8sDeployMode::Apply);
        assert!(cfg.namespace.is_none());
        assert!(cfg.helm_release_name.is_none());
    }
}
