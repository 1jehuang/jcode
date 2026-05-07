use super::send_message_to_session;
use std::sync::Mutex;

fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
    unsafe {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
}

#[test]
fn send_message_spawn_error_redacts_session_id() {
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap();
    let previous_bin = std::env::var_os("JCODE_BIN");
    unsafe {
        std::env::set_var("JCODE_BIN", "/definitely/missing/jcode");
    }

    let err = send_message_to_session("session_sensitive_123", "title", "hello")
        .expect_err("missing jcode binary should fail");
    let message = format!("{err:#}");

    restore_env_var("JCODE_BIN", previous_bin);
    assert!(message.contains("[redacted]"));
    assert!(!message.contains("session_sensitive_123"));
}
