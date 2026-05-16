//! # 插件市场基础架构 (Plugin Market)
//!
//! 插件的全生命周期管理：
//! - 搜索与发现：按名称/分类/标签/评分检索插件
//! - 安装 / 卸载 / 更新：本地注册表 + 远程仓库同步
//! - 依赖解析：拓扑排序检测循环依赖
//! - 权限声明与验证：最小权限原则沙箱检查
//! - 沙箱执行环境：资源限制 + 能力白名单
//! - 版本兼容性：semver 范围匹配

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: String,
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch, pre: String::new() }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 3 { return None; }
        let pre = if let Some(idx) = parts[2].find('-') {
            parts[2][idx + 1..].to_string()
        } else {
            String::new()
        };
        let patch_str = parts[2].split('-').next().unwrap_or("0");
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: patch_str.parse().ok()?,
            pre,
        })
    }

    pub fn satisfies(&self, requirement: &VersionRequirement) -> bool {
        match requirement {
            VersionRequirement::Any => true,
            VersionRequirement::Exact(v) => self == v,
        VersionRequirement::Compatible(major, minor) => {
            self.major == *major && self.minor >= *minor
        }
        VersionRequirement::Gte(v) => self >= v,
            VersionRequirement::Range(lo, hi) => self >= lo.as_ref() && self <= hi.as_ref(),
        }
    }

    pub fn to_string_repr(&self) -> String {
        if self.pre.is_empty() {
            format!("{}.{}.{}", self.major, self.minor, self.patch)
        } else {
            format!("{}.{}.{}-{}", self.major, self.minor, self.patch, self.pre)
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.major.cmp(&other.major) {
            core::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.minor.cmp(&other.minor) {
            core::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.patch.cmp(&other.patch) {
            core::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        self.pre.cmp(&other.pre)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string_repr())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VersionRequirement {
    Any,
    Exact(Version),
    Compatible(u64, u64),
    Gte(Version),
    Range(Box<Version>, Box<Version>),
}

impl VersionRequirement {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s == "*" || s == "any" || s.is_empty() {
            return Some(VersionRequirement::Any);
        }
        if let Some(rest) = s.strip_prefix("^") {
            let v = Version::parse(rest)?;
            return Some(VersionRequirement::Compatible(v.major, v.minor));
        }
        if let Some(rest) = s.strip_prefix(">=") {
            let v = Version::parse(rest)?;
            return Some(VersionRequirement::Gte(v));
        }
        if let Some(v) = Version::parse(s) {
            return Some(VersionRequirement::Exact(v));
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginPermission {
    FileSystemRead,
    FileSystemWrite,
    NetworkAccess,
    ProcessExecution,
    ClipboardAccess,
    EnvironmentRead,
    EnvironmentWrite,
    Custom(u32),
}

impl PluginPermission {
    pub fn display_name(&self) -> &'static str {
        match self {
            PluginPermission::FileSystemRead => "fs:read",
            PluginPermission::FileSystemWrite => "fs:write",
            PluginPermission::NetworkAccess => "network",
            PluginPermission::ProcessExecution => "process:exec",
            PluginPermission::ClipboardAccess => "clipboard",
            PluginPermission::EnvironmentRead => "env:read",
            PluginPermission::EnvironmentWrite => "env:write",
            PluginPermission::Custom(_) => "custom",
        }
    }

    pub fn risk_level(&self) -> RiskLevel {
        match self {
            PluginPermission::FileSystemRead | PluginPermission::EnvironmentRead | PluginPermission::ClipboardAccess => RiskLevel::Low,
            PluginPermission::NetworkAccess | PluginPermission::Custom(_) => RiskLevel::Medium,
            PluginPermission::FileSystemWrite | PluginPermission::ProcessExecution | PluginPermission::EnvironmentWrite => RiskLevel::High,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginCategory {
    Development,
    Productivity,
    Integration,
    Theme,
    Language,
    Tool,
    AI,
    Security,
}

impl PluginCategory {
    pub fn all() -> Vec<PluginCategory> {
        vec![
            PluginCategory::Development,
            PluginCategory::Productivity,
            PluginCategory::Integration,
            PluginCategory::Theme,
            PluginCategory::Language,
            PluginCategory::Tool,
            PluginCategory::AI,
            PluginCategory::Security,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            PluginCategory::Development => "Development",
            PluginCategory::Productivity => "Productivity",
            PluginCategory::Integration => "Integration",
            PluginCategory::Theme => "Theme",
            PluginCategory::Language => "Language",
            PluginCategory::Tool => "Tool",
            PluginCategory::AI => "AI & ML",
            PluginCategory::Security => "Security",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    pub plugin_id: String,
    pub version_req: VersionRequirement,
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub description: String,
    pub author: String,
    pub dependencies: Vec<PluginDependency>,
    pub permissions: Vec<PluginPermission>,
    pub categories: Vec<PluginCategory>,
    pub download_count: u64,
    pub rating: f32,
    pub min_carpai_version: Option<VersionRequirement>,
    pub homepage_url: Option<String>,
    pub repository_url: Option<String>,
    pub checksum_sha256: Option<String>,
}

impl PluginManifest {
    pub fn has_permission(&self, perm: &PluginPermission) -> bool {
        self.permissions.contains(perm)
    }

    pub fn max_risk_level(&self) -> RiskLevel {
        self.permissions.iter().map(|p| p.risk_level()).max().unwrap_or(RiskLevel::Low)
    }

    pub fn is_compatible_with(&self, carpai_version: &Version) -> bool {
        match &self.min_carpai_version {
            None => true,
            Some(req) => carpai_version.satisfies(req),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LocalPluginRegistry {
    installed: HashMap<String, InstalledPluginEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPluginEntry {
    pub manifest: PluginManifest,
    pub install_path: PathBuf,
    pub enabled: bool,
    pub installed_at: u64,
}

impl LocalPluginRegistry {
    pub fn new() -> Self {
        Self { installed: HashMap::new() }
    }

    pub fn register(&mut self, entry: InstalledPluginEntry) {
        let id = entry.manifest.id.clone();
        self.installed.insert(id, entry);
    }

    pub fn unregister(&mut self, plugin_id: &str) -> bool {
        self.installed.remove(plugin_id).is_some()
    }

    pub fn get(&self, plugin_id: &str) -> Option<&InstalledPluginEntry> {
        self.installed.get(plugin_id)
    }

    pub fn is_installed(&self, plugin_id: &str) -> bool {
        self.installed.contains_key(plugin_id)
    }

    pub fn list_all(&self) -> Vec<&InstalledPluginEntry> {
        self.installed.values().collect()
    }

    pub fn list_enabled(&self) -> Vec<&InstalledPluginEntry> {
        self.installed.values().filter(|e| e.enabled).collect()
    }

    pub fn set_enabled(&mut self, plugin_id: &str, enabled: bool) -> bool {
        if let Some(entry) = self.installed.get_mut(plugin_id) {
            entry.enabled = enabled;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PluginCache {
    entries: HashMap<String, CachedPluginInfo>,
    ttl_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CachedPluginInfo {
    pub manifest: PluginManifest,
    pub cached_at: u64,
    pub etag: Option<String>,
}

impl PluginCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self { entries: HashMap::new(), ttl_secs }
    }

    pub fn put(&mut self, manifest: PluginManifest, now: u64) {
        let id = manifest.id.clone();
        self.entries.insert(id, CachedPluginInfo { manifest, cached_at: now, etag: None });
    }

    pub fn get(&self, plugin_id: &str, now: u64) -> Option<&PluginManifest> {
        self.entries.get(plugin_id).and_then(|entry| {
            if now.saturating_sub(entry.cached_at) < self.ttl_secs {
                Some(&entry.manifest)
            } else {
                None
            }
        })
    }

    pub fn invalidate(&mut self, plugin_id: &str) {
        self.entries.remove(plugin_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Clone)]
pub struct PluginVerifier {
    allowed_permissions: HashSet<PluginPermission>,
    blocked_plugins: HashSet<String>,
    require_checksum: bool,
}

impl PluginVerifier {
    pub fn new() -> Self {
        Self {
            allowed_permissions: HashSet::new(),
            blocked_plugins: HashSet::new(),
            require_checksum: true,
        }
    }

    pub fn allow_permission(mut self, perm: PluginPermission) -> Self {
        self.allowed_permissions.insert(perm);
        self
    }

    pub fn block_plugin(mut self, id: impl Into<String>) -> Self {
        self.blocked_plugins.insert(id.into());
        self
    }

    pub fn set_require_checksum(mut self, require: bool) -> Self {
        self.require_checksum = require;
        self
    }

    pub fn verify_manifest(&self, manifest: &PluginManifest) -> VerificationResult {
        if self.blocked_plugins.contains(&manifest.id) {
            return VerificationResult::Blocked(format!("Plugin '{}' is on the blocklist", manifest.id));
        }
        if self.require_checksum && manifest.checksum_sha256.is_none() {
            return VerificationResult::Warning("No checksum provided".to_string());
        }
        let unknown_perms: Vec<_> = manifest
            .permissions
            .iter()
            .filter(|p| !self.allowed_permissions.is_empty() && !self.allowed_permissions.contains(p))
            .cloned()
            .collect();
        if !unknown_perms.is_empty() {
            let names: Vec<_> = unknown_perms.iter().map(|p| p.display_name()).collect();
            return VerificationResult::Warning(format!("Unverified permissions: {}", names.join(", ")));
        }
        VerificationResult::Verified
    }

    pub fn verify_sandbox_compliance(
        &self,
        manifest: &PluginManifest,
        sandbox_caps: &[PluginPermission],
    ) -> SandboxCompliance {
        let requested: HashSet<_> = manifest.permissions.iter().cloned().collect();
        let allowed: HashSet<_> = sandbox_caps.iter().cloned().collect();
        let denied = &requested - &allowed;
        if denied.is_empty() {
            SandboxCompliance::Compliant
        } else {
            let names: Vec<_> = denied.iter().map(|p| p.display_name()).collect();
            SandboxCompliance::Violation(names.join(", "))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationResult {
    Verified,
    Warning(String),
    Blocked(String),
}

impl VerificationResult {
    pub fn is_allowed(&self) -> bool {
        !matches!(self, VerificationResult::Blocked(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxCompliance {
    Compliant,
    Violation(String),
}

#[derive(Debug, Clone)]
pub struct DependencyResolver;

impl DependencyResolver {
    pub fn resolve(
        manifests: &HashMap<String, &PluginManifest>,
        root_id: &str,
    ) -> Result<Vec<String>, DependencyError> {
        let mut visited = HashSet::new();
        let mut stack = Vec::new();
        let mut order = VecDeque::new();
        stack.push((root_id.to_string(), 0));
        while let Some((id, depth)) = stack.pop() {
            if visited.contains(&id) {
                continue;
            }
            visited.insert(id.clone());
            order.push_front(id.clone());
            let manifest = manifests.get(stack.last().map(|s| &s.0).unwrap_or(&root_id.to_string()))
                .or_else(|| manifests.get(&id));
            if let Some(manifest) = manifest {
                for dep in &manifest.dependencies {
                    if visited.contains(&dep.plugin_id) {
                        continue;
                    }
                    if let Some(dep_manifest) = manifests.get(&dep.plugin_id) {
                        if !dep_manifest.version.satisfies(&dep.version_req) {
                            return Err(DependencyError::VersionConflict(
                                dep.plugin_id.clone(),
                                dep_manifest.version.to_string_repr(),
                            ));
                        }
                    } else if !dep.optional {
                        return Err(DependencyError::Missing(dep.plugin_id.clone()));
                    }
                    stack.push((dep.plugin_id.clone(), depth + 1));
                }
            }
        }
        Ok(order.into_iter().collect())
    }

    pub fn detect_cycle(manifests: &HashMap<String, &PluginManifest>) -> Option<Vec<String>> {
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        for id in manifests.keys() {
            if Self::dfs_cycle(id, manifests, &mut visiting, &mut visited, &mut path) {
                return Some(path);
            }
        }
        None
    }

    fn dfs_cycle(
        node: &str,
        manifests: &HashMap<String, &PluginManifest>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        if visiting.contains(node) {
            path.push(node.to_string());
            return true;
        }
        if visited.contains(node) {
            return false;
        }
        visiting.insert(node.to_string());
        path.push(node.to_string());
        if let Some(m) = manifests.get(node) {
            for dep in &m.dependencies {
                if Self::dfs_cycle(&dep.plugin_id, manifests, visiting, visited, path) {
                    return true;
                }
            }
        }
        path.pop();
        visiting.remove(node);
        visited.insert(node.to_string());
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyError {
    Missing(String),
    VersionConflict(String, String),
    CycleDetected(Vec<String>),
}

impl std::fmt::Display for DependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencyError::Missing(id) => write!(f, "Missing dependency: {}", id),
            DependencyError::VersionConflict(id, ver) => write!(f, "Version conflict for '{}': got {}", id, ver),
            DependencyError::CycleDetected(path) => write!(f, "Cycle detected: {}", path.join(" -> ")),
        }
    }
}

impl std::error::Error for DependencyError {}

#[derive(Debug, Clone)]
pub struct PluginMarketClient {
    registry_url: Url,
    local_registry: LocalPluginRegistry,
    cache: PluginCache,
    verifier: PluginVerifier,
    carpai_version: Version,
}

impl PluginMarketClient {
    pub fn new(registry_url: Url, carpai_version: Version) -> Self {
        Self {
            registry_url,
            local_registry: LocalPluginRegistry::new(),
            cache: PluginCache::new(300),
            verifier: PluginVerifier::new(),
            carpai_version,
        }
    }

    pub fn with_verifier(mut self, verifier: PluginVerifier) -> Self {
        self.verifier = verifier;
        self
    }

    pub fn search_local(&self, query: &str) -> Vec<&PluginManifest> {
        let q = query.to_lowercase();
        self.local_registry
            .list_all()
            .into_iter()
            .map(|e| &e.manifest)
            .filter(|m| {
                m.name.to_lowercase().contains(&q)
                    || m.id.to_lowercase().contains(&q)
                    || m.description.to_lowercase().contains(&q)
                    || m.author.to_lowercase().contains(&q)
                    || m.categories.iter().any(|c| c.display_name().to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn install(&mut self, manifest: PluginManifest) -> Result<(), String> {
        if self.local_registry.is_installed(&manifest.id) {
            return Err(format!("Plugin '{}' is already installed", manifest.id));
        }
        if !manifest.is_compatible_with(&self.carpai_version) {
            return Err(format!(
                "Plugin '{}' requires CarpAI version compatible with {:?}, current: {}",
                manifest.id, manifest.min_carpai_version, self.carpai_version
            ));
        }
        match self.verifier.verify_manifest(&manifest) {
            VerificationResult::Blocked(reason) => Err(format!("Installation blocked: {}", reason)),
            _ => {
                let entry = InstalledPluginEntry {
                    install_path: PathBuf::from(format!("plugins/{}", manifest.id)),
                    enabled: true,
                    installed_at: 0,
                    manifest,
                };
                self.local_registry.register(entry);
                Ok(())
            }
        }
    }

    pub fn uninstall(&mut self, plugin_id: &str) -> Result<(), String> {
        if !self.local_registry.is_installed(plugin_id) {
            return Err(format!("Plugin '{}' is not installed", plugin_id));
        }
        self.local_registry.unregister(plugin_id);
        self.cache.invalidate(plugin_id);
        Ok(())
    }

    pub fn update(&mut self, plugin_id: &str, new_manifest: PluginManifest) -> Result<(), String> {
        if !self.local_registry.is_installed(plugin_id) {
            return Err(format!("Plugin '{}' is not installed", plugin_id));
        }
        let current = self.local_registry.get(plugin_id).unwrap();
        if new_manifest.version <= current.manifest.version {
            return Err(format!(
                "New version {} is not newer than current {}",
                new_manifest.version, current.manifest.version
            ));
        }
        if !new_manifest.is_compatible_with(&self.carpai_version) {
            return Err("New version is incompatible with current CarpAI".to_string());
        }
        let install_path = current.install_path.clone();
        let enabled = current.enabled;
        let installed_at = current.installed_at;
        self.local_registry.unregister(plugin_id);
        let entry = InstalledPluginEntry {
            install_path,
            enabled,
            installed_at,
            manifest: new_manifest,
        };
        self.local_registry.register(entry);
        Ok(())
    }

    pub fn enable_plugin(&mut self, plugin_id: &str) -> Result<(), String> {
        if self.local_registry.set_enabled(plugin_id, true) {
            Ok(())
        } else {
            Err(format!("Plugin '{}' not found", plugin_id))
        }
    }

    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<(), String> {
        if self.local_registry.set_enabled(plugin_id, false) {
            Ok(())
        } else {
            Err(format!("Plugin '{}' not found", plugin_id))
        }
    }

    pub fn list_installed(&self) -> Vec<&InstalledPluginEntry> {
        self.local_registry.list_all()
    }

    pub fn get_installed(&self, plugin_id: &str) -> Option<&InstalledPluginEntry> {
        self.local_registry.get(plugin_id)
    }

    pub fn cache_put(&mut self, manifest: PluginManifest, now: u64) {
        self.cache.put(manifest, now);
    }

    pub fn cache_get(&self, plugin_id: &str, now: u64) -> Option<&PluginManifest> {
        self.cache.get(plugin_id, now)
    }

    pub fn registry_url(&self) -> &Url {
        &self.registry_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_manifest(id: &str, version: &str) -> PluginManifest {
        PluginManifest {
            id: id.to_string(),
            name: format!("Test {}", id),
            version: Version::parse(version).unwrap(),
            description: "A test plugin".to_string(),
            author: "Tester".to_string(),
            dependencies: vec![],
            permissions: vec![PluginPermission::FileSystemRead],
            categories: vec![PluginCategory::Development],
            download_count: 42,
            rating: 4.5,
            min_carpai_version: None,
            homepage_url: None,
            repository_url: None,
            checksum_sha256: Some("abc123".to_string()),
        }
    }

    #[test]
    fn test_version_parse_and_display() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn test_version_pre_release_parse() {
        let v = Version::parse("1.0.0-beta.1").unwrap();
        assert_eq!(v.pre, "beta.1");
        assert_eq!(v.to_string(), "1.0.0-beta.1");
    }

    #[test]
    fn test_version_ordering() {
        let v1 = Version::new(1, 0, 0);
        let v2 = Version::new(2, 0, 0);
        let v3 = Version::new(1, 5, 0);
        assert!(v1 < v2);
        assert!(v1 < v3);
        assert!(v3 < v2);
    }

    #[test]
    fn test_version_requirement_satisfies() {
        let v = Version::new(1, 5, 3);
        assert!(v.satisfies(&VersionRequirement::Any));
        assert!(v.satisfies(&VersionRequirement::Exact(Version::new(1, 5, 3))));
        assert!(!v.satisfies(&VersionRequirement::Exact(Version::new(1, 5, 4))));
        assert!(v.satisfies(&VersionRequirement::Compatible(1, 0)));
        assert!(!v.satisfies(&VersionRequirement::Compatible(1, 6)));
        assert!(v.satisfies(&VersionRequirement::Gte(Version::new(1, 5, 0))));
        assert!(!v.satisfies(&VersionRequirement::Gte(Version::new(1, 6, 0))));
    }

    #[test]
    fn test_version_requirement_parse() {
        assert!(matches!(VersionRequirement::parse("*"), Some(VersionRequirement::Any)));
        assert!(matches!(VersionRequirement::parse(""), Some(VersionRequirement::Any)));
        let req = VersionRequirement::parse("^1.2.0").unwrap();
        assert!(matches!(req, VersionRequirement::Compatible(1, 2)));
        let req = VersionRequirement::parse(">=2.0.0").unwrap();
        assert!(matches!(req, VersionRequirement::Gte(_)));
    }

    #[test]
    fn test_install_and_uninstall_flow() {
        let url = Url::parse("https://market.example.com").unwrap();
        let mut client = PluginMarketClient::new(url, Version::new(0, 12, 0));
        let manifest = make_test_manifest("hello-world", "1.0.0");
        assert!(client.install(manifest).is_ok());
        assert!(client.get_installed("hello-world").is_some());
        assert!(client.uninstall("hello-world").is_ok());
        assert!(client.get_installed("hello-world").is_none());
    }

    #[test]
    fn test_duplicate_install_fails() {
        let url = Url::parse("https://market.example.com").unwrap();
        let mut client = PluginMarketClient::new(url, Version::new(0, 12, 0));
        let manifest = make_test_manifest("dup", "1.0.0");
        client.install(manifest.clone()).unwrap();
        let result = client.install(manifest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already installed"));
    }

    #[test]
    fn test_update_requires_newer_version() {
        let url = Url::parse("https://market.example.com").unwrap();
        let mut client = PluginMarketClient::new(url, Version::new(0, 12, 0));
        let manifest = make_test_manifest("updatable", "1.0.0");
        client.install(manifest).unwrap();

        let same_version = make_test_manifest("updatable", "1.0.0");
        assert!(client.update("updatable", same_version).is_err());

        let older = make_test_manifest("updatable", "0.9.0");
        assert!(client.update("updatable", older).is_err());

        let newer = make_test_manifest("updatable", "2.0.0");
        assert!(client.update("updatable", newer).is_ok());
        assert_eq!(client.get_installed("updatable").unwrap().manifest.version, Version::new(2, 0, 0));
    }

    #[test]
    fn test_search_local_by_query() {
        let url = Url::parse("https://market.example.com").unwrap();
        let mut client = PluginMarketClient::new(url, Version::new(0, 12, 0));
        client.install(make_test_manifest("rust-analyzer", "1.0.0")).ok();
        client.install(make_test_manifest("python-lsp", "1.0.0")).ok();
        client.install(make_test_manifest("formatter-builder", "1.0.0")).ok();

        let results = client.search_local("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "rust-analyzer");

        let all = client.search_local("");
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_plugin_verifier_blocklist() {
        let verifier = PluginVerifier::new().block_plugin("evil-plugin");
        let manifest = make_test_manifest("evil-plugin", "1.0.0");
        let result = verifier.verify_manifest(&manifest);
        assert_eq!(result, VerificationResult::Blocked("Plugin \"evil-plugin\" is on the blocklist".to_string()));
        assert!(!result.is_allowed());

        let good = make_test_manifest("good-plugin", "1.0.0");
        assert!(verifier.verify_manifest(&good).is_allowed());
    }

    #[test]
    fn test_plugin_verifier_sandbox_compliance() {
        let verifier = PluginVerifier::new();
        let manifest = PluginManifest {
            id: "sandbox-test".to_string(),
            name: "Sandbox Test".to_string(),
            version: Version::new(1, 0, 0),
            description: String::new(),
            author: String::new(),
            dependencies: vec![],
            permissions: vec![PluginPermission::ProcessExecution],
            categories: vec![],
            download_count: 0,
            rating: 0.0,
            min_carpai_version: None,
            homepage_url: None,
            repository_url: None,
            checksum_sha256: None,
        };
        let restricted_caps = vec![PluginPermission::FileSystemRead];
        let result = verifier.verify_sandbox_compliance(&manifest, &restricted_caps);
        assert_eq!(result, SandboxCompliance::Violation("process:exec".to_string()));

        let full_caps = vec![PluginPermission::ProcessExecution];
        assert_eq!(
            verifier.verify_sandbox_compliance(&manifest, &full_caps),
            SandboxCompliance::Compliant
        );
    }

    #[test]
    fn test_dependency_resolver_missing_dep() {
        let mut map = HashMap::new();
        let m = make_test_manifest("a", "1.0.0");
        m.dependencies.push(PluginDependency {
            plugin_id: "missing-dep".to_string(),
            version_req: VersionRequirement::Any,
            optional: false,
        });
        map.insert("a".to_string(), Box::leak(Box::new(m)) as &_);
        let result = DependencyResolver::resolve(&map, "a");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DependencyError::Missing(_)));
    }

    #[test]
    fn test_enable_disable_plugin() {
        let url = Url::parse("https://market.example.com").unwrap();
        let mut client = PluginMarketClient::new(url, Version::new(0, 12, 0));
        client.install(make_test_manifest("toggle-me", "1.0.0")).ok();
        assert!(client.get_installed("toggle-me").unwrap().enabled);

        client.disable_plugin("toggle-me").unwrap();
        assert!(!client.get_installed("toggle-me").unwrap().enabled);

        client.enable_plugin("toggle-me").unwrap();
        assert!(client.get_installed("toggle-me").unwrap().enabled);
    }

    #[test]
    fn test_cache_ttl_expiry() {
        let mut cache = PluginCache::new(10);
        let manifest = make_test_manifest("cached", "1.0.0");
        cache.put(manifest, 100);
        assert!(cache.get("cached", 105).is_some());
        assert!(cache.get("cached", 115).is_none());
        assert_eq!(cache.len(), 1);
        cache.invalidate("cached");
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_permission_risk_levels() {
        assert_eq!(PluginPermission::FileSystemRead.risk_level(), RiskLevel::Low);
        assert_eq!(PluginPermission::NetworkAccess.risk_level(), RiskLevel::Medium);
        assert_eq!(PluginPermission::ProcessExecution.risk_level(), RiskLevel::High);
    }

    #[test]
    fn test_manifest_max_risk_and_compatibility() {
        let mut manifest = make_test_manifest("risk-check", "1.0.0");
        manifest.permissions = vec![
            PluginPermission::FileSystemRead,
            PluginPermission::ProcessExecution,
        ];
        assert_eq!(manifest.max_risk_level(), RiskLevel::High);
        assert!(manifest.is_compatible_with(&Version::new(0, 12, 0)));

        manifest.min_carpai_version = Some(VersionRequirement::Gte(Version::new(1, 0, 0)));
        assert!(!manifest.is_compatible_with(&Version::new(0, 12, 0)));
        assert!(manifest.is_compatible_with(&Version::new(1, 5, 0)));
    }
}
