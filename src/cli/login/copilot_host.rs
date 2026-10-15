use super::*;

const PUBLIC_GITHUB_HOST: &str = "github.com";

pub(super) fn configure_login_host(
    provider: LoginProviderDescriptor,
    options: &LoginOptions,
) -> Result<()> {
    if let Some(host) = options.copilot_enterprise_url.as_deref() {
        anyhow::ensure!(
            provider.target == LoginProviderTarget::Copilot,
            "--enterprise-url / --github-host requires --provider copilot."
        );
        return save_login_host(host);
    }
    if provider.target != LoginProviderTarget::Copilot
        || options.uses_scriptable_flow()?
        || !io::stdin().is_terminal()
    {
        return Ok(());
    }

    let endpoints = auth::copilot::CopilotEndpoints::load()?;
    let is_enterprise = endpoints.host != PUBLIC_GITHUB_HOST;
    eprintln!("\nGitHub Copilot sign-in:");
    eprintln!("  1. Public GitHub (github.com)");
    eprintln!("  2. GitHub Enterprise (your organization's hostname)");
    let default = if is_enterprise { "2" } else { "1" };
    let choice = read_line_trimmed(&format!("Enter 1-2 [{default}]: "))?;
    let choice = if choice.is_empty() { default } else { &choice };
    match choice {
        "1" => save_login_host(PUBLIC_GITHUB_HOST),
        "2" => {
            let prompt = if is_enterprise {
                format!("GitHub Enterprise hostname [{}]: ", endpoints.host)
            } else {
                "GitHub Enterprise hostname (for example hiqfi.ghe.com): ".to_string()
            };
            let host = read_line_trimmed(&prompt)?;
            let host = if host.is_empty() && is_enterprise {
                endpoints.host.as_str()
            } else {
                host.as_str()
            };
            save_login_host(host)
        }
        _ => anyhow::bail!("Choose 1 for public GitHub or 2 for GitHub Enterprise."),
    }
}

fn save_login_host(host: &str) -> Result<()> {
    let host = normalize_login_host(host)?;
    crate::config::Config::set_copilot_enterprise_url(host.as_deref())
        .context("Could not save the GitHub Copilot login hostname")
}

pub(super) fn normalize_login_host(host: &str) -> Result<Option<String>> {
    let endpoints = auth::copilot::CopilotEndpoints::from_enterprise_url(Some(host.trim()))
        .context("Invalid --enterprise-url / --github-host")?;
    Ok((endpoints.host != PUBLIC_GITHUB_HOST).then_some(endpoints.host))
}
