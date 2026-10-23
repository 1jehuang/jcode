//! Terminal presentation for shared authentication flows.

use super::*;

/// Terminal implementation of the shared auth flows' login presenter.
struct TerminalLoginPresenter;

impl crate::auth::login_ui::LoginPresenter for TerminalLoginPresenter {
    fn qr_section(
        &self,
        data: &str,
        heading: &str,
        indent: &str,
        browser_suppressed: bool,
    ) -> Option<String> {
        jcode_terminal_prompt::qr::indented_section(data, heading, indent, browser_suppressed)
    }

    fn read_secret_line(&self) -> Result<String> {
        jcode_terminal_prompt::read_secret_line()
    }
}

pub fn install_terminal_login_presenter() {
    crate::auth::login_ui::install(Box::new(TerminalLoginPresenter));
}

pub(super) fn read_line_trimmed(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}
