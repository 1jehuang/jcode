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
    anyhow::ensure!(root.join(".git").try_exists()?, "repository root required");
    env::checked_path(&root, Path::new(".jcode/cloud-environment.json"))?;
    Ok(root)
}

fn current_root() -> Result<PathBuf> {
    let mut current = std::env::current_dir()?.canonicalize()?;
    loop {
        if current.join(".git").try_exists()? {
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
                let value = match std::env::var(name) {
                    Ok(value) => value,
                    Err(std::env::VarError::NotPresent) => env::load_environment(root)?
                        .get(name)
                        .cloned()
                        .context("variable unavailable")?,
                    Err(std::env::VarError::NotUnicode(_)) => bail!("variable encoding invalid"),
                };
                env::validate_environment_value(&value)?;
                vec![value.into_bytes()]
            }
            Resource::Credential { name } => {
                let home = dirs::home_dir().context("home unavailable")?;
                let mut found = Vec::new();
                for (slot, path) in env::credential_paths(name)?.iter().enumerate() {
                    let full = env::checked_path(&home, Path::new(path))?;
                    if full
                        .try_exists()
                        .context("cannot inspect credential availability")?
                    {
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

fn exchange_bounded(
    mut command: Command,
    bytes: Vec<u8>,
    timeout: std::time::Duration,
) -> Result<Vec<u8>> {
    #[cfg(not(unix))]
    {
        // Release the unused command and private input on unsupported platforms.
        drop((command, bytes, timeout));
        bail!("private transport unsupported");
    }
    #[cfg(unix)]
    {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let own_process_group = std::env::var_os("JCODE_CLOUD_ENV_SESSION").is_none();
        // Keep fulfillment inside the TUI-owned disconnect cancellation group.
        if own_process_group {
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|_| anyhow::anyhow!("environment transport unavailable"))?;
        let result = (|| -> Result<Vec<u8>> {
            let mut stdin = child.stdin.take();
            let mut stdout = child.stdout.take().context("stdout")?;
            for fd in [
                stdin.as_ref().context("stdin")?.as_raw_fd(),
                stdout.as_raw_fd(),
            ] {
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                anyhow::ensure!(
                    flags >= 0
                        && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
                    "cannot configure private pipe"
                );
            }
            let deadline = std::time::Instant::now() + timeout;
            let mut sent = 0;
            let mut response = Vec::new();
            let mut eof = false;
            let mut status = None;
            loop {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "environment transport timed out"
                );
                if let Some(input) = stdin.as_mut() {
                    if sent < bytes.len() {
                        match input.write(&bytes[sent..]) {
                            Ok(0) => bail!("private pipe closed"),
                            Ok(count) => sent += count,
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(_) => bail!("private pipe write failed"),
                        }
                    }
                    if sent == bytes.len() {
                        stdin.take();
                    }
                }
                if !eof {
                    let mut buffer = [0; 1024];
                    match stdout.read(&mut buffer) {
                        Ok(0) => eof = true,
                        Ok(count) => {
                            response.extend_from_slice(&buffer[..count]);
                            anyhow::ensure!(response.len() <= 4096, "invalid status");
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(_) => bail!("private pipe read failed"),
                    }
                }
                if status.is_none() {
                    status = child.try_wait()?;
                }
                if let Some(status) = status {
                    anyhow::ensure!(status.success(), "transport failed");
                    if eof {
                        anyhow::ensure!(sent == bytes.len(), "incomplete transfer");
                        return Ok(response);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        })();
        if result.is_err() {
            if own_process_group {
                unsafe {
                    if libc::kill(-(child.id() as i32), libc::SIGKILL) != 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                    {
                        crate::logging::warn("private transport process group cleanup failed");
                    }
                }
            }
            match child.kill() {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
                Err(_) => crate::logging::warn("private transport process cleanup failed"),
            }
        }
        child
            .wait()
            .map_err(|_| anyhow::anyhow!("private transport reaping failed"))?;
        result
    }
}

fn transmit(target: &Target, host: &str, payload: &Payload) -> Result<Status> {
    let bytes = serde_json::to_vec(payload)?;
    anyhow::ensure!(bytes.len() <= env::MAX_PAYLOAD_BYTES, "payload too large");
    let command = target.remote_command(host, &["cloud", "env", "receive"])?;
    let response = exchange_bounded(command, bytes, std::time::Duration::from_secs(30))?;
    let status: Status = serde_json::from_slice(&response)?;
    anyhow::ensure!(
        matches!(status.code.as_str(), "received" | "unavailable"),
        "invalid status"
    );
    anyhow::ensure!(
        status.applied <= 256 && status.skipped <= 256,
        "invalid status"
    );
    Ok(status)
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

fn callback_target(host: String) -> Result<Target> {
    let remote_binary = match std::env::var("JCODE_SSH_BINARY") {
        Ok(binary) => Some(binary).filter(|binary| !binary.trim().is_empty()),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => bail!("remote binary encoding invalid"),
    };
    Ok(Target {
        host: Some(host),
        remote_binary,
        transport: None,
    })
}

fn fulfill(session: &str, request: Request) -> Result<Status> {
    anyhow::ensure!(
        std::env::var_os("JCODE_CLOUD_ENV_SESSION").as_deref()
            == Some(std::ffi::OsStr::new(session)),
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
    let target = callback_target(host)?;
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
                env::validate_environment_value(std::str::from_utf8(&bytes)?)?;
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
    // Shared group members may push concurrently. Serialize the complete
    // preflight/publication sequence, then revalidate leases under the lock.
    // Per-resource publication remains no-clobber and retries remain safe.
    // Invalid input must not create even private lock metadata. Recheck under
    // the lock as another receiver may have changed state while we waited.
    let (candidate_root, _) = validate_payload(&payload)?;
    let _hydration = env::lock_workspace_hydration(&candidate_root)?;
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
    // Never format the underlying error: parsing and validation may contain secrets.
    // Preserve a bounded status on stdout while signaling failure to the caller.
    match result {
        Ok(status) => {
            println!("{}", serde_json::to_string(&status)?);
            Ok(())
        }
        Err(_) => {
            println!("{}", serde_json::to_string(&Status::rejected())?);
            bail!("private environment request rejected")
        }
    }
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
    fn rejected_cli_request_returns_redacted_failure() {
        let error = run_cli(super::super::super::args::CloudEnvCommand::Push {
            host: None,
            request: "secret-invalid-request".into(),
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "private environment request rejected");
    }

    #[test]
    #[cfg(unix)]
    fn callback_rejects_non_unicode_binary_without_leaking_it() {
        use std::os::unix::ffi::OsStringExt;
        let _lock = storage::lock_test_env();
        let previous = std::env::var_os("JCODE_SSH_BINARY");
        crate::env::set_var(
            "JCODE_SSH_BINARY",
            std::ffi::OsString::from_vec(b"secret\xff".to_vec()),
        );
        let result = callback_target("test-host".into());
        match previous {
            Some(value) => crate::env::set_var("JCODE_SSH_BINARY", value),
            None => crate::env::remove_var("JCODE_SSH_BINARY"),
        }
        let error = result.err().expect("invalid encoding must fail");
        assert_eq!(error.to_string(), "remote binary encoding invalid");
    }

    #[test]
    #[cfg(unix)]
    fn concurrent_workspace_receives_publish_once() {
        let _guard = storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let barrier = std::sync::Barrier::new(8);
        let statuses = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let root = &root;
                let barrier = &barrier;
                handles.push(scope.spawn(move || {
                    let resource = Resource::ProjectFile {
                        path: "shared.env".into(),
                    };
                    barrier.wait();
                    receive(Payload {
                        version: 1,
                        source: Source::LocalPush {},
                        root: root.to_str().unwrap().into(),
                        requirements: vec![resource.clone()],
                        entries: vec![Entry {
                            resource,
                            slot: 0,
                            data: env::encode_bytes(b"test-value"),
                        }],
                    })
                    .unwrap()
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(statuses.iter().all(|s| s.ok));
        assert_eq!(statuses.iter().map(|s| s.applied).sum::<usize>(), 1);
        assert_eq!(statuses.iter().map(|s| s.skipped).sum::<usize>(), 7);
        assert_eq!(env::load_manifest(&root).unwrap().requirements.len(), 1);
        assert_eq!(
            env::read_private(&root, Path::new("shared.env")).unwrap(),
            b"test-value"
        );
    }

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
    #[cfg(unix)]
    fn rejects_private_key_environment_before_publishing_project_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let file = Resource::ProjectFile {
            path: "allowed".into(),
        };
        let variable = Resource::EnvVar {
            name: "SERVICE_TOKEN".into(),
        };
        let payload = Payload {
            version: 1,
            source: Source::LocalPush {},
            root: root.to_str().unwrap().into(),
            requirements: vec![file.clone(), variable.clone()],
            entries: vec![
                Entry {
                    resource: file,
                    slot: 0,
                    data: env::encode_bytes(b"ordinary content"),
                },
                Entry {
                    resource: variable,
                    slot: 0,
                    data: env::encode_bytes(b"-----BEGIN OPENSSH PRIVATE KEY-----"),
                },
            ],
        };
        assert!(receive(payload).is_err());
        assert!(!root.join("allowed").exists());
        assert!(!root.join(".jcode").exists());
    }

    #[test]
    #[cfg(unix)]
    fn bounded_transport_times_out_blocked_pipes_and_descendants() {
        let _lock = storage::lock_test_env();
        for (script, input) in [
            ("sleep 10", vec![0; 1024 * 1024]),
            ("sleep 10 & exit 0", Vec::new()),
        ] {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            let start = std::time::Instant::now();
            assert!(
                exchange_bounded(command, input, std::time::Duration::from_millis(100)).is_err()
            );
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
        }
    }
    #[test]
    #[cfg(unix)]
    fn bounded_transport_success() {
        let _lock = storage::lock_test_env();
        let mut command = Command::new("sh");
        command.args(["-c", "cat >/dev/null; printf 'safe-status'"]);
        assert_eq!(
            exchange_bounded(
                command,
                b"opaque-input".to_vec(),
                std::time::Duration::from_secs(3)
            )
            .unwrap(),
            b"safe-status"
        );
    }
    #[test]
    #[cfg(unix)]
    fn fulfillment_transport_stays_in_caller_process_group() {
        let _lock = storage::lock_test_env();
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    crate::env::set_var("JCODE_CLOUD_ENV_SESSION", value);
                } else {
                    crate::env::remove_var("JCODE_CLOUD_ENV_SESSION");
                }
            }
        }
        let _restore = Restore(std::env::var_os("JCODE_CLOUD_ENV_SESSION"));
        crate::env::set_var("JCODE_CLOUD_ENV_SESSION", "test-session");
        let mut command = Command::new("sh");
        command.args(["-c", "ps -o pgid= -p $$"]);
        let output =
            exchange_bounded(command, Vec::new(), std::time::Duration::from_secs(3)).unwrap();
        let group: i32 = std::str::from_utf8(&output)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(group, unsafe { libc::getpgrp() });
    }
    #[test]
    fn callback_honors_remote_binary() {
        let _lock = storage::lock_test_env();
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    crate::env::set_var("JCODE_SSH_BINARY", value);
                } else {
                    crate::env::remove_var("JCODE_SSH_BINARY");
                }
            }
        }
        let _restore = Restore(std::env::var_os("JCODE_SSH_BINARY"));
        crate::env::set_var("JCODE_SSH_BINARY", "/custom/remote-jcode");
        let target = callback_target("test-host".into()).unwrap();
        assert_eq!(
            target.remote_binary.as_deref(),
            Some("/custom/remote-jcode")
        );
        let command = target
            .remote_command("test-host", &["cloud", "env", "receive"])
            .unwrap();
        assert!(
            command
                .get_args()
                .any(|arg| arg.to_string_lossy().contains("/custom/remote-jcode"))
        );
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
