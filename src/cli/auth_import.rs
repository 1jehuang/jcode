//! Private, bounded stdin receiver for explicitly approved credential transfers.
//! There is deliberately no CLI export command that could print credentials.
use super::provider_init::ProviderChoice;
use crate::auth::transfer::{self, MAX_TRANSFER_BYTES, TransferProvider};
use anyhow::Result;
use std::io::IsTerminal;

fn selected_provider(choice: &ProviderChoice) -> Result<TransferProvider, &'static str> {
    match choice {
        ProviderChoice::Openai => Ok(TransferProvider::OpenAi),
        ProviderChoice::Claude => Ok(TransferProvider::Claude),
        _ => Err("Credential import requires --provider openai or --provider claude"),
    }
}

// Tokio's stdin uses a blocking worker whose shutdown can hang after a timeout.
// Poll the pipe directly instead, bounding the lifetime even if its writer stalls.
#[cfg(unix)]
fn read_private_stdin() -> Result<Vec<u8>, &'static str> {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err("Credential import requires piped stdin, not terminal input");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Credential import input timed out");
        }
        let mut fd = libc::pollfd {
            fd: stdin.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd points to one initialized pollfd and lives through poll.
        let ready = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().min(30_000) as i32) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("Could not read credential import input");
        }
        if ready == 0 {
            return Err("Credential import input timed out");
        }
        let mut buffer = [0u8; 4096];
        // SAFETY: stdin is valid and buffer is writable for its entire length.
        // Use the raw fd rather than StdinLock, which can buffer bytes that a
        // subsequent poll would not see.
        let count =
            unsafe { libc::read(stdin.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err("Could not read credential import input");
        }
        let count = count as usize;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len() + count > MAX_TRANSFER_BYTES {
            return Err("Credential import input is too large");
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(not(unix))]
fn read_private_stdin() -> Result<Vec<u8>, &'static str> {
    Err("Native SSH credential import is supported on Unix hosts")
}

pub(crate) fn run(choice: &ProviderChoice, json: bool) -> Result<()> {
    let provider = selected_provider(choice);
    let provider_id = provider
        .as_ref()
        .map(|p| p.as_str())
        .unwrap_or("unsupported");
    let outcome = provider.and_then(|provider| {
        let payload = read_private_stdin()?;
        transfer::import_local(provider, &payload).map_err(|error| error.message())
    });
    match outcome {
        Ok(()) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"status":"imported", "provider":provider_id})
                );
            } else {
                println!(
                    "Imported {provider_id} credentials. This is a one-time copy, not synchronization."
                );
            }
            Ok(())
        }
        Err(message) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"status":"error", "provider":provider_id, "message":message})
                );
            }
            anyhow::bail!("{message}")
        }
    }
}

pub(crate) fn run_opencodex(json: bool) -> Result<()> {
    let result = crate::auth::provider_pool::import_opencodex();
    match result {
        Ok(summary) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "imported",
                        "cursor_accounts": summary.cursor_imported,
                        "antigravity_accounts": summary.antigravity_imported,
                    })
                );
            } else {
                println!(
                    "Imported {} Cursor and {} Antigravity accounts from ~/.opencodex.",
                    summary.cursor_imported, summary.antigravity_imported
                );
            }
            Ok(())
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"status":"error", "message": error.to_string()})
                );
            }
            Err(error)
        }
    }
}

#[derive(serde::Serialize)]
struct AccountView {
    provider: &'static str,
    id: String,
    label: String,
    active: bool,
    on_cooldown: bool,
    expires_at: i64,
}

pub(crate) fn run_accounts(provider: &str, switch: Option<&str>, json: bool) -> Result<()> {
    let provider = provider.trim().to_ascii_lowercase();
    let providers: Vec<&'static str> = match provider.as_str() {
        "all" => vec!["openai", "cursor", "antigravity"],
        "openai" | "cursor" | "antigravity" => vec![match provider.as_str() {
            "openai" => "openai",
            "cursor" => "cursor",
            _ => "antigravity",
        }],
        _ => anyhow::bail!(
            "Unsupported account-pool provider '{provider}'. Use openai, cursor, antigravity, or all."
        ),
    };
    if switch.is_some() && providers.len() != 1 {
        anyhow::bail!("--switch requires an explicit provider, not 'all'");
    }

    if let Some(label) = switch.map(str::trim).filter(|label| !label.is_empty()) {
        match providers[0] {
            "openai" => crate::auth::codex::set_active_account(label)?,
            "cursor" | "antigravity" => {
                crate::auth::provider_pool::set_active_account(providers[0], label)?
            }
            _ => unreachable!(),
        }
    }

    let mut views = Vec::new();
    for provider in providers {
        match provider {
            "openai" => {
                let active = crate::auth::codex::active_account_label();
                for account in crate::auth::codex::list_accounts().unwrap_or_default() {
                    views.push(AccountView {
                        provider,
                        id: account
                            .account_id
                            .clone()
                            .unwrap_or_else(|| account.label.clone()),
                        label: account.label.clone(),
                        active: active.as_deref() == Some(account.label.as_str()),
                        on_cooldown: false,
                        expires_at: account.expires_at.unwrap_or_default(),
                    });
                }
            }
            "cursor" | "antigravity" => {
                let active = crate::auth::provider_pool::active_account(provider)?
                    .map(|account| account.label);
                for account in crate::auth::provider_pool::list_accounts(provider)? {
                    views.push(AccountView {
                        provider,
                        id: account.id,
                        label: account.label.clone(),
                        active: active.as_deref() == Some(account.label.as_str()),
                        on_cooldown: crate::auth::provider_pool::account_on_cooldown(
                            provider,
                            &account.label,
                        ),
                        expires_at: account.expires_at,
                    });
                }
            }
            _ => unreachable!(),
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&views)?);
    } else {
        for account in &views {
            println!(
                "{}\t{}\t{}{}",
                account.provider,
                account.label,
                if account.active {
                    "active"
                } else {
                    "available"
                },
                if account.on_cooldown {
                    " (cooldown)"
                } else {
                    ""
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_import_requires_selected_supported_oauth_provider() {
        assert!(selected_provider(&ProviderChoice::Openai).is_ok());
        assert!(selected_provider(&ProviderChoice::Claude).is_ok());
        for provider in [
            ProviderChoice::Auto,
            ProviderChoice::OpenaiApi,
            ProviderChoice::Gemini,
        ] {
            assert!(selected_provider(&provider).is_err());
        }
    }
}
