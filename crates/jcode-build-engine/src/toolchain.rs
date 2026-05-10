//! # 工具链管理器 (ToolchainManager)
//!
//! 负责注册/发现/验证编译工具链、管理 Docker 镜像和工具链版本

use crate::types::*;
use parking_lot::Mutex;
use std::collections::HashMap;

/// 工具链管理器
pub struct ToolchainManager {
    /// 按语言注册的工具链
    toolchains: Mutex<HashMap<ProgrammingLanguage, Vec<ToolchainSpec>>>,
    /// 默认镜像映射
    default_images: Mutex<HashMap<ProgrammingLanguage, String>>,
}

impl ToolchainManager {
    pub fn new() -> Self {
        let mgr = Self {
            toolchains: Mutex::new(HashMap::new()),
            default_images: Mutex::new(HashMap::new()),
        };
        mgr.init_defaults();
        mgr
    }

    /// 初始化默认工具链配置
    fn init_defaults(&self) {
        let mut images = self.default_images.lock();
        let defs = vec![
            (ProgrammingLanguage::Rust, "rust:1.78"),
            (ProgrammingLanguage::Go, "golang:1.22"),
            (ProgrammingLanguage::Python, "python:3.12"),
            (ProgrammingLanguage::Java, "maven:3.9-eclipse-temurin-21"),
            (ProgrammingLanguage::JavaScript, "node:20"),
            (ProgrammingLanguage::TypeScript, "node:20-typescript"),
            (ProgrammingLanguage::Cpp, "gcc:14"),
            (ProgrammingLanguage::C, "gcc:14"),
            (ProgrammingLanguage::DotNet, "mcr.microsoft.com/dotnet/sdk:8.0"),
            (ProgrammingLanguage::Swift, "swift:5.10"),
            (ProgrammingLanguage::Kotlin, "gradle:8-jdk21"),
        ];
        for (lang, image) in defs {
            images.insert(lang, image.to_string());
        }
    }

    /// 注册工具链
    pub fn register(&self, mut spec: ToolchainSpec) {
        let lang = spec.language.clone();
        let mut chains = self.toolchains.lock();
        chains.entry(lang).or_default().push(spec);
    }

    /// 获取指定语言的工具链列表
    pub fn get_toolchains(&self, language: ProgrammingLanguage) -> Vec<ToolchainSpec> {
        self.toolchains.lock().get(&language).cloned().unwrap_or_default()
    }

    /// 获取默认 Docker 镜像
    pub fn default_image(&self, language: ProgrammingLanguage) -> String {
        self.default_images.lock().get(&language).cloned().unwrap_or_else(|| "ubuntu:22.04".to_string())
    }

    /// 验证工具链可用性
    pub fn verify_toolchain(&self, language: ProgrammingLanguage) -> bool {
        let chains = self.toolchains.lock();
        let images = self.default_images.lock();
        chains.contains_key(&language) || images.contains_key(&language)
    }

    /// 获取所有支持的语言
    pub fn supported_languages(&self) -> Vec<ProgrammingLanguage> {
        let mut langs: Vec<ProgrammingLanguage> = self.default_images.lock().keys().cloned().collect();
        langs.sort_by_key(|l| format!("{:?}", l));
        langs
    }
}

impl Default for ToolchainManager {
    fn default() -> Self {
        Self::new()
    }
}
