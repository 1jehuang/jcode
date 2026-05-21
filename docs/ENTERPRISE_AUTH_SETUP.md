# CarpAI 企业级安全功能集成指南

本文档介绍如何在 CarpAI 中集成和使用企业级安全功能，包括 OAuth2 + JWT 认证、RBAC 权限系统、审计日志和 GDPR 合规、以及数据加密。

## 目录

1. [架构概览](#架构概览)
2. [OAuth2 + JWT 认证](#oauth2--jwt-认证)
3. [RBAC 权限系统](#rbac-权限系统)
4. [审计日志与 GDPR 合规](#审计日志与-gdpr-合规)
5. [数据加密](#数据加密)
6. [完整集成示例](#完整集成示例)
7. [生产环境部署](#生产环境部署)

---

## 架构概览

CarpAI 企业级安全模块 (`jcode-auth`) 提供以下核心功能：

```
┌─────────────────────────────────────────────┐
│         Enterprise Security Stack           │
├─────────────────────────────────────────────┤
│  OAuth2 Provider    │  JWT Manager          │
│  - Google           │  - Token Generation   │
│  - GitHub           │  - Validation         │
│  - Azure AD         │  - Refresh            │
│  - Okta             │  - Claims Management  │
├─────────────────────┼───────────────────────┤
│  RBAC Engine        │  Audit Logger         │
│  - Role Management  │  - Event Logging      │
│  - Permission Flags │  - GDPR Compliance    │
│  - Hierarchies      │  - Retention Policy   │
│  - User Assignment  │  - Consent Mgmt       │
├─────────────────────┴───────────────────────┤
│     Encryption Manager (AES-256-GCM)        │
│  - Key Generation    - Key Rotation         │
│  - Data Encryption   - Secure Storage       │
└─────────────────────────────────────────────┘
```

### 添加依赖

在 `Cargo.toml` 中添加：

```toml
[dependencies]
jcode-auth = { path = "crates/jcode-auth" }
```

---

## OAuth2 + JWT 认证

### 1. 配置 OAuth2 提供商

```rust
use jcode_auth::oauth::*;

// 使用预配置的 GitHub OAuth2
let config = ProviderType::GitHub.default_config(
    "your_client_id",
    "your_client_secret"
);

let provider = StandardOAuthProvider::new(config)?;

// 生成授权 URL
let (auth_url, csrf_token, pkce_verifier) = provider.get_authorization_url()?;
println!("Visit: {}", auth_url);
```

### 2. 交换授权码获取令牌

```rust
// 用户完成 OAuth2 流程后，用授权码交换令牌
let token = provider.exchange_code(
    authorization_code,
    pkce_verifier
).await?;

println!("Access token: {}", token.access_token);
```

### 3. 生成 JWT Session Token

```rust
use jcode_auth::jwt::*;

// 创建 JWT 管理器（HS256）
let jwt_manager = JwtManager::new_hs256(
    b"your_jwt_secret_key",
    "carpai-server".to_string(),
    24, // 24小时过期
)?;

// 生成访问令牌
let claims = JwtClaims::new(
    user_id.to_string(),
    "carpai-server".to_string(),
    1, // 1小时
)
.with_claim("roles", serde_json::json!(["developer"]))
.with_claim("email", serde_json::json!(user_email));

let token = jwt_manager.generate_token(claims)?;
```

### 4. 验证 JWT Token

```rust
let validation = jwt_manager.validate_token(&token)?;

if validation.is_valid {
    println!("User: {}", validation.claims.sub);
    println!("Issuer: {}", validation.claims.iss);
} else {
    eprintln!("Invalid token: {:?}", validation.error);
}
```

---

## RBAC 权限系统

### 1. 初始化 RBAC 引擎

```rust
use jcode_auth::rbac::*;

let rbac_engine = RbacEngine::new();

// 预定义角色已自动注册：
// - admin (所有权限)
// - developer (代码和文件操作)
// - viewer (只读)
// - auditor (审计和合规)
```

### 2. 分配角色给用户

```rust
// 分配开发者角色
rbac_engine.assign_role("user-123", "developer", None)?;

// 分配管理员角色（由其他管理员分配）
rbac_engine.assign_role(
    "user-456",
    "admin",
    Some("admin-001".to_string())
)?;
```

### 3. 检查权限

```rust
// 检查基本权限
if rbac_engine.check_permission("user-123", PermissionFlags::FILE_WRITE)? {
    // 允许写文件
    write_file(path, content)?;
} else {
    return Err(RbacError::PermissionDenied("No write access".to_string()));
}

// 检查上下文权限
let context = PermissionContext::new("file", "write")
    .with_resource_id("/src/main.rs");

if rbac_engine.check_context_permission("user-123", &context)? {
    // 允许操作
}
```

### 4. 自定义角色

```rust
let mut custom_role = Role::new(
    "senior-dev",
    "Senior Developer",
    "Extended development permissions"
);

custom_role.add_permissions(
    PermissionFlags::FILE_READ |
    PermissionFlags::FILE_WRITE |
    PermissionFlags::CODE_REFACTOR |
    PermissionFlags::AI_DEPLOY
);

rbac_engine.register_role(custom_role);
```

---

## 审计日志与 GDPR 合规

### 1. 配置审计日志

```rust
use jcode_auth::audit::*;

let config = AuditConfig {
    enabled: true,
    retention_days: 90,        // 保留90天
    max_events: 100000,        // 最大事件数
    log_pii: false,            // 不记录 PII（生产环境）
    export_format: ExportFormat::Json,
    gdpr_compliance: true,     // 启用 GDPR 合规
};

let storage = Arc::new(InMemoryAuditStorage::new(100000));
let audit_logger = Arc::new(AuditLogger::new(config, storage));
```

### 2. 记录审计事件

```rust
// 登录事件
let login_event = AuditEvent::new(
    AuditEventType::LoginSuccess,
    "user_login"
)
.with_user("user-123")
.with_session("session-abc")
.with_metadata("ip_address", serde_json::json!("192.168.1.100"));

audit_logger.log_event(login_event).await?;

// 权限拒绝事件
let denial_event = AuditEvent::new(
    AuditEventType::PermissionDenied,
    "file_access_denied"
)
.with_user("user-123")
.with_severity(AuditSeverity::Warning)
.with_metadata("resource", serde_json::json!("/etc/passwd"));

audit_logger.log_event(denial_event).await?;
```

### 3. GDPR 同意管理

```rust
// 记录用户同意
let consent = GdprConsent {
    user_id: "user-123".to_string(),
    consent_type: GdprConsentType::DataProcessing,
    granted: true,
    timestamp: chrono::Utc::now(),
    ip_address: Some("192.168.1.100".to_string()),
    user_agent: None,
    withdrawal_timestamp: None,
};

audit_logger.record_consent(consent).await?;

// 检查是否有同意
if audit_logger.has_consent("user-123", GdprConsentType::DataProcessing).await? {
    // 可以处理用户数据
} else {
    // 需要获取同意
}
```

### 4. 数据保留策略

```rust
// 强制执行保留策略（删除旧事件）
let deleted_count = audit_logger.enforce_retention().await?;
println!("Deleted {} old audit events", deleted_count);

// 处理 GDPR 删除请求（被遗忘权）
audit_logger.process_deletion_request("user-123").await?;
```

### 5. 查询和导出审计日志

```rust
// 查询特定用户的事件
let filter = AuditQueryFilter {
    user_id: Some("user-123".to_string()),
    start_time: Some(chrono::Utc::now() - chrono::Duration::days(7)),
    event_types: Some(vec![
        AuditEventType::LoginSuccess,
        AuditEventType::PermissionDenied,
    ]),
    ..Default::default()
};

let events = audit_logger.query_events(&filter).await?;

// 导出为 JSON
let exported = audit_logger.export_logs(&filter).await?;
std::fs::write("audit_export.json", exported)?;
```

---

## 数据加密

### 1. 初始化加密管理器

```rust
use jcode_auth::encryption::*;

// 生成主密钥
let master_key = EncryptionKey::generate_random(Some("master-key".to_string()))?;
let encryption_manager = EncryptionManager::new(master_key);
```

### 2. 加密敏感数据

```rust
// 加密字符串
let sensitive_data = "API key: sk-1234567890";
let encrypted = helpers::encrypt_string(&encryption_manager, sensitive_data)?;

println!("Encrypted: {}", encrypted.ciphertext);
println!("Algorithm: {}", encrypted.algorithm);
```

### 3. 解密数据

```rust
let decrypted = helpers::decrypt_string(&encryption_manager, &encrypted)?;
assert_eq!(sensitive_data, decrypted);
```

### 4. 密钥轮换

```rust
// 轮换密钥（旧数据仍可解密）
let new_key_id = encryption_manager.rotate_key()?;
println!("New active key: {}", new_key_id);

// 新加密使用新密钥
let new_encrypted = helpers::encrypt_string(&encryption_manager, "new data")?;
assert_eq!(new_encrypted.key_id, Some(new_key_id.clone()));
```

### 5. 密码哈希

```rust
// 哈希密码（用于存储）
let password = "user_password_123";
let salt = b"random_salt_value";
let hash = helpers::hash_password(password, salt)?;

// 验证密码
let is_valid = helpers::verify_password(password, salt, &hash)?;
assert!(is_valid);
```

---

## 完整集成示例

以下是在 CarpAI Server 中集成所有企业级功能的完整示例：

```rust
use std::sync::Arc;
use jcode_auth::*;

pub struct EnterpriseSecurityContext {
    pub oauth_provider: oauth::StandardOAuthProvider,
    pub jwt_manager: Arc<jwt::JwtManager>,
    pub rbac_engine: Arc<rbac::RbacEngine>,
    pub audit_logger: Arc<audit::AuditLogger>,
    pub encryption_manager: encryption::EncryptionManager,
}

impl EnterpriseSecurityContext {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // 1. OAuth2
        let oauth_config = oauth::ProviderType::GitHub
            .default_config("client_id", "client_secret");
        let oauth_provider = oauth::StandardOAuthProvider::new(oauth_config)?;

        // 2. JWT
        let jwt_manager = Arc::new(jwt::JwtManager::new_hs256(
            std::env::var("JWT_SECRET")?.as_bytes(),
            "carpai".to_string(),
            24,
        )?);

        // 3. RBAC
        let rbac_engine = Arc::new(rbac::RbacEngine::new());

        // 4. Audit
        let audit_config = audit::AuditConfig::default();
        let audit_storage = Arc::new(audit::InMemoryAuditStorage::new(100000));
        let audit_logger = Arc::new(audit::AuditLogger::new(audit_config, audit_storage));

        // 5. Encryption
        let enc_key = encryption::EncryptionKey::generate_random(None)?;
        let encryption_manager = encryption::EncryptionManager::new(enc_key);

        Ok(Self {
            oauth_provider,
            jwt_manager,
            rbac_engine,
            audit_logger,
            encryption_manager,
        })
    }

    /// 完整的认证和授权流程
    pub async fn authenticate_and_authorize(
        &self,
        oauth_code: &str,
        pkce_verifier: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // 1. Exchange OAuth code for token
        let oauth_token = self.oauth_provider
            .exchange_code(oauth_code.to_string(), pkce_verifier.to_string())
            .await?;

        // 2. Extract user info (implementation depends on provider)
        let user_info = self.oauth_provider
            .validate_token(&oauth_token.access_token)
            .await?;

        let user_id = user_info.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // 3. Assign default role if not exists
        self.rbac_engine.assign_role(&user_id, "developer", None)?;

        // 4. Generate JWT session token
        let roles = vec!["developer".to_string()];
        let session_token = jwt::helpers::generate_access_token(
            &self.jwt_manager,
            &user_id,
            roles,
        )?;

        // 5. Log authentication event
        let auth_event = audit::AuditEvent::new(
            audit::AuditEventType::LoginSuccess,
            "oauth_login"
        )
        .with_user(&user_id)
        .with_metadata("provider", serde_json::json!("github"));

        self.audit_logger.log_event(auth_event).await?;

        Ok(session_token)
    }

    /// 检查权限并记录审计事件
    pub async fn check_permission_with_audit(
        &self,
        user_id: &str,
        permission: rbac::PermissionFlags,
        resource: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let has_permission = self.rbac_engine.check_permission(user_id, permission)?;

        // 记录审计事件
        let event_type = if has_permission {
            audit::AuditEventType::PermissionGranted
        } else {
            audit::AuditEventType::PermissionDenied
        };

        let event = audit::AuditEvent::new(event_type, resource)
            .with_user(user_id)
            .with_metadata("permission", serde_json::json!(format!("{:?}", permission)))
            .with_metadata("resource", serde_json::json!(resource));

        self.audit_logger.log_event(event).await?;

        Ok(has_permission)
    }
}
```

---

## 生产环境部署

### 环境变量配置

```bash
# JWT 配置
export JWT_SECRET="your_256_bit_secret_key_here"
export JWT_EXPIRATION_HOURS=24

# OAuth2 配置
export OAUTH_CLIENT_ID="your_oauth_client_id"
export OAUTH_CLIENT_SECRET="your_oauth_client_secret"
export OAUTH_REDIRECT_URI="https://your-domain.com/oauth/callback"

# 审计日志配置
export AUDIT_RETENTION_DAYS=90
export AUDIT_MAX_EVENTS=1000000
export AUDIT_LOG_PII=false

# 加密配置
export ENCRYPTION_KEY_ROTATION_DAYS=30
```

### Docker 部署

```dockerfile
FROM rust:1.75 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --package carpai

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y openssl ca-certificates

COPY --from=builder /app/target/release/carpai /usr/local/bin/

# 非 root 用户运行
RUN useradd -m carpai
USER carpai

EXPOSE 8080

CMD ["carpai", "serve"]
```

### Kubernetes Secret 管理

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: carpai-secrets
type: Opaque
data:
  jwt-secret: <base64-encoded-secret>
  oauth-client-id: <base64-encoded-client-id>
  oauth-client-secret: <base64-encoded-client-secret>
  encryption-master-key: <base64-encoded-key>
```

---

## API 参考

### OAuth2

- `OAuthProvider::get_authorization_url()` - 生成授权 URL
- `OAuthProvider::exchange_code()` - 交换授权码
- `OAuthProvider::refresh_token()` - 刷新令牌
- `OAuthProvider::validate_token()` - 验证令牌

### JWT

- `JwtManager::generate_token()` - 生成 JWT
- `JwtManager::validate_token()` - 验证 JWT
- `JwtManager::refresh_token()` - 刷新 JWT
- `jwt::helpers::generate_access_token()` - 生成访问令牌
- `jwt::helpers::generate_refresh_token()` - 生成刷新令牌

### RBAC

- `RbacEngine::register_role()` - 注册角色
- `RbacEngine::assign_role()` - 分配角色
- `RbacEngine::check_permission()` - 检查权限
- `RbacEngine::get_user_roles()` - 获取用户角色

### Audit

- `AuditLogger::log_event()` - 记录事件
- `AuditLogger::query_events()` - 查询事件
- `AuditLogger::record_consent()` - 记录同意
- `AuditLogger::enforce_retention()` - 执行保留策略
- `AuditLogger::process_deletion_request()` - 处理删除请求

### Encryption

- `EncryptionManager::encrypt()` - 加密数据
- `EncryptionManager::decrypt()` - 解密数据
- `EncryptionManager::rotate_key()` - 轮换密钥
- `helpers::encrypt_string()` - 加密字符串
- `helpers::hash_password()` - 哈希密码

---

## 最佳实践

1. **密钥管理**
   - 永远不要硬编码密钥
   - 使用环境变量或密钥管理服务
   - 定期轮换密钥（建议每30天）

2. **最小权限原则**
   - 为用户分配最小必要权限
   - 使用角色继承简化权限管理
   - 定期审计权限分配

3. **审计日志**
   - 记录所有安全相关事件
   - 不要在日志中存储 PII
   - 实施适当的保留策略

4. **GDPR 合规**
   - 在处理数据前获取用户同意
   - 提供数据删除机制
   - 支持数据导出请求

5. **加密**
   - 对所有敏感数据进行加密
   - 使用强密钥（至少256位）
   - 实施密钥轮换策略

---

## 故障排除

### JWT Token 验证失败

```rust
let validation = jwt_manager.validate_token(&token)?;
if !validation.is_valid {
    eprintln!("Token error: {:?}", validation.error);
    // 常见原因：
    // - Token 过期
    // - Issuer 不匹配
    // - 签名无效
}
```

### RBAC 权限检查失败

```rust
// 调试：查看用户的所有角色
let roles = rbac_engine.get_user_roles(user_id);
for role in roles {
    println!("Role: {} - Permissions: {:?}", role.name, role.permissions);
}
```

### 审计日志查询为空

```rust
// 检查时间范围
let filter = AuditQueryFilter {
    start_time: Some(chrono::Utc::now() - chrono::Duration::days(30)),
    end_time: Some(chrono::Utc::now()),
    ..Default::default()
};
```

---

## 支持与贡献

如有问题或建议，请提交 Issue 或 Pull Request。

**许可证**: MIT
