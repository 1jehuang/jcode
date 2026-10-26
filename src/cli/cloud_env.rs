//! Private environment transfer. This module is a child of cloud_move so the
//! existing authenticated SSH target implementation remains the single transport.
use super::*;
use crate::cloud_environment::{self as env, Request, Resource, Status};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u32,
    source: Source,
    root: String,
    requirements: Vec<Resource>,
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Source {
    LocalPush {},
    Handoff { session: String, epoch: u64 },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    resource: Resource,
    /// Credential slot only. Never a caller-selected home path.
    slot: usize,
    data: String,
}

fn repository_root(path: &Path) -> Result<PathBuf> {
    let root = path.canonicalize()?;
    anyhow::ensure!(
        root.is_dir() && root.parent().is_some(),
        "invalid repository"
    );
    if let Some(home) = dirs::home_dir() {
        anyhow::ensure!(root != home, "home is not a project");
    }
    anyhow::ensure!(
        root != storage::jcode_dir()?,
        "private state is not a project"
    );
    // No shell, hooks, or user-controlled commands are executed to resolve paths.
    anyhow::ensure!(root.join(".git").exists(), "repository root required");
    env::checked_path(&root, Path::new(".jcode/cloud-environment.json"))?;
    Ok(root)
}

fn current_root() -> Result<PathBuf> {
    let mut current = std::env::current_dir()?.canonicalize()?;
    loop {
        if current.join(".git").exists() {
            return repository_root(&current);
        }
        anyhow::ensure!(current.pop(), "repository root required");
    }
}

fn append_entry(
    entries: &mut Vec<Entry>,
    resource: &Resource,
    slot: usize,
    bytes: &[u8],
) -> Result<()> {
    let data = env::encode_bytes(bytes);
    let previous: usize = entries.iter().map(|entry| entry.data.len()).sum();
    anyhow::ensure!(
        previous.saturating_add(data.len()) <= env::MAX_PAYLOAD_BYTES,
        "payload too large"
    );
    entries.push(Entry {
        resource: resource.clone(),
        slot,
        data,
    });
    Ok(())
}

fn collect(root: &Path, request: &Request, source: Source) -> Result<Payload> {
    let requirements = match request {
        Request::Status => Vec::new(),
        Request::Sync => env::load_manifest(root)?.requirements,
        Request::Fetch { resource, .. } => vec![resource.clone()],
    };
    let mut entries = Vec::new();
    for resource in &requirements {
        env::validate_resource(resource)?;
        let values = match resource {
            Resource::ProjectFile { path } => {
                let bytes = env::read_private(root, Path::new(path))?;
                env::validate_project_bytes(&bytes)?;
                vec![bytes]
            }
            Resource::EnvVar { name } => {
                let value = std::env::var(name)
                    .ok()
                    .or_else(|| env::load_environment(root).ok()?.get(name).cloned())
                    .context("variable unavailable")?;
                vec![value.into_bytes()]
            }
            Resource::Credential { name } => {
                let home = dirs::home_dir().context("home unavailable")?;
                let mut found = Vec::new();
                for (slot, path) in env::credential_paths(name)?.iter().enumerate() {
                    let full = env::checked_path(&home, Path::new(path))?;
                    if full.exists() {
                        let data = env::read_private(&home, Path::new(path))?;
                        append_entry(&mut entries, resource, slot, &data)?;
                        found.push(());
                    }
                }
                anyhow::ensure!(!found.is_empty(), "credential unavailable");
                continue;
            }
        };
        for (slot, bytes) in values.into_iter().enumerate() {
            append_entry(&mut entries, resource, slot, &bytes)?;
        }
    }
    let payload = Payload {
        version: 1,
        source,
        root: root.to_str().context("root encoding")?.into(),
        requirements,
        entries,
    };
    anyhow::ensure!(
        serde_json::to_vec(&payload)?.len() <= env::MAX_PAYLOAD_BYTES,
        "payload too large"
    );
    Ok(payload)
}

