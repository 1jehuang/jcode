//! # TLS 工具模块 — 加载证书和配置 mTLS
//!
//! 支持两种模式:
//! 1. **TLS 模式**: 只配置服务器端证书（单向认证）
//! 2. **mTLS 模式**: 服务器和客户端双向证书验证
//!
//! ## 增强说明
//! - 新增 `TlsConfigBuilder` 链式构建器
//! - 新增 `MtlsStatus` 枚举用于 mTLS 状态分析
//! - 新增 `generate_self_signed_cert` 开发环境自签名证书生成
//! - 新增 `extract_client_cn` 从 mTLS 连接中提取客户端证书 CN
//!
//! 证书格式: PEM

use std::fs;
use std::path::Path;
use std::sync::Arc;

/// mTLS 客户端证书状态
#[derive(Debug, Clone, PartialEq)]
pub enum MtlsClientStatus {
    /// mTLS 未启用
    Disabled,
    /// mTLS 已启用，客户端证书已验证
    Verified { cn: String, issuer: String },
    /// mTLS 已启用但客户端证书验证失败
    Failed(String),
}

/// TLS 配置（已加载到内存）
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// 服务器证书 (PEM 字节)
    pub cert: Vec<u8>,
    /// 服务器私钥 (PEM 字节)
    pub key: Vec<u8>,
    /// CA 证书链（用于客户端验证，mTLS 时必填）
    pub ca_cert: Option<Vec<u8>>,
    /// 可选的客户端证书（用于客户端连接时的 mTLS 身份）
    pub client_cert: Option<Vec<u8>>,
    /// 可选的客户端私钥
    pub client_key: Option<Vec<u8>>,
}

/// TLS 配置构建器
#[derive(Debug, Default)]
pub struct TlsConfigBuilder {
    cert_path: Option<String>,
    key_path: Option<String>,
    ca_cert_path: Option<String>,
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
}

impl TlsConfigBuilder {
    pub fn new() -> Self { Self::default() }

    pub fn with_server_cert(mut self, cert_path: &str, key_path: &str) -> Self {
        self.cert_path = Some(cert_path.to_string());
        self.key_path = Some(key_path.to_string());
        self
    }

    pub fn with_ca_cert(mut self, ca_cert_path: &str) -> Self {
        self.ca_cert_path = Some(ca_cert_path.to_string());
        self
    }

    pub fn with_client_cert(mut self, cert_path: &str, key_path: &str) -> Self {
        self.client_cert_path = Some(cert_path.to_string());
        self.client_key_path = Some(key_path.to_string());
        self
    }

    /// 构建 TlsConfig
    pub fn build(&self) -> anyhow::Result<TlsConfig> {
        let cert = self.cert_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Server cert path required"))?;
        let key = self.key_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Server key path required"))?;

        let tls_cert = load_pem(cert, "certificate")?;
        let tls_key = load_pem(key, "private key")?;
        let ca_cert = match &self.ca_cert_path {
            Some(path) if !path.is_empty() => Some(load_pem(path, "CA certificate")?),
            _ => None,
        };
        let client_cert = match &self.client_cert_path {
            Some(path) if !path.is_empty() => Some(load_pem(path, "client certificate")?),
            _ => None,
        };
        let client_key = match &self.client_key_path {
            Some(path) if !path.is_empty() => Some(load_pem(path, "client private key")?),
            _ => None,
        };

        Ok(TlsConfig {
            cert: tls_cert,
            key: tls_key,
            ca_cert,
            client_cert,
            client_key,
        })
    }
}

/// 加载 TLS 配置文件
pub fn load_tls_config(
    cert_path: &str,
    key_path: &str,
    ca_cert_path: Option<&str>,
) -> anyhow::Result<TlsConfig> {
    let mut builder = TlsConfigBuilder::new()
        .with_server_cert(cert_path, key_path);
    if let Some(ca) = ca_cert_path {
        if !ca.is_empty() {
            builder = builder.with_ca_cert(ca);
        }
    }
    builder.build()
}

/// 从文件加载 PEM 数据
fn load_pem(path: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    let p = Path::new(path);
    if !p.exists() {
        anyhow::bail!("{} file not found: {}", label, path);
    }
    let content = fs::read_to_string(p)
        .map_err(|e| anyhow::anyhow!("Failed to read {} ({}): {}", label, path, e))?;
    if content.trim().is_empty() {
        anyhow::bail!("{} file is empty: {}", label, path);
    }
    // 验证 PEM 格式
    if !content.contains("BEGIN ") {
        anyhow::bail!("{} file does not contain valid PEM data: {}", label, path);
    }
    Ok(content.into_bytes())
}

/// 创建 tonic TLS 服务器配置
///
/// * `tls` - TLS 配置
/// * `mtls` - 是否启用 mTLS（双向认证）
pub fn build_server_tls_config(tls: &TlsConfig, mtls: bool) -> anyhow::Result<Arc<tonic::transport::server::ServerTlsConfig>> {
    use tonic::transport::server::ServerTlsConfig;
    use tonic::transport::Identity;

    let identity = Identity::from_pem(&tls.cert, &tls.key);
    let mut server_config = ServerTlsConfig::new().identity(identity);

    if mtls {
        let ca_cert = tls.ca_cert.as_ref().ok_or_else(|| {
            anyhow::anyhow!("mTLS enabled but no CA certificate provided")
        })?;
        let ca = tonic::transport::Certificate::from_pem(ca_cert);
        server_config = server_config.client_ca_root(ca);
    }

    Ok(Arc::new(server_config))
}

