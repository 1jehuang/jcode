use super::*;

pub(super) fn login_openai_api_key_flow() -> Result<()> {
    eprintln!("Setting up OpenAI API key...");
    eprintln!("Get your API key from: https://platform.openai.com/api-keys\n");
    eprint!("Paste your OpenAI API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }
    if !key.starts_with("sk-") {
        eprintln!("Warning: OpenAI API keys usually start with 'sk-'. Saving anyway.");
    }

    save_named_api_key("openai.env", "OPENAI_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved OpenAI API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openai.env")
            .display()
    );
    eprintln!("Provider: openai-api (native OpenAI Responses API)");
    crate::telemetry::record_auth_success("openai-api", "api_key");
    Ok(())
}