fn transmit(target: &Target, host: &str, payload: &Payload) -> Result<Status> {
    let bytes = serde_json::to_vec(payload)?;
    anyhow::ensure!(bytes.len() <= env::MAX_PAYLOAD_BYTES, "payload too large");
    let mut command = target.remote_command(host, &["cloud", "env", "receive"])?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| anyhow::anyhow!("environment transport unavailable"))?;
    let result = std::thread::scope(|scope| -> Result<Status> {
        let mut stdin = child.stdin.take().context("stdin")?;
        let writer = scope.spawn(move || stdin.write_all(&bytes));
        let mut response = Vec::new();
        child
            .stdout
            .take()
            .context("stdout")?
            .take(4097)
            .read_to_end(&mut response)?;
        if response.len() > 4096 {
            let _ = child.kill();
            bail!("invalid status");
        }
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("writer failed"))??;
        anyhow::ensure!(child.wait()?.success(), "transport failed");
        let status: Status = serde_json::from_slice(&response)?;
        // Treat arbitrary remote strings as untrusted, even on authenticated SSH.
        anyhow::ensure!(
            matches!(status.code.as_str(), "received" | "unavailable"),
            "invalid status"
        );
        anyhow::ensure!(
            status.applied <= 256 && status.skipped <= 256,
            "invalid status"
        );
        Ok(status)
    });
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn send(root: &Path, target: &Target, request: &Request, source: Source) -> Result<Status> {
    if matches!(request, Request::Status) {
        return Ok(Status::new(
            "available",
            0,
            env::load_manifest(root)?.requirements.len(),
        ));
    }
    let host = target.resolve_host()?;
    let payload = collect(root, request, source)?;
    // Metadata only. Reasons are intentionally not persisted because callers may
    // accidentally put secret values in free text.
    for resource in &payload.requirements {
        env::save_requirement(root, resource)?;
    }
    transmit(target, &host, &payload)
}

fn fulfill(session: &str, request: Request) -> Result<Status> {
    anyhow::ensure!(
        std::env::var("JCODE_CLOUD_ENV_SESSION").ok().as_deref() == Some(session),
        "no handoff grant"
    );
    let host = std::env::var("JCODE_SSH_REMOTE").context("no target grant")?;
    let lease = storage::read_session_lease(session).context("no lease")?;
    let root = repository_root(&env::validate_lease(
        &lease,
        session,
        &host,
        chrono::Utc::now(),
    )?)?;
    // Re-read at the last boundary before collecting local resources. Returned or
    // superseded sessions cannot reuse an old authorization snapshot.
    let current = storage::read_session_lease(session).context("no lease")?;
    anyhow::ensure!(current == lease, "lease changed");
    let target = Target {
        host: Some(host),
        remote_binary: None,
        transport: None,
    };
    send(
        &root,
        &target,
        &request,
        Source::Handoff {
            session: session.into(),
            epoch: lease.epoch,
        },
    )
}

fn read_bounded(limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= limit, "input too large");
    Ok(bytes)
}

fn validate_payload(payload: &Payload) -> Result<(PathBuf, Vec<Vec<u8>>)> {
    anyhow::ensure!(
        payload.version == 1 && payload.requirements.len() <= 128 && payload.entries.len() <= 256,
        "invalid payload"
    );
    let root = repository_root(Path::new(&payload.root))?;
    anyhow::ensure!(root.to_str() == Some(&payload.root), "noncanonical root");
    if let Source::Handoff { session, epoch } = &payload.source {
        let lease = storage::read_session_lease(session).context("no receiving lease")?;
        anyhow::ensure!(
            lease.is_here()
                && lease.epoch == *epoch
                && *epoch > 0
                && lease.repo_root.as_deref() == Some(&payload.root),
            "wrong receiving lease"
        );
    }
    // Validate every entry, every decoded value and every destination before any
    // filesystem writes, including metadata writes.
    let mut decoded = Vec::new();
    let mut seen = Vec::new();
    for resource in &payload.requirements {
        env::validate_resource(resource)?;
    }
    for entry in &payload.entries {
        env::validate_resource(&entry.resource)?;
        anyhow::ensure!(
            payload.requirements.contains(&entry.resource),
            "undeclared resource"
        );
        anyhow::ensure!(
            !seen.contains(&(&entry.resource, entry.slot)),
            "duplicate resource"
        );
        seen.push((&entry.resource, entry.slot));
        let bytes = env::decode_bytes(&entry.data)?;
        match &entry.resource {
            Resource::ProjectFile { path } => {
                anyhow::ensure!(entry.slot == 0, "invalid slot");
                env::validate_project_bytes(&bytes)?;
                env::checked_path(&root, Path::new(path))?;
            }
            Resource::EnvVar { .. } => {
                anyhow::ensure!(
                    entry.slot == 0
                        && bytes.len() <= env::MAX_PAYLOAD_BYTES / 2
                        && !bytes.contains(&0),
                    "invalid environment"
                );
                std::str::from_utf8(&bytes)?;
            }
            Resource::Credential { name } => {
                let paths = env::credential_paths(name)?;
                let path = paths.get(entry.slot).context("invalid credential slot")?;
                env::checked_path(
                    &dirs::home_dir().context("home unavailable")?,
                    Path::new(path),
                )?;
            }
        }
        decoded.push(bytes);
    }
    anyhow::ensure!(
        payload
            .requirements
            .iter()
            .all(|r| payload.entries.iter().any(|e| &e.resource == r)),
        "missing resource"
    );
    // Preflight the manifest and private environment state as well.
    let mut existing = env::load_manifest(&root)?.requirements;
    for resource in &payload.requirements {
        if !existing.contains(resource) {
            existing.push(resource.clone());
        }
    }
    anyhow::ensure!(existing.len() <= 128, "too many saved requirements");
    if payload
        .entries
        .iter()
        .any(|e| matches!(e.resource, Resource::EnvVar { .. }))
    {
        env::load_environment(&root)?;
    }
    Ok((root, decoded))
}

