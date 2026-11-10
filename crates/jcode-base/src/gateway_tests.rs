use super::*;
use tokio_tungstenite::tungstenite::handshake::server::Request;

/// `DeviceRegistry` persists every pairing change to `$JCODE_HOME/devices.json`
/// (default `~/.jcode`). Without this, registry tests paired a fake
/// "Test iPhone" into the developer's real device registry on every run.
struct TempHome {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
}

impl TempHome {
    fn new() -> Self {
        let lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("temp dir");
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        Self {
            _lock: lock,
            previous,
            _temp: temp,
        }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

#[test]
fn test_device_registry_pairing() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();

    // Generate pairing code
    let code = registry.generate_pairing_code();
    assert_eq!(code.len(), 6);
    assert_eq!(registry.pending_codes.len(), 1);

    // Validate correct code
    assert!(registry.validate_code(&code));
    assert_eq!(registry.pending_codes.len(), 0); // consumed

    // Validate again should fail (consumed)
    assert!(!registry.validate_code(&code));
}

/// A different 6-digit code than `code`, so the guess is always wrong.
fn wrong_code(code: &str) -> String {
    let n: u32 = code.parse().unwrap();
    format!("{:06}", (n + 1) % 1_000_000)
}

#[test]
fn failed_pairing_guesses_revoke_pending_codes_at_the_cap() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();
    let code = registry.generate_pairing_code();

    // The gateway reloads the registry from disk for every /pair request, so
    // each simulated guess must too: the counter has to survive on disk.
    for attempt in 1..=registry::MAX_FAILED_PAIRING_ATTEMPTS {
        let mut per_request = DeviceRegistry::load();
        assert!(!per_request.validate_code(&wrong_code(&code)));
        let stored = DeviceRegistry::load();
        if attempt < registry::MAX_FAILED_PAIRING_ATTEMPTS {
            assert_eq!(stored.failed_pairing_attempts, attempt);
            assert!(!stored.pending_codes.is_empty());
        }
    }
    let mut registry = DeviceRegistry::load();
    assert!(registry.pending_codes.is_empty());
    // The real code no longer works: enumeration cannot outlast the cap.
    assert!(!registry.validate_code(&code));

    // The counter survives the reload the gateway does on every request.
    let reloaded = DeviceRegistry::load();
    assert!(reloaded.pending_codes.is_empty());

    // A freshly issued code resets the budget and works.
    let fresh = registry.generate_pairing_code();
    assert_eq!(registry.failed_pairing_attempts, 0);
    assert!(registry.validate_code(&fresh));
}

#[test]
fn a_few_typos_still_allow_pairing() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();
    let code = registry.generate_pairing_code();

    for _ in 1..registry::MAX_FAILED_PAIRING_ATTEMPTS {
        assert!(!registry.validate_code(&wrong_code(&code)));
    }
    let mut reloaded = DeviceRegistry::load();
    assert!(reloaded.validate_code(&code));
    assert_eq!(reloaded.failed_pairing_attempts, 0);
}

/// If `devices.json` is readable but not writable, the miss counter cannot
/// persist across the gateway's per-request reloads. Pairing must then fail
/// closed instead of letting a peer guess without limit.
#[cfg(unix)]
#[test]
fn pairing_fails_closed_when_registry_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;

    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();
    let code = registry.generate_pairing_code();
    let path = crate::storage::jcode_dir().unwrap().join("devices.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
        // Running as root: permissions cannot make the file unwritable.
        return;
    }

    for _ in 0..=registry::MAX_FAILED_PAIRING_ATTEMPTS {
        assert!(!DeviceRegistry::load().validate_code(&wrong_code(&code)));
    }
    assert!(
        !DeviceRegistry::load().validate_code(&code),
        "a correct code must not pair when its consumption cannot be saved"
    );

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn guesses_without_pending_codes_are_not_counted() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();
    assert!(!registry.validate_code("123456"));
    assert_eq!(registry.failed_pairing_attempts, 0);
}

#[test]
fn test_device_registry_token_auth() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();

    // Pair a device
    let token = registry.pair_device("test-device-1".to_string(), "Test iPhone".to_string(), None);

    // Validate correct token
    assert!(registry.validate_token(&token).is_some());
    let device = registry.validate_token(&token).unwrap();
    assert_eq!(device.name, "Test iPhone");
    assert_eq!(device.id, "test-device-1");

    // Validate wrong token
    assert!(registry.validate_token("wrong-token").is_none());

    // Token hash should be stored, not raw token
    assert!(registry.devices[0].token_hash.starts_with("sha256:"));
}