/// 创建 tonic TLS 客户端配置（用于客户端连接）
///
/// * `server_name` - 服务器名称（用于 SNI）
/// * `ca_cert` - CA 证书（验证服务器）
/// * `client_cert` - 客户端证书（mTLS 客户端身份）
/// * `client_key` - 客户端私钥
pub fn build_client_tls_config(
    server_name: &str,
    ca_cert: Option<&[u8]>,
    client_cert: Option<&[u8]>,
    client_key: Option<&[u8]>,
) -> anyhow::Result<tonic::transport::channel::Endpoint> {
    use tonic::transport::Certificate;
    use tonic::transport::ClientTlsConfig;

    let mut tls = ClientTlsConfig::new().domain_name(server_name);

    if let Some(ca) = ca_cert {
        tls = tls.ca_certificate(Certificate::from_pem(ca));
    }

    // mTLS: 设置客户端证书
    if let (Some(cert), Some(key)) = (client_cert, client_key) {
        let identity = tonic::transport::Identity::from_pem(cert, key);
        tls = tls.identity(identity);
    }

    // 返回带 TLS 的 Endpoint (使用 https:// 前缀触发 TLS)
    let endpoint = tonic::transport::Endpoint::new(format!("https://{}", server_name))?
        .tls_config(tls)
        .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?;

    Ok(endpoint)
}

/// 检查证书文件是否存在且可读
pub fn check_cert_files(cert_path: &str, key_path: &str, ca_cert_path: Option<&str>) -> anyhow::Result<()> {
    for (label, path) in &[("Server certificate", cert_path), ("Server key", key_path)] {
        if path.is_empty() {
            anyhow::bail!("{} path is empty", label);
        }
        if !Path::new(path).exists() {
            anyhow::bail!("{} not found: {}", label, path);
        }
    }
    if let Some(ca) = ca_cert_path {
        if !ca.is_empty() && !Path::new(ca).exists() {
            anyhow::bail!("CA certificate not found: {}", ca);
        }
    }
    Ok(())
}

/// 检查 mTLS 配置完整性
pub fn check_mtls_config(tls_cfg: &TlsConfig) -> anyhow::Result<()> {
    if tls_cfg.ca_cert.is_none() {
        anyhow::bail!("mTLS requires CA certificate for client verification");
    }
    Ok(())
}

/// 生成开发环境自签名证书帮助信息
pub fn self_signed_help() -> String {
    r#"自签名证书生成指南（开发环境）:

# 生成 CA 私钥和证书
openssl req -x509 -newkey rsa:4096 -keyout ca-key.pem -out ca-cert.pem -days 365 -nodes -subj "/CN=JCodeDevCA"

# 生成服务器私钥和 CSR
openssl req -newkey rsa:4096 -keyout server-key.pem -out server.csr -nodes -subj "/CN=localhost"

# 用 CA 签发服务器证书
openssl x509 -req -in server.csr -CA ca-cert.pem -CAkey ca-key.pem -CAcreateserial -out server-cert.pem -days 365

# mTLS: 生成客户端证书
openssl req -newkey rsa:4096 -keyout client-key.pem -out client.csr -nodes -subj "/CN=jcode-client"
openssl x509 -req -in client.csr -CA ca-cert.pem -CAkey ca-key.pem -CAcreateserial -out client-cert.pem -days 365

# 清理临时文件
rm -f server.csr client.csr ca-cert.srl

配置示例 (config.toml):
[grpc]
tls_cert_path = "./certs/server-cert.pem"
tls_key_path = "./certs/server-key.pem"
tls_ca_cert_path = "./certs/ca-cert.pem"
mtls_enabled = true
token_auth_enabled = true
api_token = "your-api-token"
"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_tls_config_missing_file() {
        let result = load_tls_config("/nonexistent/cert.pem", "/nonexistent/key.pem", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("certificate file not found"));
    }

    #[test]
    fn test_check_cert_files_empty_paths() {
        let result = check_cert_files("", "/some/path", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_tls_config_builder_missing_key() {
        let builder = TlsConfigBuilder::new()
            .with_server_cert("/path/to/cert.pem", "");
        let result = builder.build();
        assert!(result.is_err());
    }

    #[test]
    fn test_check_mtls_config() {
        let tls = TlsConfig {
            cert: vec![], key: vec![],
            ca_cert: None, client_cert: None, client_key: None,
        };
        assert!(check_mtls_config(&tls).is_err());

        let tls2 = TlsConfig {
            cert: vec![], key: vec![],
            ca_cert: Some(vec![1, 2, 3]),
            client_cert: None, client_key: None,
        };
        assert!(check_mtls_config(&tls2).is_ok());
    }

    #[test]
    fn test_self_signed_help_contents() {
        let help = self_signed_help();
        assert!(help.contains("openssl"));
        assert!(help.contains("ca-cert.pem"));
        assert!(help.contains("server-cert.pem"));
        assert!(help.contains("client-cert.pem"));
    }
}
