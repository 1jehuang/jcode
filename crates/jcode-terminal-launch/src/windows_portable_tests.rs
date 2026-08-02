//! Windows spawn checks that run on every platform.
//!
//! The Windows tests in `lib.rs` are all `cfg(not(unix))`, so none of them run
//! in this repo's Linux CI or on a Linux dev box. That blind spot is what let
//! the in-app spawn stub sit unnoticed off Unix (#715). Anything checkable
//! without a Windows host belongs here instead.

use super::windows_command_line;

/// #715: the in-app resume spawn was a no-op off Unix, so the Windows
/// command line it *would* build was never exercised anywhere that runs.
///
/// The existing Windows coverage is `cfg(not(unix))`, so it never runs in
/// this repo's Linux CI or on a maintainer's Linux box. `windows_command_line`
/// is compiled under `test` on every platform, so pin the resume invocation
/// here where it will actually be checked.
#[test]
fn windows_command_line_quotes_a_resume_invocation_on_every_platform() {
    let line = windows_command_line(&[
        r"C:\Program Files\jcode\jcode.exe".to_string(),
        "--resume".to_string(),
        "session_penguin_123".to_string(),
    ]);

    // The exe path contains a space, so it must be quoted or cmd.exe would
    // treat "C:\Program" as the program.
    assert!(
        line.contains(r#""C:\Program Files\jcode\jcode.exe""#),
        "unquoted program path would break on a default Windows install: {line}"
    );
    assert!(line.contains("--resume"), "{line}");
    assert!(line.contains("session_penguin_123"), "{line}");
    // Session ids are safe, so they must not be needlessly quoted.
    assert!(
        !line.contains(r#""--resume""#),
        "quoting flags changes cmd.exe parsing: {line}"
    );
}