fn receive(payload: Payload) -> Result<Status> {
    let (root, decoded) = validate_payload(&payload)?;
    let mut applied = 0;
    let mut skipped = 0;
    for (entry, bytes) in payload.entries.iter().zip(decoded) {
        let wrote = match &entry.resource {
            Resource::ProjectFile { path } => env::publish_private(&root, Path::new(path), &bytes)?,
            Resource::EnvVar { name } => {
                env::save_environment(&root, name, std::str::from_utf8(&bytes)?)?
            }
            Resource::Credential { name } => env::publish_private(
                &dirs::home_dir().context("home unavailable")?,
                Path::new(env::credential_paths(name)?[entry.slot]),
                &bytes,
            )?,
        };
        if wrote {
            applied += 1;
        } else {
            skipped += 1;
        }
    }
    for resource in &payload.requirements {
        env::save_requirement(&root, resource)?;
    }
    Ok(Status::new("received", applied, skipped))
}

pub(crate) fn run_cli(action: super::super::args::CloudEnvCommand) -> Result<()> {
    use super::super::args::CloudEnvCommand as C;
    let result = (|| -> Result<Status> {
        match action {
            C::Push { host, request } => {
                let request = env::parse_request(request.as_bytes())?;
                let root = current_root()?;
                send(
                    &root,
                    &Target {
                        host,
                        remote_binary: None,
                        transport: None,
                    },
                    &request,
                    Source::LocalPush {},
                )
            }
            C::Fulfill { session } => fulfill(
                &session,
                env::parse_request(&read_bounded(env::MAX_REQUEST_BYTES)?)?,
            ),
            C::Receive => receive(serde_json::from_slice(&read_bounded(
                env::MAX_PAYLOAD_BYTES,
            )?)?),
        }
    })();
    // Errors are intentionally flattened, never printed, formatted or logged.
    // Successful process exit lets callers parse the bounded status consistently.
    let status = result.unwrap_or_else(|_| Status::rejected());
    println!("{}", serde_json::to_string(&status)?);
    Ok(())
}

pub(super) fn sync_saved(root: &Path, target: &super::Target) -> Result<()> {
    let root = repository_root(root)?;
    if env::load_manifest(&root)?.requirements.is_empty() {
        return Ok(());
    }
    let status = send(&root, target, &Request::Sync, Source::LocalPush {})
        .map_err(|_| anyhow::anyhow!("saved environment sync unavailable"))?;
    anyhow::ensure!(status.ok, "saved environment sync unavailable");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_payload_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let good = Resource::ProjectFile {
            path: "good".into(),
        };
        let bad = Resource::ProjectFile {
            path: "../bad".into(),
        };
        let payload = Payload {
            version: 1,
            source: Source::LocalPush {},
            root: root.to_str().unwrap().into(),
            requirements: vec![good.clone(), bad.clone()],
            entries: vec![
                Entry {
                    resource: good,
                    slot: 0,
                    data: "c2VjcmV0".into(),
                },
                Entry {
                    resource: bad,
                    slot: 0,
                    data: "c2VjcmV0".into(),
                },
            ],
        };
        assert!(receive(payload).is_err());
        assert!(!root.join("good").exists());
        assert!(!root.join(".jcode").exists());
    }
    #[test]
    fn source_and_request_are_strict() {
        assert!(
            serde_json::from_str::<Source>(r#"{"kind":"local_push","value":"secret"}"#).is_err()
        );
        assert!(serde_json::from_str::<Source>(r#"{"kind":"arbitrary"}"#).is_err());
        assert!(env::parse_request(br#"{"action":"status","value":"secret"}"#).is_err());
    }
    #[test]
    fn status_never_reflects_errors() {
        let result: Result<Status> = Err(anyhow::anyhow!("secret-value"));
        let output = serde_json::to_string(&result.unwrap_or_else(|_| Status::rejected())).unwrap();
        assert!(!output.contains("secret-value"));
        assert!(output.len() < 128);
    }
}
