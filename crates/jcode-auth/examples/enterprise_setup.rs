//! Enterprise authentication and authorization setup example
//!
//! This example demonstrates how to configure and use the complete
//! enterprise security stack for CarpAI.

use jcode_auth::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== CarpAI Enterprise Security Setup ===\n");

    // 1. Setup OAuth2 Provider
    println!("1. Configuring OAuth2 provider...");
    let oauth_config = oauth::ProviderType::GitHub
        .default_config("your_client_id", "your_client_secret");

    let oauth_provider = oauth::StandardOAuthProvider::new(oauth_config)?;
    let (auth_url, _csrf, _verifier) = oauth_provider.get_authorization_url()?;
    println!("   Authorization URL: {}", auth_url);
    println!("   ✓ OAuth2 configured\n");

    // 2. Setup JWT Manager
    println!("2. Setting up JWT token management...");
    let jwt_secret = b"super_secret_jwt_key_for_enterprise";
    let jwt_manager = Arc::new(jwt::JwtManager::new_hs256(
        jwt_secret,
        "carpai-enterprise".to_string(),
        24, // 24 hour expiration
    )?);

    // Generate access token
    let access_token = jwt::helpers::generate_access_token(
        &jwt_manager,
        "user-123",
        vec!["developer".to_string()],
    )?;
    println!("   Access token generated: {}...", &access_token[..20]);

    // Validate token
    let validation = jwt_manager.validate_token(&access_token)?;
    println!("   Token valid: {}", validation.is_valid);
    println!("   User ID: {}", validation.claims.sub);
    println!("   ✓ JWT configured\n");

    // 3. Setup RBAC Engine
    println!("3. Initializing RBAC permission system...");
    let rbac_engine = Arc::new(rbac::RbacEngine::new());

    // Assign roles to user
    rbac_engine.assign_role("user-123", "developer", None)?;
    rbac_engine.assign_role("user-456", "admin", None)?;

    // Check permissions
    let has_read = rbac_engine.check_permission("user-123", rbac::PermissionFlags::FILE_READ)?;
    let has_admin = rbac_engine.check_permission("user-123", rbac::PermissionFlags::SYSTEM_ADMIN)?;

    println!("   User 'user-123' can read files: {}", has_read);
    println!("   User 'user-123' is admin: {}", has_admin);

    let admin_has_all = rbac_engine.check_permission("user-456", rbac::PermissionFlags::ALL)?;
    println!("   User 'user-456' has all permissions: {}", admin_has_all);
    println!("   ✓ RBAC configured\n");

    // 4. Setup Audit Logger
    println!("4. Configuring audit logging with GDPR compliance...");
    let audit_config = audit::AuditConfig {
        enabled: true,
        retention_days: 90,
        max_events: 10000,
        log_pii: false, // Don't log PII in production
        export_format: audit::ExportFormat::Json,
        gdpr_compliance: true,
    };

    let audit_storage = Arc::new(audit::InMemoryAuditStorage::new(10000));
    let audit_logger = Arc::new(audit::AuditLogger::new(audit_config, audit_storage));

    // Log some events
    let login_event = audit::AuditEvent::new(
        audit::AuditEventType::LoginSuccess,
        "user_login",
    )
    .with_user("user-123")
    .with_metadata("ip_address", serde_json::json!("192.168.1.100"));

    audit_logger.log_event(login_event).await?;

    let permission_event = audit::AuditEvent::new(
        audit::AuditEventType::PermissionGranted,
        "file_access_granted",
    )
    .with_user("user-123")
    .with_metadata("resource", serde_json::json!("/path/to/file.rs"));

    audit_logger.log_event(permission_event).await?;

    // Record GDPR consent
    let consent = audit::GdprConsent {
        user_id: "user-123".to_string(),
        consent_type: audit::GdprConsentType::DataProcessing,
        granted: true,
        timestamp: chrono::Utc::now(),
        ip_address: Some("192.168.1.100".to_string()),
        user_agent: None,
        withdrawal_timestamp: None,
    };

    audit_logger.record_consent(consent).await?;

    println!("   Audit events logged: 2");
    println!("   GDPR consent recorded for user-123");

    // Query events
    let filter = audit::AuditQueryFilter {
        user_id: Some("user-123".to_string()),
        ..Default::default()
    };

    let events = audit_logger.query_events(&filter).await?;
    println!("   Events for user-123: {}", events.len());
    println!("   ✓ Audit logging configured\n");

    // 5. Setup Encryption
    println!("5. Initializing encryption manager (AES-256-GCM)...");
    let encryption_key = encryption::EncryptionKey::generate_random(Some("master-key".to_string()))?;
    let encryption_manager = encryption::EncryptionManager::new(encryption_key);

    // Encrypt sensitive data
    let sensitive_data = "This is confidential user information";
    let encrypted = encryption::helpers::encrypt_string(&encryption_manager, sensitive_data)?;
    println!("   Data encrypted successfully");
    println!("   Algorithm: {}", encrypted.algorithm);

    // Decrypt
    let decrypted = encryption::helpers::decrypt_string(&encryption_manager, &encrypted)?;
    println!("   Data decrypted: {}", decrypted);
    assert_eq!(sensitive_data, decrypted);

    // Key rotation
    let new_key_id = encryption_manager.rotate_key()?;
    println!("   Key rotated: new key ID = {}", new_key_id);
    println!("   ✓ Encryption configured\n");

    // 6. Complete Security Workflow Example
    println!("6. Running complete security workflow...");

    // User logs in via OAuth
    println!("   a) User authenticates via OAuth2");
    let oauth_token = oauth::OAuthToken {
        access_token: "oauth_access_token_here".to_string(),
        refresh_token: Some("oauth_refresh_token".to_string()),
        token_type: "Bearer".to_string(),
        expires_in: Some(3600),
        scope: vec!["user:email".to_string()],
        id_token: None,
    };
    println!("      OAuth token received (expires in {:?}s)", oauth_token.expires_in);

    // Generate JWT session token
    println!("   b) Generating JWT session token");
    let session_token = jwt::helpers::generate_access_token(
        &jwt_manager,
        "user-123",
        vec!["developer".to_string(), "viewer".to_string()],
    )?;
    println!("      Session token: {}...", &session_token[..30]);

    // Check permissions before operation
    println!("   c) Checking permissions for file write operation");
    if rbac_engine.check_permission("user-123", rbac::PermissionFlags::FILE_WRITE)? {
        println!("      ✓ Permission granted");

        // Log the action
        let action_event = audit::AuditEvent::new(
            audit::AuditEventType::DataWrite,
            "file_write",
        )
        .with_user("user-123")
        .with_session("session-abc-123")
        .with_metadata("file_path", serde_json::json!("/src/main.rs"));

        audit_logger.log_event(action_event).await?;
        println!("      ✓ Action logged to audit trail");
    } else {
        println!("      ✗ Permission denied");

        // Log denial
        let denial_event = audit::AuditEvent::new(
            audit::AuditEventType::PermissionDenied,
            "file_write_denied",
        )
        .with_user("user-123")
        .with_severity(audit::AuditSeverity::Warning);

        audit_logger.log_event(denial_event).await?;
    }

    // Encrypt sensitive operation result
    println!("   d) Encrypting operation result");
    let operation_result = "Sensitive data from operation";
    let encrypted_result = encryption::helpers::encrypt_string(&encryption_manager, operation_result)?;
    println!("      Result encrypted and stored");

    println!("\n✓ Complete workflow executed successfully!\n");

    // Summary
    println!("=== Enterprise Security Features Enabled ===");
    println!("✓ OAuth2 Authentication (GitHub/Google/Azure AD)");
    println!("✓ JWT Session Management (HS256/RS256)");
    println!("✓ RBAC Permission System (4 predefined roles)");
    println!("✓ Audit Logging with GDPR Compliance");
    println!("✓ AES-256-GCM Encryption");
    println!("✓ Key Rotation Support");
    println!("✓ Consent Management");
    println!("✓ Data Retention Policies");
    println!("\nAll systems operational!");

    Ok(())
}
