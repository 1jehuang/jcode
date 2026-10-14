//! Client-supplied presentation for interactive login flows.
//!
//! The OAuth flows in this crate run in the CLI, but they live in the shared
//! foundation so every client can reuse token exchange and storage. Rendering a
//! QR code or reading a masked secret from a terminal is the client's job, so
//! the flows go through this hook. The CLI installs a terminal presenter at
//! startup (`jcode-terminal-prompt`). Without one, flows print no QR and read a
//! plain line from stdin, which is what a piped or headless caller wants.

use anyhow::Result;
use std::sync::OnceLock;

pub trait LoginPresenter: Send + Sync {
    /// A heading plus a scannable rendering of `data`, indented by `indent`.
    /// `browser_suppressed` is true when no local browser will open, which is
    /// when a QR is most useful. Return `None` to show no QR.
    fn qr_section(
        &self,
        data: &str,
        heading: &str,
        indent: &str,
        browser_suppressed: bool,
    ) -> Option<String>;

    /// Read one line of secret input (authorization code, callback URL).
    fn read_secret_line(&self) -> Result<String>;
}

static PRESENTER: OnceLock<Box<dyn LoginPresenter>> = OnceLock::new();

/// Install the process-wide presenter. Later calls are ignored.
pub fn install(presenter: Box<dyn LoginPresenter>) {
    let _ = PRESENTER.set(presenter);
}

pub fn qr_section(
    data: &str,
    heading: &str,
    indent: &str,
    browser_suppressed: bool,
) -> Option<String> {
    PRESENTER
        .get()?
        .qr_section(data, heading, indent, browser_suppressed)
}

pub fn read_secret_line() -> Result<String> {
    match PRESENTER.get() {
        Some(presenter) => presenter.read_secret_line(),
        None => {
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            Ok(input.trim().to_string())
        }
    }
}
