// ════════════════════════════════════════════════════════════════
// SSRF (Server-Side Request Forgery) 防护系统
//
// 移植自 Claude Code ssrfGuard.ts + hooks/ssrfGuard.ts
//
// 所有 HTTP 请求 (WebFetch/WebSearch/MCP Streamable HTTP) 都经过此检查:
//
//   1. 内网 IP 黑名单 — 10.x, 172.16-31.x, 192.168.x, 169.254.x
//   2. Link-Local 地址 — ::1, 127.x.x.x, fe80::/10
//   3. Cloud Metadata endpoints — AWS IMDSv1/v2, GCP, Azure
//   4. IPv6 映射的 IPv4 — ::ffff:127.0.0.1
//   5. DNS rebinding 保护 — 解析后二次验证 IP
//
// 支持白名单覆盖和自定义黑名单扩展。
// ════════════════════════════════════════════════════════════════

use regex::Regex;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;
use std::time::Duration;

/// SSRF 检查结果
#[derive(Debug, Clone)]
pub struct SsrfCheckResult {
    /// 是否允许请求
    pub allowed: bool,

    /// 阻止原因 (如果不允许)
    pub block_reason: Option<String>,

    /// 被阻止的目标地址
    pub blocked_address: Option<String>,

    /// 匹配的规则名称
    pub matched_rule: Option<String>,
}

/// SSRF Guard 配置
#[derive(Debug, Clone)]
pub struct SsrfGuardConfig {
    /// 是否启用检查
    pub enabled: bool,

    /// 是否启用 DNS 解析验证 (防止 DNS rebinding)
    pub enable_dns_verification: bool,

    /// DNS 解析超时 (毫秒)
    pub dns_timeout_ms: u64,

    /// 自定义白名单 (允许访问的内网地址)
    pub whitelist: HashSet<String>,

    /// 自定义黑名单 (额外阻止的模式)
    pub blacklist: HashSet<String>,
}

impl Default for SsrfGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_dns_verification: true,
            dns_timeout_ms: 3000,
            whitelist: HashSet::new(),
            blacklist: HashSet::new(),
        }
    }
}

/// 内网 IPv4 范围描述
struct IpRange {
    start: Ipv4Addr,
    end: Ipv4Addr,
    name: &'static str,
}

/// RFC1918 私有地址 + 特殊内网范围
const PRIVATE_IPV4_RANGES: &[IpRange] = &[
    // RFC1918 私有地址
    IpRange { start: Ipv4Addr::new(10, 0, 0, 0), end: Ipv4Addr::new(10, 255, 255, 255), name: "RFC1918-10" },
    IpRange { start: Ipv4Addr::new(172, 16, 0, 0), end: Ipv4Addr::new(172, 31, 255, 255), name: "RFC1918-172" },
    IpRange { start: Ipv4Addr::new(192, 168, 0, 0), end: Ipv4Addr::new(192, 168, 255, 255), name: "RFC1918-192" },
    // Loopback
    IpRange { start: Ipv4Addr::new(127, 0, 0, 0), end: Ipv4Addr::new(127, 255, 255, 255), name: "Loopback" },
    // Link-local
    IpRange { start: Ipv4Addr::new(169, 254, 0, 0), end: Ipv4Addr::new(169, 254, 255, 255), name: "LinkLocal" },
    // 0.0.0.0 (特殊)
    IpRange { start: Ipv4Addr::new(0, 0, 0, 0), end: Ipv4Addr::new(0, 255, 255, 255), name: "ZeroNet" },
    // 共享地址空间 (100.64.0.0/10)
    IpRange { start: Ipv4Addr::new(100, 64, 0, 0), end: Ipv4Addr::new(100, 127, 255, 255), name: "SharedAddressSpace" },
    // 基准测试 (198.18.0.0/15)
    IpRange { start: Ipv4Addr::new(198, 18, 0, 0), end: Ipv4Addr::new(198, 19, 255, 255), name: "Benchmark" },
];

/// 云 Metadata endpoint 主机名
static CLOUD_METADATA_HOSTS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // AWS
        "metadata.google.internal",
        "metadata.google.internal.",
        // GCP
        "169.254.169.254",
        // Azure
        "169.254.169.254", // 同上
        "instance-data.azure.net",
        "instance-data.",
        // DigitalOcean
        "169.254.169.254",
        // VMware
        "169.254.169.254",
        // Alibaba Cloud
        "100.100.100.200",
        // Tencent Cloud
        "metadata.tencentyun.com",
        // Huawei Cloud
        "169.254.169.254",
        // IBM Cloud
        "169.254.169.254",
        // OVH
        "169.254.169.254",
        // Oracle Cloud
        "169.254.169.254",
        // Kubernetes
        "kubernetes.default.svc",
        "kubernetes.default",
    ]
    .into_iter()
    .collect()
});

