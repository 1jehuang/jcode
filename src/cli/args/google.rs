//! Google-specific `jcode login` flags, flattened into `Command::Login`.

use clap::{Args, ValueEnum};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum GoogleAccessTierArg {
    Full,
    Readonly,
}

#[derive(Args, Debug, Clone, Default)]
pub(crate) struct GoogleLoginArgs {
    /// Gmail/Google access tier for non-interactive flows. Defaults to full.
    #[arg(long, value_enum)]
    pub google_access_tier: Option<GoogleAccessTierArg>,

    /// Google services to authorize, comma-separated: gmail, calendar, or all.
    /// Defaults to the services already granted, or an interactive prompt.
    #[arg(long, value_name = "SERVICES")]
    pub google_services: Option<String>,

    /// Re-run the guided Google Cloud setup (project, APIs, OAuth client) for Google login.
    #[arg(long)]
    pub setup: bool,

    /// Import a Google OAuth client JSON (downloaded from Cloud Console) before Google login.
    /// Pass `auto` to pick the newest client_secret*.json in Downloads.
    #[arg(long, value_name = "PATH|auto")]
    pub google_client_json: Option<String>,
}

impl GoogleLoginArgs {
    /// Copy these flags into login options, validating the service list.
    pub(crate) fn apply(self, options: &mut crate::cli::login::LoginOptions) -> anyhow::Result<()> {
        use crate::auth::google::{GmailAccessTier, GoogleService};
        options.google_access_tier = self.google_access_tier.map(|tier| match tier {
            GoogleAccessTierArg::Full => GmailAccessTier::Full,
            GoogleAccessTierArg::Readonly => GmailAccessTier::ReadOnly,
        });
        options.google_services = self
            .google_services
            .as_deref()
            .map(GoogleService::parse_list)
            .transpose()?;
        options.google_setup = self.setup;
        options.google_client_json = self.google_client_json;
        Ok(())
    }
}
