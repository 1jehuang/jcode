//! # 环境管理器 (EnvironmentManager)

use crate::types::*;
use parking_lot::Mutex;
use std::collections::HashMap;

pub struct EnvironmentManager {
    environments: Mutex<HashMap<String, EnvironmentSpec>>,
    containers: Mutex<HashMap<String, ContainerConfig>>,
}

impl EnvironmentManager {
    pub fn new() -> Self {
        let mgr = Self {
            environments: Mutex::new(HashMap::new()),
            containers: Mutex::new(HashMap::new()),
        };
        mgr.init_default_environments();
        mgr
    }

    fn init_default_environments(&self) {
        let mut envs = self.environments.lock();
        let mut push = |name: &str, spec: EnvironmentSpec| { envs.insert(name.to_string(), spec); };

        push("rust-build", EnvironmentSpec {
            env_id: uuid::Uuid::new_v4(), name: "Rust Build".to_string(), os_image: "rust:1.78".to_string(),
            toolchains: vec![ToolchainSpec {
                language: ProgrammingLanguage::Rust, version: "1.78".to_string(), image: "rust:1.78".to_string(),
                install_path: "/usr/local/cargo".to_string(), binary_path: "/usr/local/cargo/bin/cargo".to_string(),
                verify_command: "cargo --version".to_string(), pre_pulled: false,
            }],
            environment_variables: HashMap::new(),
            default_resources: ResourceLimits { cpu_cores: Some(4.0), memory_mb: Some(8192), disk_mb: Some(40960), ..Default::default() },
        });
        push("go-build", EnvironmentSpec {
            env_id: uuid::Uuid::new_v4(), name: "Go Build".to_string(), os_image: "golang:1.22".to_string(),
            toolchains: vec![ToolchainSpec {
                language: ProgrammingLanguage::Go, version: "1.22".to_string(), image: "golang:1.22".to_string(),
                install_path: "/usr/local/go".to_string(), binary_path: "/usr/local/go/bin/go".to_string(),
                verify_command: "go version".to_string(), pre_pulled: false,
            }],
            environment_variables: HashMap::new(),
            default_resources: ResourceLimits { cpu_cores: Some(4.0), memory_mb: Some(4096), disk_mb: Some(20480), ..Default::default() },
        });
        push("node-build", EnvironmentSpec {
            env_id: uuid::Uuid::new_v4(), name: "Node Build".to_string(), os_image: "node:20".to_string(),
            toolchains: vec![ToolchainSpec {
                language: ProgrammingLanguage::JavaScript, version: "20".to_string(), image: "node:20".to_string(),
                install_path: "/usr/local".to_string(), binary_path: "/usr/local/bin/npm".to_string(),
                verify_command: "node --version && npm --version".to_string(), pre_pulled: false,
            }],
            environment_variables: HashMap::new(),
            default_resources: ResourceLimits { cpu_cores: Some(2.0), memory_mb: Some(2048), disk_mb: Some(10240), ..Default::default() },
        });
        push("python-build", EnvironmentSpec {
            env_id: uuid::Uuid::new_v4(), name: "Python Build".to_string(), os_image: "python:3.12".to_string(),
            toolchains: vec![ToolchainSpec {
                language: ProgrammingLanguage::Python, version: "3.12".to_string(), image: "python:3.12".to_string(),
                install_path: "/usr/local".to_string(), binary_path: "/usr/local/bin/python3".to_string(),
                verify_command: "python3 --version".to_string(), pre_pulled: false,
            }],
            environment_variables: HashMap::new(),
            default_resources: ResourceLimits { cpu_cores: Some(2.0), memory_mb: Some(2048), disk_mb: Some(5120), ..Default::default() },
        });
    }

    pub fn register_environment(&self, env: EnvironmentSpec) {
        self.environments.lock().insert(env.name.clone(), env);
    }
    pub fn get_environment(&self, name: &str) -> Option<EnvironmentSpec> {
        self.environments.lock().get(name).cloned()
    }
    pub fn recommend_environment(&self, language: ProgrammingLanguage) -> Option<EnvironmentSpec> {
        let name = match language {
            ProgrammingLanguage::Rust => "rust-build",
            ProgrammingLanguage::Go => "go-build",
            ProgrammingLanguage::JavaScript | ProgrammingLanguage::TypeScript => "node-build",
            ProgrammingLanguage::Python => "python-build",
            ProgrammingLanguage::Java => "java-build",
            ProgrammingLanguage::Cpp | ProgrammingLanguage::C => "cpp-build",
            _ => return None,
        };
        self.get_environment(name)
    }
    pub fn create_container_config(&self, language: ProgrammingLanguage, custom_image: Option<String>, resource_limits: ResourceLimits, work_dir: &str) -> ContainerConfig {
        let image = custom_image.or_else(|| self.recommend_environment(language).map(|e| e.os_image)).unwrap_or_else(|| "ubuntu:22.04".to_string());
        ContainerConfig {
            container_id: None, image, limits: resource_limits,
            volume_mounts: vec![VolumeMount { source: work_dir.to_string(), destination: "/workspace".to_string(), read_only: false }],
            network_mode: "bridge".to_string(), privileged: false, timeout_secs: 600, extra_env: HashMap::new(),
        }
    }
    pub fn register_container(&self, container_id: String, config: ContainerConfig) {
        self.containers.lock().insert(container_id, config);
    }
    pub fn remove_container(&self, container_id: &str) {
        self.containers.lock().remove(container_id);
    }
    pub fn active_container_count(&self) -> usize { self.containers.lock().len() }
}

impl Default for EnvironmentManager { fn default() -> Self { Self::new() } }