/// Metadata endpoint URL 路径模式 (用于 IMDS)
static METADATA_PATH_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)/latest/(meta-data|user-data|dynamic)/").unwrap(),
        Regex::new(r"(?i)/meta-data/").unwrap(),
        Regex::new(r"(?i)/metadata/").unwrap(),
        Regex::new(r"(?i)metadata\.google\.internal").unwrap(),
        Regex::new(r"(?i)instance-data").unwrap(),
    ]
});

pub struct SsrfGuard {
    config: SsrfGuardConfig,
    custom_private_ranges: Vec<IpRange>,
}

impl Default for SsrfGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl SsrfGuard {
    pub fn new() -> Self {
        Self {
            config: SsrfGuardConfig::default(),
            custom_private_ranges: Vec::new(),
        }
    }

    pub fn with_config(config: SsrfGuardConfig) -> Self {
        Self { config, ..Default::default() }
    }

    pub fn with_whitelist(mut self, hosts: Vec<String>) -> Self {
        self.config.whitelist = hosts.into_iter().collect();
        self
    }

    pub fn with_blacklist(mut self, patterns: Vec<String>) -> Self {
        self.config.blacklist = patterns.into_iter().collect();
        self
    }

    /// 核心检查方法 — 检查 URL 是否安全可访问
    ///
    /// # Arguments
    /// * `url` - 要检查的完整 URL 字符串
    ///
    /// # Returns
    /// `SsrfCheckResult` 表示是否允许该请求
    pub async fn check_url(&self, url: &str) -> SsrfCheckResult {
        if !self.config.enabled {
            return SsrfCheckResult {
                allowed: true,
                block_reason: None,
                blocked_address: None,
                matched_rule: None,
            };
        }

        // 1. 白名单检查 (优先于所有其他规则)
        if self.is_whitelisted(url) {
            return SsrfCheckResult {
                allowed: true,
                block_reason: None,
                blocked_address: None,
                matched_rule: Some("Whitelist".into()),
            };
        }

        // 2. 黑名单检查
        if let Some(reason) = self.check_blacklist(url) {
            return SsrfCheckResult {
                allowed: false,
                block_reason: Some(reason),
                blocked_address: Some(url.to_string()),
                matched_rule: Some("CustomBlacklist".into()),
            };
        }

        // 3. 解析 URL 获取主机名
        let host = match Self::extract_host(url) {
            Some(h) => h,
            None => {
                return SsrfCheckResult {
                    allowed: false,
                    block_reason: Some("无法解析 URL 主机名".into()),
                    blocked_address: Some(url.to_string()),
                    matched_rule: Some("ParseError".into()),
                };
            }
        };

        // 4. 检查云 Metadata 端点
        if let Some(result) = self.check_metadata_endpoint(url, &host) {
            return result;
        }

        // 5. 检查是否为 IP 地址
        if let Ok(ip_addr) = host.parse::<IpAddr>() {
            if let Some(result) = self.check_ip(&ip_addr) {
                return result;
            }
        }

        // 6. DNS 解析验证 (防止 DNS rebinding)
        if self.config.enable_dns_verification {
            if let Some(result) = self.dns_verify_and_check(&host).await {
                return result;
            }
        }

        // 通过所有检查
        SsrfCheckResult {
            allowed: true,
            block_reason: None,
            blocked_address: None,
            matched_rule: None,
        }
    }

    // ─── 检查逻辑实现 ─────────────────────────────────

    fn is_whitelisted(&self, url: &str) -> bool {
        let host_lower = Self::extract_host(url).unwrap_or_default().to_lowercase();

        // 精确匹配或子域名匹配
        for entry in &self.config.whitelist {
            let entry_lower = entry.to_lowercase();
            if host_lower == entry_lower || host_lower.ends_with(&format!(".{}", entry_lower)) {
                return true;
            }
        }
        false
    }

    fn check_blacklist(&self, url: &str) -> Option<String> {
        let url_lower = url.to_lowercase();
        for pattern in &self.config.blacklist {
            if url_lower.contains(&pattern.to_lowercase()) {
                return Some(format!("命中自定义黑名单模式: {}", pattern));
            }
        }
        None
    }

