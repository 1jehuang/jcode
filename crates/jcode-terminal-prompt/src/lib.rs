//! Terminal login presentation for jcode clients.
//!
//! Masked secret entry and Unicode QR rendering are interactive terminal
//! concerns, so they live here rather than in the shared runtime. The CLI wires
//! them into `jcode_base::auth::login_ui` at startup. Test with
//! `justrust test -p jcode-terminal-prompt`.

pub mod qr;
pub mod secret;

pub use secret::read_secret_line;

/// Indented QR section for the CLI login presenter. Shown automatically when
/// no local browser will open, otherwise only with `JCODE_SHOW_LOGIN_QR`.
pub use qr::indented_section as qr_indented_section;
