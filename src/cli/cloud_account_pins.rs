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

type SessionProofs = std::collections::BTreeMap<String, Vec<CloudAuthBaseline>>;

/// Capture once: these exact bytes supply both the proof and the private transfer.
pub(super) fn capture_credentials(
    sessions: &[session::Session],
    accounts: &[CloudAccount],
) -> Result<(Vec<CloudCredentialTransfer>, SessionProofs)> {
    let mut captured: Vec<(String, CloudCredentialTransfer)> = Vec::new();
    let mut proofs = std::collections::BTreeMap::new();
    for sess in sessions {
        let active = oauth_selection(sess)?.map(|(provider, _)| provider);
        let mut session_proofs = Vec::new();
        for (family, provider) in [
            ("claude", TransferProvider::Claude),
            ("openai", TransferProvider::OpenAi),
        ] {
            let Some(pin) = sess.account_pins.get(family) else {
                continue;
            };
            let snapshot = (|| -> Result<_> {
                let pin = match provider {
                    TransferProvider::Claude => account_pins::normalize_claude_pin(pin)?,
                    TransferProvider::OpenAi => account_pins::normalize_openai_pin(pin)?,
                };
                let payload = account_pins::export_cloud_pin(provider, &pin)?;
                Ok((
                    pin.record_id
                        .context("saved cloud account has no local record identity")?,
                    payload,
                ))
            })();
            let (record_id, payload) = match snapshot {
                Ok(snapshot) => snapshot,
                Err(_) if active != Some(provider) => continue,
                Err(error) => return Err(error),
            };
            let index = if let Some(index) = captured.iter().position(|(_, saved)| {
                saved.provider() == provider && saved.label() == payload.label()
            }) {
                anyhow::ensure!(
                    captured[index].0 == record_id,
                    "source account changed during credential capture; retry migration"
                );
                index
            } else {
                captured.push((record_id, payload));
                captured.len() - 1
            };
            let payload = &captured[index].1;
            session_proofs.push(CloudAuthBaseline {
                account: CloudAccount {
                    provider,
                    label: payload.label().into(),
                },
                fingerprint: credential_fingerprint(payload),
            });
        }
        proofs.insert(sess.id.clone(), session_proofs);
    }
    let mut payloads: Vec<_> = captured.into_iter().map(|(_, payload)| payload).collect();
    for account in accounts {
        if !payloads
            .iter()
            .any(|p| p.provider() == account.provider && p.label() == account.label)
        {
            payloads.push(transfer::export_cloud_account_at(
                &storage::jcode_dir()?,
                account.provider,
                Some(&account.label),
            )?);
        }
    }
    Ok((payloads, proofs))
}

pub(super) fn account_pin_proofs(sess: &session::Session) -> Result<Vec<CloudAuthBaseline>> {
    let (_, mut proofs) = capture_credentials(std::slice::from_ref(sess), &[])?;
    proofs
        .remove(&sess.id)
        .context("missing captured session proofs")
}

pub(super) fn set_pin_proofs(manifest: &mut MoveManifest, proofs: Vec<CloudAuthBaseline>) {
    if let Some(account) = &mut manifest.auth_account
        && let Some(proof) = proofs
            .iter()
            .find(|p| p.account.provider == account.provider)
    {
        *account = proof.account.clone();
    }
    manifest.auth_pin_proofs = proofs;
}

pub(super) fn send_credentials(
    target: &Target,
    host: &str,
    payload: &CloudCredentialTransfer,
) -> Result<CloudAuthBaseline> {
    let account = CloudAccount {
        provider: payload.provider(),
        label: payload.label().into(),
    };
    retry_ssh(host, "credential transfer", || {
        target.exchange_credentials(host, &account, Some(payload.as_bytes()), None)
    })?;
    Ok(CloudAuthBaseline {
        account,
        fingerprint: credential_fingerprint(payload),
    })
}

pub(super) fn rebind_account_pins(
    sess: &mut session::Session,
    proofs: &[CloudAuthBaseline],
) -> Result<()> {
    let active = oauth_selection(sess)?.map(|(provider, _)| provider);
    let mut pins = sess.account_pins.clone();
    for (family, provider) in [
        ("claude", TransferProvider::Claude),
        ("openai", TransferProvider::OpenAi),
    ] {
        let Some(pin) = pins.get(family) else {
            continue;
        };
        let required = active == Some(provider);
        if !required
            && !proofs
                .iter()
                .any(|proof| proof.account.provider == provider)
        {
            continue;
        }
        let binding = (|| -> Result<_> {
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
                    account_pins::rebind_cloud_pin(
                        provider,
                        &proof.account.label,
                        &proof.fingerprint,
                    )?
                }
            };
            Ok(local)
        })();
        match binding {
            Ok(local) => {
                pins.insert(family.into(), local);
            }
            Err(_) if !required => continue,
            Err(error) => return Err(error),
        }
    }
    sess.account_pins = pins;
    Ok(())
}

pub(super) fn prepare_return_session(
    target: &Target,
    host: &str,
    manifest: &MoveManifest,
    stage: &Path,
    keep_local_auth: bool,
) -> Result<session::Session> {
    let mut sess: session::Session = read_json(&stage.join("session.json"))?;
    anyhow::ensure!(
        sess.id == manifest.session_id,
        "returned transcript has the wrong session identity"
    );
    if keep_local_auth {
        step(
            "credentials",
            "keeping local OAuth credentials and session account selection",
        );
        let local = session::Session::load(&sess.id)?;
        sess.inherit_provider_route(&local);
        sess.model = local.model;
        sess.reasoning_effort = local.reasoning_effort;
        return Ok(sess);
    }
    let departure_path = baseline_path(&sess.id)?;
    let accounts = if departure_path.exists() {
        let value: serde_json::Value = read_json(&departure_path)?;
        let baselines = if value.is_array() {
            serde_json::from_value::<Vec<CloudAuthBaseline>>(value)?
        } else {
            vec![serde_json::from_value::<CloudAuthBaseline>(value)?]
        };
        baselines.into_iter().map(|b| b.account).collect::<Vec<_>>()
    } else if manifest.auth_accounts.is_empty() {
        manifest.auth_account.iter().cloned().collect()
    } else {
        manifest.auth_accounts.clone()
    };
    let mut returned = Vec::new();
    for account in accounts {
        step(
            "credentials",
            "bringing refreshed OAuth credentials back securely",
        );
        let family = match account.provider {
            TransferProvider::Claude => "claude",
            TransferProvider::OpenAi => "openai",
        };
        let pin = sess.account_pins.get(family).filter(|pin| {
            pin.label == account.label
                || manifest
                    .auth_pin_proofs
                    .iter()
                    .any(|hint| hint.account == account)
        });
        let expected_pin = pin.map(serde_json::to_string).transpose()?;
        let bytes = retry_ssh(host, "credential return", || {
            target.exchange_credentials(host, &account, None, expected_pin.as_deref())
        })?;
        import_return_credentials(&sess.id, &account, &bytes)?;
        returned.push(CloudAuthBaseline {
            account,
            fingerprint: format!("{:x}", Sha256::digest(&bytes)),
        });
    }
    // Source-side pin validation connects fresh return bytes to the archived account.
    let proofs = manifest
        .auth_pin_proofs
        .iter()
        .filter_map(|hint| returned.iter().find(|proof| proof.account == hint.account))
        .cloned()
        .collect::<Vec<_>>();
    rebind_account_pins(&mut sess, &proofs)?;
    Ok(sess)
}

#[cfg(test)]
#[path = "cloud_account_pins_tests.rs"]
mod tests;