#[test]
fn test_device_re_pairing() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();

    // Pair same device twice
    let token1 = registry.pair_device("device-1".to_string(), "iPhone v1".to_string(), None);
    let token2 = registry.pair_device("device-1".to_string(), "iPhone v2".to_string(), None);

    // Only one device entry (old one replaced)
    assert_eq!(registry.devices.len(), 1);
    assert_eq!(registry.devices[0].name, "iPhone v2");

    // Old token should be invalid
    assert!(registry.validate_token(&token1).is_none());
    // New token should be valid
    assert!(registry.validate_token(&token2).is_some());
}

#[test]
fn test_parse_bearer_token() {
    assert_eq!(parse_bearer_token("Bearer abc"), Some("abc"));
    assert_eq!(parse_bearer_token("bearer abc"), Some("abc"));
    assert_eq!(parse_bearer_token("BEARER abc"), Some("abc"));
    assert_eq!(parse_bearer_token("Bearer"), None);
    assert_eq!(parse_bearer_token("Basic abc"), None);
    assert_eq!(parse_bearer_token("Bearer abc def"), None);
}

#[test]
fn test_parse_query_token() {
    assert_eq!(parse_query_token("token=abc"), Some("abc"));
    assert_eq!(parse_query_token("foo=bar&token=abc123"), Some("abc123"));
    assert_eq!(parse_query_token("token="), None);
    assert_eq!(parse_query_token("foo=bar"), None);
}

#[test]
fn test_hex_token_validation() {
    assert!(is_valid_hex_token(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    ));
    assert!(!is_valid_hex_token("abc"));
    assert!(!is_valid_hex_token(
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
    ));
}

#[test]
fn test_extract_ws_auth_prefers_header_and_falls_back_to_query() {
    let token_a = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let token_b = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    let header_request = Request::builder()
        .uri("ws://example.com/ws")
        .header("authorization", format!("Bearer {token_a}"))
        .body(())
        .expect("request");
    let header_auth = extract_ws_auth(&header_request).expect("header auth");
    assert_eq!(header_auth.token, token_a);
    assert_eq!(header_auth.source, WsAuthSource::Header);

    let query_request = Request::builder()
        .uri(format!("ws://example.com/ws?token={token_b}"))
        .body(())
        .expect("request");
    let query_auth = extract_ws_auth(&query_request).expect("query auth");
    assert_eq!(query_auth.token, token_b);
    assert_eq!(query_auth.source, WsAuthSource::Query);
}

#[test]
fn test_extract_ws_auth_rejects_conflicting_sources() {
    let token_a = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let token_b = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    let request = Request::builder()
        .uri(format!("ws://example.com/ws?token={token_b}"))
        .header("authorization", format!("Bearer {token_a}"))
        .body(())
        .expect("request");
    assert!(extract_ws_auth(&request).is_err());
}

#[test]
fn test_find_header_end() {
    assert_eq!(
        super::find_header_end(b"POST /pair HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}"),
        Some(38)
    );
    assert_eq!(
        super::find_header_end(b"POST /pair HTTP/1.1\r\nContent-"),
        None
    );
    assert_eq!(super::find_header_end(b""), None);
}

#[test]
fn test_authorize_ws_device_valid_token() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();
    let token = registry.pair_device("dev-1".to_string(), "iPhone".to_string(), None);

    let device = auth::authorize_ws_device(&registry, &token).expect("valid token authorizes");
    assert_eq!(device.name, "iPhone");
    assert_eq!(device.id, "dev-1");
}

#[test]
fn test_authorize_ws_device_rejects_unknown_and_revoked_with_401() {
    let _home = TempHome::new();
    let mut registry = DeviceRegistry::default();
    let token = registry.pair_device("dev-1".to_string(), "iPhone".to_string(), None);

    // Unknown token -> 401 at handshake time.
    let unknown = "a".repeat(64);
    let err =
        auth::authorize_ws_device(&registry, &unknown).expect_err("unknown token must be rejected");
    assert_eq!(err.status(), 401);
    assert!(
        err.body()
            .as_deref()
            .unwrap_or_default()
            .contains("re-pair"),
        "401 body should tell the client to re-pair"
    );

    // Revoked device -> same 401 path.
    registry.devices.retain(|d| d.id != "dev-1");
    let err =
        auth::authorize_ws_device(&registry, &token).expect_err("revoked token must be rejected");
    assert_eq!(err.status(), 401);
}
