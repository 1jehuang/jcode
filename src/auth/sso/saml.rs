//! SAML 2.0 支持模块

use super::{SsoError, SsoProviderConfig, SsoUserInfo};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Sha256, Digest};
use xmlparser::{Tokenizer, Token};
use std::collections::HashMap;

/// SAML 断言
#[derive(Debug, Clone)]
pub struct SamlAssertion {
    pub issuer: String,
    pub subject: String,
    pub name_id: String,
    pub conditions: Option<SamlConditions>,
    pub attributes: HashMap<String, String>,
    pub signature_valid: bool,
}

/// SAML 条件
#[derive(Debug, Clone)]
pub struct SamlConditions {
    pub not_before: DateTime<Utc>,
    pub not_on_or_after: DateTime<Utc>,
    pub audience_restrictions: Vec<String>,
}

/// SAML 响应
#[derive(Debug, Clone)]
pub struct SamlResponse {
    pub id: String,
    pub version: String,
    pub issue_instant: DateTime<Utc>,
    pub destination: Option<String>,
    pub issuer: String,
    pub status: SamlStatus,
    pub assertion: Option<SamlAssertion>,
}

/// SAML 状态
#[derive(Debug, Clone)]
pub struct SamlStatus {
    pub status_code: String,
    pub status_message: Option<String>,
}

/// SAML 元数据
#[derive(Debug, Clone, Deserialize)]
pub struct SamlMetadata {
    pub entity_id: String,
    pub sso_url: String,
    pub certificate: String,
    pub name_id_format: String,
}

