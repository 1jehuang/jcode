use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TranscriptModeArg {
    Insert,
    Append,
    Replace,
    Send,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum ProviderAuthArg {
    /// Send the API key as Authorization: Bearer <key> (OpenAI-compatible default)
    Bearer,
    /// Send the API key in an API-key header (defaults to api-key)
    ApiKey,
    /// Do not send authentication, useful for localhost model servers
    None,
}
