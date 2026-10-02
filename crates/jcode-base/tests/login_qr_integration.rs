//! Exercise the real OpenAI login prompt in a child process, without completing
//! OAuth, opening a browser, or touching saved credentials.
use std::process::{Command, Stdio};

#[test]
fn openai_browserless_login_prints_qr() {
    if std::env::var_os("JCODE_QR_LOGIN_TEST_CHILD").is_some() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(jcode_base::auth::oauth::login_openai(true))
            .unwrap_err();
        assert!(error.to_string().contains("No callback URL entered"));
        return;
    }

    let home = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "openai_browserless_login_prints_qr",
            "--nocapture",
        ])
        .env("JCODE_QR_LOGIN_TEST_CHILD", "1")
        .env("JCODE_HOME", home.path())
        .env_remove("JCODE_SHOW_LOGIN_QR")
        .env_remove("JCODE_LOGIN_QR")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("https://auth.openai.com/"), "{stderr}");
    assert!(stderr.contains("Scan this QR"), "{stderr}");
    assert!(stderr.contains('█') || stderr.contains('▀') || stderr.contains('▄'));
    assert!(stderr.contains("Paste the full callback URL"), "{stderr}");
    assert!(!stderr.contains("Waiting up to"), "{stderr}");
}