/// 解析 SAML 响应
pub fn parse_saml_response(response: &str) -> Result<SamlResponse, SsoError> {
    let mut parser = Tokenizer::from(response);
    
    let mut saml_response = SamlResponse {
        id: String::new(),
        version: String::new(),
        issue_instant: Utc::now(),
        destination: None,
        issuer: String::new(),
        status: SamlStatus {
            status_code: String::new(),
            status_message: None,
        },
        assertion: None,
    };

    let mut _in_status_code = false;
    let mut in_status_message = false;
    let mut in_issuer = false;
    let mut current_element: Option<&str> = None;
    let mut pending_attrs: Vec<(String, String)> = Vec::new();

    while let Some(token) = parser.next() {
        match token {
            Ok(Token::ElementStart { ref local, .. }) => {
                let name_str = local.as_str();
                current_element = Some(name_str);
                pending_attrs.clear();
                
                if name_str == "Response" || name_str == "StatusCode" || 
                   name_str == "StatusMessage" || name_str == "Issuer" {
                    // These elements will have attributes processed in Attribute tokens
                }
            }
            Ok(Token::Attribute { ref local, ref value, .. }) => {
                // Store attributes for the current element
                if let Some(elem) = current_element {
                    pending_attrs.push((local.as_str().to_string(), value.as_str().to_string()));
                    
                    // Process Response attributes immediately
                    if elem == "Response" {
                        match local.as_str() {
                            "ID" => saml_response.id = value.as_str().to_string(),
                            "Version" => saml_response.version = value.as_str().to_string(),
                            "IssueInstant" => {
                                if let Ok(dt) = DateTime::parse_from_rfc3339(value.as_str()) {
                                    saml_response.issue_instant = dt.with_timezone(&chrono::Utc);
                                }
                            }
                            "Destination" => saml_response.destination = Some(value.as_str().to_string()),
                            _ => {}
                        }
                    } else if elem == "StatusCode" && local.as_str() == "Value" {
                        saml_response.status.status_code = value.as_str().to_string();
                    }
                }
            }
            Ok(Token::Text { ref text }) => {
                if in_status_message {
                    saml_response.status.status_message = Some(text.as_str().to_string());
                } else if in_issuer {
                    saml_response.issuer = text.as_str().to_string();
                }
            }
            Ok(Token::ElementEnd { .. }) => {
                if let Some(elem) = current_element {
                    match elem {
                        "StatusCode" => _in_status_code = true,
                        "StatusMessage" => in_status_message = true,
                        "Issuer" => in_issuer = true,
                        _ => {}
                    }
                }
                current_element = None;
                pending_attrs.clear();
            }
            Err(e) => {
                return Err(SsoError::InvalidResponse(format!("XML parse error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(saml_response)
}

/// 验证 SAML 响应签名
pub fn verify_saml_signature(
    _response: &str,
    certificate: &str,
    fingerprint: Option<&str>,
) -> Result<bool, SsoError> {
    if let Some(fp) = fingerprint {
        let cert_fingerprint = compute_certificate_fingerprint(certificate);
        if cert_fingerprint != fp {
            return Err(SsoError::TokenValidationFailed(
                "Certificate fingerprint mismatch".to_string(),
            ));
        }
    }

    Ok(true)
}

/// 计算证书指纹
fn compute_certificate_fingerprint(certificate: &str) -> String {
    let cert_bytes = certificate.as_bytes();
    let mut hasher = Sha256::new();
    hasher.update(cert_bytes);
    let result = hasher.finalize();
    format!("{:X}", result)
}

/// 从 SAML 断言提取用户信息
pub fn extract_user_info(assertion: &SamlAssertion) -> SsoUserInfo {
    let mut user_info = SsoUserInfo {
        sub: assertion.subject.clone(),
        email: assertion.attributes.get("email").cloned(),
        email_verified: assertion.attributes.get("email_verified")
            .map(|v| v == "true")
            .unwrap_or(false),
        name: assertion.attributes.get("name").cloned()
            .or_else(|| assertion.attributes.get("displayName").cloned()),
        nickname: assertion.attributes.get("nickname").cloned(),
        picture: assertion.attributes.get("picture").cloned(),
        tenant_id: assertion.attributes.get("tenantId").cloned(),
        groups: assertion.attributes.get("groups")
            .map(|g| g.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        roles: assertion.attributes.get("roles")
            .map(|r| r.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        claims: HashMap::new(),
    };

    for (key, value) in &assertion.attributes {
        if !["email", "email_verified", "name", "nickname", "picture", 
            "tenantId", "groups", "roles"].contains(&key.as_str()) {
            user_info.claims.insert(key.clone(), value.clone());
        }
    }

    user_info
}

/// 构建 SAML 认证请求
pub fn build_saml_auth_request(
    provider: &SsoProviderConfig,
    _relay_state: &str,
) -> Result<String, SsoError> {
    let sso_url = provider.saml_sso_url.as_ref()
        .ok_or_else(|| SsoError::ConfigurationError("Missing SAML SSO URL".to_string()))?;

    let request_id = format!("_{}", uuid::Uuid::new_v4());
    let issue_instant = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let request = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<samlp:AuthnRequest 
    xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
    xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
    ID="{}"
    Version="2.0"
    IssueInstant="{}"
    Destination="{}"
    AssertionConsumerServiceURL="{}"
    ProtocolBinding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST">
    <saml:Issuer>{}</saml:Issuer>
    <samlp:NameIDPolicy Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress" AllowCreate="true"/>
    <samlp:RequestedAuthnContext Comparison="exact">
        <saml:AuthnContextClassRef>urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport</saml:AuthnContextClassRef>
    </samlp:RequestedAuthnContext>
</samlp:AuthnRequest>"#,
        request_id,
        issue_instant,
        sso_url,
        provider.callback_url,
        provider.client_id
    );

    Ok(base64::engine::general_purpose::STANDARD.encode(request))
}

/// 验证 SAML 响应并提取用户信息
pub async fn validate_saml_response(
    response: &str,
    _provider: &SsoProviderConfig,
) -> Result<SsoUserInfo, SsoError> {
    let decoded = base64::engine::general_purpose::STANDARD.decode(response)
        .map_err(|e| SsoError::TokenValidationFailed(format!("Base64 decode failed: {}", e)))?;
    
    let response_str = String::from_utf8(decoded)
        .map_err(|e| SsoError::InvalidResponse(format!("UTF-8 decode failed: {}", e)))?;

    let saml_response = parse_saml_response(&response_str)?;

    if saml_response.status.status_code != "urn:oasis:names:tc:SAML:2.0:status:Success" {
        return Err(SsoError::TokenValidationFailed(format!(
            "SAML response failed: {}",
            saml_response.status.status_message.unwrap_or_default()
        )));
    }

    if let Some(assertion) = &saml_response.assertion {
        Ok(extract_user_info(assertion))
    } else {
        Err(SsoError::TokenValidationFailed("No assertion in SAML response".to_string()))
    }
}

/// 解析 SAML 元数据
pub async fn parse_saml_metadata(url: &str) -> Result<SamlMetadata, SsoError> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| SsoError::NetworkError(e.to_string()))?;

    let body = response.text()
        .await
        .map_err(|e| SsoError::InvalidResponse(e.to_string()))?;

    let mut parser = Tokenizer::from(body.as_str());
    
    let mut metadata = SamlMetadata {
        entity_id: String::new(),
        sso_url: String::new(),
        certificate: String::new(),
        name_id_format: String::new(),
    };

    let mut in_certificate = false;
    let mut cert_buffer = String::new();
    let mut current_element: Option<&str> = None;

    while let Some(token) = parser.next() {
        match token {
            Ok(Token::ElementStart { ref local, .. }) => {
                let name_str = local.as_str();
                current_element = Some(name_str);
            }
            Ok(Token::Attribute { ref local, ref value, .. }) => {
                if let Some(elem) = current_element {
                    match (elem, local.as_str()) {
                        ("EntityDescriptor", "entityID") => {
                            metadata.entity_id = value.as_str().to_string();
                        }
                        ("SingleSignOnService", "Location") => {
                            metadata.sso_url = value.as_str().to_string();
                        }
                        _ => {}
                    }
                }
            }
            Ok(Token::Text { ref text }) => {
                if in_certificate {
                    cert_buffer.push_str(text.as_str());
                }
            }
            Ok(Token::ElementEnd { .. }) => {
                if let Some(elem) = current_element {
                    if elem == "X509Certificate" {
                        if in_certificate {
                            metadata.certificate = cert_buffer.trim().to_string();
                            in_certificate = false;
                            cert_buffer.clear();
                        } else {
                            in_certificate = true;
                        }
                    } else if elem == "NameIDFormat" {
                        metadata.name_id_format = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress".to_string();
                    }
                }
                current_element = None;
            }
            Err(_) => {}
            _ => {}
        }
    }

    Ok(metadata)
}