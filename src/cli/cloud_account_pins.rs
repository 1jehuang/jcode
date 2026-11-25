use super::*;
use crate::auth::account_pins;

/// Never infer OAuth from a bare provider name: it may actually use an API key.
fn oauth_selection(sess: &session::Session) -> Result<Option<(TransferProvider, Option<&str>)>> {
    let route = sess.route_api_method.as_deref().or_else(|| {
        sess.model
            .as_deref()
            .and_then(|model| model.split_once(':').map(|(route, _)| route))
    });
    let Some(route) = route else { return Ok(None) };
    let method = route.split('@').next().unwrap_or(route);
    let provider = match method {
        "openai-oauth" => TransferProvider::OpenAi,
        "claude-oauth" => TransferProvider::Claude,
        _ => return Ok(None),
    };
    let label = crate::provider::oauth_account_label(route);
    if route.contains('@') && label.is_none() {
        bail!("session has an invalid saved OAuth account route");
    }
    Ok(Some((provider, label)))
}

pub(super) fn session_credentials(
    sess: &session::Session,
) -> Result<Option<CloudCredentialTransfer>> {
    let Some((provider, label)) = oauth_selection(sess)? else {
        return Ok(None);
    };
    let pin = sess.account_pins.get(match provider {
        TransferProvider::Claude => "claude",
        TransferProvider::OpenAi => "openai",
    });
    if let Some(pin) = pin {
        return Ok(Some(account_pins::export_cloud_pin(provider, pin)?));
    }
    Ok(Some(transfer::export_cloud_account_at(
        &storage::jcode_dir()?,
        provider,
        label,
    )?))
}

// Portable proofs are checked only at the handoff boundary; local UUIDs stay local.
pub(super) fn account_pin_proofs(sess: &session::Session) -> Result<Vec<CloudAuthBaseline>> {
    let mut proofs = Vec::new();
    for (family, provider) in [
        ("claude", TransferProvider::Claude),
        ("openai", TransferProvider::OpenAi),
    ] {
        let Some(pin) = sess.account_pins.get(family) else {
            continue;
        };
        let payload = account_pins::export_cloud_pin(provider, pin)?;
        let label = payload.label().to_owned();
        proofs.push(CloudAuthBaseline {
            account: CloudAccount { provider, label },
            fingerprint: credential_fingerprint(&payload),
        });
    }
    Ok(proofs)
}

pub(super) fn rebind_account_pins(
    sess: &mut session::Session,
    proofs: &[CloudAuthBaseline],
) -> Result<()> {
    let mut pins = sess.account_pins.clone();
    for (family, provider) in [
        ("claude", TransferProvider::Claude),
        ("openai", TransferProvider::OpenAi),
    ] {
        let Some(pin) = pins.get(family) else {
            continue;
        };
        let normalized = match provider {
            TransferProvider::Claude => account_pins::normalize_claude_pin(pin),
            TransferProvider::OpenAi => account_pins::normalize_openai_pin(pin),
        };
        let local = match normalized {
            Ok(local) => local,
            Err(_) => {
                let mut matching = proofs
                    .iter()
                    .filter(|proof| proof.account.provider == provider);
                let proof = matching
                    .next()
                    .context("incoming account pin has no credential proof")?;
                if matching.next().is_some() {
                    bail!("incoming account pin has ambiguous credential proofs");
                }
                account_pins::rebind_cloud_pin(provider, &proof.account.label, &proof.fingerprint)?
            }
        };
        pins.insert(family.into(), local);
    }
    sess.account_pins = pins;
    Ok(())
}

#[cfg(test)]
#[path = "cloud_account_pins_tests.rs"]
mod tests;
