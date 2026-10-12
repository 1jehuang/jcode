use super::*;

#[test]
fn detects_termux_like_install_sh() {
    assert!(is_termux_env(Some("0.118"), None, false));
    assert!(is_termux_env(None, Some(TERMUX_PREFIX), false));
    assert!(is_termux_env(None, None, true));
    assert!(!is_termux_env(None, Some("/usr"), false));
    assert!(!is_termux_env(Some(""), None, false));
}

#[test]
fn picks_glibc_interpreter_per_arch() {
    assert_eq!(
        termux_glibc_interpreter("linux", "aarch64").as_deref(),
        Some("/data/data/com.termux/files/usr/glibc/lib/ld-linux-aarch64.so.1")
    );
    assert_eq!(
        termux_glibc_interpreter("linux", "x86_64").as_deref(),
        Some("/data/data/com.termux/files/usr/glibc/lib/ld-linux-x86-64.so.2")
    );
    assert_eq!(termux_glibc_interpreter("linux", "riscv64"), None);
    assert_eq!(termux_glibc_interpreter("macos", "aarch64"), None);
}