    fn check_metadata_endpoint(
        &self,
        url: &str,
        host: &str,
    ) -> Option<SsrfCheckResult> {
        let host_lower = host.to_lowercase();

        // 检查主机名是否为已知的 metadata 主机
        if CLOUD_METADATA_HOSTS.iter().any(|h| *h == host_lower || host_lower.ends_with(&format!(".{}", h.to_lowercase()))) {
            return Some(SsrfCheckResult {
                allowed: false,
                block_reason: Some(format!(
                    "阻止访问云 Metadata 端点: {}. 这可能泄露实例凭证或敏感元数据。",
                    host
                )),
                blocked_address: Some(host.to_string()),
                matched_rule: Some("CloudMetadataEndpoint".into()),
            });
        }

        // 检查 URL 路径是否包含 metadata 路径模式
        for re in METADATA_PATH_PATTERNS.iter() {
            if re.is_match(url) {
                return Some(SsrfCheckResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "检测到疑似 Metadata API 访问路径。URL: {}",
                        if url.len() > 120 {
                            format!("{}...", &url[..120])
                        } else {
                            url.to_string()
                        }
                    )),
                    blocked_address: Some(url.to_string()),
                    matched_rule: Some("MetadataPathPattern".into()),
                });
            }
        }

        None
    }

    fn check_ip(&self, ip: &IpAddr) -> Option<SsrfCheckResult> {
        match ip {
            IpAddr::V4(v4) => self.check_ipv4(v4),
            IpAddr::V6(v6) => self.check_ipv6(v6),
        }
    }

    fn check_ipv4(&self, ip: &Ipv4Addr) -> Option<SsrfCheckResult> {
        // 检查标准私有范围
        for range in PRIVATE_IPV4_RANGES.iter().chain(self.custom_private_ranges.iter()) {
            if Self::ip_in_range(ip, &range.start, &range.end) {
                return Some(SsrfCheckResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "阻止访问内网 IPv4 地址 {} (属于 {} 范围). \
                         这可能是 SSRF 攻击或意外访问内部服务。",
                        ip, range.name
                    )),
                    blocked_address: Some(ip.to_string()),
                    matched_rule: Some(range.name.to_string()),
                });
            }
        }
        None
    }

    fn check_ipv6(&self, ip: &Ipv6Addr) -> Option<SsrfCheckResult> {
        // ::1 (IPv6 loopback)
        if ip.is_loopback() {
            return Some(SsrfCheckResult {
                allowed: false,
                block_reason: Some("阻止访问 IPv6 loopback 地址 (::1)".into()),
                blocked_address: Some(ip.to_string()),
                matched_rule: Some("LoopbackV6".into()),
            });
        }

        // fe80::/10 (link-local)
        if ip.octets()[0] == 0xfe && (ip.octets()[1] & 0xc0) == 0x80 {
            return Some(SsrfCheckResult {
                allowed: false,
                block_reason: Some("阻止访问 IPv6 link-local 地址 (fe80::/10)".into()),
                blocked_address: Some(ip.to_string()),
                matched_rule: Some("LinkLocalV6".into()),
            });
        }

        // ::ffff:x:x (IPv4-mapped IPv6) — 提取并检查内部的 IPv4
        if let Some(v4) = Self::extract_mapped_ipv4(ip) {
            return self.check_ipv4(&v4);
        }

        // fc00::/7 (unique local, 相当于 IPv6 的私有地址)
        if (ip.octets()[0] & 0xfc) == 0xfc {
            return Some(SsrfCheckResult {
                allowed: false,
                block_reason: Some("阻止访问 IPv6 unique-local 地址 (fc00::/7)".into()),
                blocked_address: Some(ip.to_string()),
                matched_rule: Some("UniqueLocalV6".into()),
            });
        }

        None
    }

    /// DNS 解析 + 二次验证 (防止 DNS rebinding)
    async fn dns_verify_and_check(&self, hostname: &str) -> Option<SsrfCheckResult> {
        // 如果看起来像 IP 地址则跳过 DNS
        if hostname.parse::<IpAddr>().is_ok() {
            return None;
        }

        // 执行 DNS 解析
        match tokio::time::timeout(
            Duration::from_millis(self.config.dns_timeout_ms),
            tokio::net::lookup_host(hostname),
        )
        .await
        {
            Ok(Ok(addrs)) => {
                for addr in addrs {
                    if let Some(blocked) = self.check_ip(&addr.ip()) {
                        return Some(SsrfCheckResult {
                            allowed: false,
                            block_reason: Some(format!(
                                "{} DNS 解析到内网地址 {}. 这可能是 DNS rebinding 攻击.",
                                hostname, addr.ip()
                            )),
                            blocked_address: Some(addr.ip().to_string()),
                            matched_rule: Some("DnsRebinding".into()),
                        });
                    }
                }
                None
            }
            Ok(Err(e)) => {
                tracing::warn!(host = hostname, error = %e, "DNS resolution failed");
                // DNS 失败时不阻止 (保守策略: 允许请求继续, 由目标服务器拒绝)
                None
            }
            Err(_) => {
                tracing::warn!(host = hostname, "DNS resolution timeout");
                None // 超时也不阻止
            }
        }
    }

    // ─── 工具函数 ──────────────────────────────────────

    fn extract_host(url: &str) -> Option<String> {
        // 移除协议前缀
        let without_scheme = url.trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("ws://")
            .trim_start_matches("wss://");

        // 提取 host:port 部分 (去掉路径、查询参数等)
        let host_part = without_scheme
            .split('/')
            .next()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("")
            .split('#')
            .next()
            .unwrap_or("");

        if host_part.is_empty() {
            None
        } else {
            // 去掉端口部分 (如果有)
            let host = host_part.split(':').next().unwrap_or(host_part);
            if host.is_empty() { None } else { Some(host.to_string()) }
        }
    }

    fn ip_in_range(ip: &Ipv4Addr, start: &Ipv4Addr, end: &Ipv4Addr) -> bool {
        let ip_u32: u32 = (*ip).into();
        let start_u32: u32 = (*start).into();
        let end_u32: u32 = (*end).into();
        ip_u32 >= start_u32 && ip_u32 <= end_u32
    }

    fn extract_mapped_ipv4(ip: &Ipv6Addr) -> Option<Ipv4Addr> {
        let octets = ip.octets();
        // IPv4-mapped 格式: ::ffff:a.b.c.d → 前 10 字节是 0, 第 11-12 是 0xff, 后 4 字节是 IPv4
        if octets[0..10] == [0; 10] && octets[10..12] == [0xff, 0xff] {
            Some(Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]))
        } else {
            None
        }
    }

    /// 批量检查多个 URL
    pub async fn check_urls(&self, urls: &[String]) -> Vec<(String, SsrfCheckResult)> {
        let mut results = Vec::with_capacity(urls.len());
        for url in urls {
            let result = self.check_url(url).await;
            results.push((url.clone(), result));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_public_ip_allowed() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("https://www.google.com/search?q=test").await;
        assert!(result.allowed, "公网 URL 应被允许, got: {:?}", result.block_reason);
    }

    #[tokio::test]
    async fn test_loopback_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://localhost:3000/api").await;
        assert!(!result.allowed, "loopback 应被阻止");
        assert_eq!(result.matched_rule.as_deref(), Some("Loopback"));
    }

    #[tokio::test]
    async fn test_127_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://127.0.0.1:8080/admin").await;
        assert!(!result.allowed, "127.0.0.1 应被阻止");
    }

    #[tokio::test]
    async fn test_private_192_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://192.168.1.1/admin").await;
        assert!(!result.allowed, "192.168.x.x 应被阻止");
        assert_eq!(result.matched_rule.as_deref(), Some("RFC1918-192"));
    }

    #[tokio::test]
    async fn test_private_10_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://10.0.0.1/internal").await;
        assert!(!result.allowed);
        assert_eq!(result.matched_rule.as_deref(), Some("RFC1918-10"));
    }

    #[tokio::test]
    async fn test_private_172_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://172.16.0.1/api").await;
        assert!(!result.allowed);
    }

    #[tokio::test]
    async fn test_metadata_endpoint_blocked() {
        let guard = SsrfGuard::new();
        let result = guard
            .check_url("http://169.254.169.254/latest/meta-data/")
            .await;
        assert!(!result.allowed, "AWS metadata endpoint 应被阻止");
        assert!(result.block_reason.as_ref().unwrap().contains("Metadata"));
    }

    #[tokio::test]
    async fn test_gcp_metadata_blocked() {
        let guard = SsrfGuard::new();
        let result = guard
            .check_url("http://metadata.google.internal/computeMetadata/v1/")
            .await;
        assert!(!result.allowed, "GCP metadata 应被阻止");
    }

    #[tokio::test]
    async fn test_link_local_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://169.254.169.254/").await;
        assert!(!result.allowed);
    }

    #[tokio::test]
    async fn test_ipv6_loopback_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://[::1]:8080/").await;
        assert!(!result.allowed);
    }

    #[tokio::test]
    async fn test_ipv4_mapped_ipv6_blocked() {
        let guard = SsrfGuard::new();
        let result = guard.check_url("http://[::ffff:127.0.0.1]:3000/").await;
        assert!(!result.allowed, "IPv4-mapped loopback 应被阻止");
    }

    #[tokio::test]
    async fn test_whitelist_override() {
        let guard = SsrfGuard::new().with_whitelist(vec!["localhost".to_string()]);
        let result = guard.check_url("http://localhost:9200/_cluster/health").await;
        assert!(result.allowed, "白名单中的 localhost 应被允许");
    }

    #[tokio::test]
    async fn test_disabled_guard_allows_all() {
        let guard = SsrfGuard::with_config(SsrfGuardConfig {
            enabled: false,
            ..Default::default()
        });
        let result = guard.check_url("http://127.0.0.1/secret").await;
        assert!(result.allowed, "禁用时应允许所有请求");
    }

    #[tokio::test]
    async fn test_azure_instance_data_blocked() {
        let guard = SsrfGuard::new();
        let result = guard
            .check_url("http://instance-data.azure.net/metadata")
            .await;
        assert!(!result.allowed);
    }
}
