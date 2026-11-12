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
