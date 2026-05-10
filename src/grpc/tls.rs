//! # TLS 工具模块 — 加载证书和配置 mTLS
//!
//! 支持两种模式:
//! 1. **TLS 模式**: 只配置服务器端证书（单向认证）
//! 2. **mTLS 模式**: 服务器和客户端双向证书验证
//!
//! 证书格式: PEM

use std::fs;
use std::path::Path;
use std::sync::Arc;

/// TLS 配置（已加载到内存）
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// 服务器证书 (DER/PEM 字节)
    pub cert: Vec<u8>,
    /// 服务器私钥 (DER/PEM 字节)
    pub key: Vec<u8>,
    /// CA 证书链（用于客户端验证，mTLS 时必填）
    pub ca_cert: Option<Vec<u8>>,
}

/// 加载 TLS 配置文件
pub fn load_tls_config(
    cert_path: &str,
    key_path: &str,
    ca_cert_path: Option<&str>,
) -> anyhow::Result<TlsConfig> {
    let cert = load_pem(cert_path, "certificate")?;
    let key = load_pem(key_path, "private key")?;
    let ca_cert = match ca_cert_path {
        Some(path) if !path.is_empty() => Some(load_pem(path, "CA certificate")?),
        _ => None,
    };

    Ok(TlsConfig { cert, key, ca_cert })
}

/// 从文件加载 PEM 数据
fn load_pem(path: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    let p = Path::new(path);
    if !p.exists() {
        anyhow::bail!("{} file not found: {}", label, path);
    }
    let content = fs::read_to_string(p)
        .map_err(|e| anyhow::anyhow!("Failed to read {} ({}): {}", label, path, e))?;
    Ok(content.into_bytes())
}

/// 创建 tonic TLS 服务器配置
///
/// * `tls` - TLS 配置
/// * `mtls` - 是否启用 mTLS（双向认证）
pub fn build_server_tls_config(tls: &TlsConfig, mtls: bool) -> anyhow::Result<Arc<tonic::transport::server::ServerTlsConfig>> {
    use tonic::transport::Identity;
    use tonic::transport::server::ServerTlsConfig;

    let identity = Identity::from_pem(&tls.cert, &tls.key);

    let mut server_config = ServerTlsConfig::new().identity(identity);

    if mtls {
        // mTLS: 验证客户端证书
        let ca_cert = tls.ca_cert.as_ref().ok_or_else(|| {
            anyhow::anyhow!("mTLS enabled but no CA certificate provided")
        })?;
        let ca = tonic::transport::Certificate::from_pem(ca_cert);

        // 设置客户端 CA 证书用于验证
        server_config = server_config
            .client_ca_root(ca);
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

    let mut tls = ClientTlsConfig::new()
        .domain_name(server_name);

    if let Some(ca) = ca_cert {
        tls = tls.ca_certificate(Certificate::from_pem(ca));
    }

    // mTLS: 设置客户端证书
    if let (Some(cert), Some(key)) = (client_cert, client_key) {
        let identity = tonic::transport::Identity::from_pem(cert, key);
        tls = tls.identity(identity);
    }

    // 返回带 TLS 的 Endpoint
    let endpoint = tonic::transport::Endpoint::new(format!("https://{}", server_name))?
        .tls_config(tls)
        .map_err(|e| anyhow::anyhow!("TLS config error: {}", e))?;

    Ok(endpoint)
}

/// 检查证书文件是否存在且可读
pub fn check_cert_files(cert_path: &str, key_path: &str, ca_cert_path: Option<&str>) -> anyhow::Result<()> {
    let files_to_check = vec![
        ("Server certificate", cert_path),
        ("Server key", key_path),
    ];
    for (label, path) in &files_to_check {
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
}
