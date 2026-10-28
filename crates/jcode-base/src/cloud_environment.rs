//! Explicit, metadata-only environment requirements and opaque resource transfer.
//!
//! Request/response types intentionally cannot contain secret bytes. Only the private
//! SSH payload carries values. Project paths are relative and credentials are fixed.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const LOCAL_ACCESS_SECONDS: i64 = 900;
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 8192;
const MAX_REQUIREMENTS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Resource {
    ProjectFile { path: String },
    EnvVar { name: String },
    Credential { name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Sync,
    Fetch {
        resource: Resource,
        #[serde(default)]
        reason: Option<String>,
    },
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub requirements: Vec<Resource>,
}

/// Deliberately omits names, paths, values and error strings supplied by callers.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub ok: bool,
    pub code: String,
    pub applied: usize,
    pub skipped: usize,
}
impl Status {
    pub fn new(code: &str, applied: usize, skipped: usize) -> Self {
        Self {
            ok: true,
            code: code.to_owned(),
            applied,
            skipped,
        }
    }
    pub fn rejected() -> Self {
        Self {
            ok: false,
            code: "unavailable".into(),
            applied: 0,
            skipped: 0,
        }
    }
}

pub fn validate_resource(resource: &Resource) -> Result<()> {
    match resource {
        Resource::ProjectFile { path } => {
            ensure!(
                !path.is_empty()
                    && path.len() <= 1024
                    && !path.contains('\\')
                    && !path.contains('\0'),
                "invalid project path"
            );
            ensure!(
                Path::new(path)
                    .components()
                    .all(|p| matches!(p, Component::Normal(_))),
                "invalid project path"
            );
            // Never let the callback mutate control metadata, Git hooks, or private state.
            ensure!(
                !Path::new(path)
                    .components()
                    .any(|p| matches!(p, Component::Normal(s) if s == ".git" || s == ".jcode" || s == ".ssh")),
                "reserved path"
            );
        }
        Resource::EnvVar { name } => {
            ensure!(
                !name.is_empty()
                    && name.len() <= 128
                    && name.bytes().enumerate().all(|(i, b)| b == b'_'
                        || b.is_ascii_alphabetic()
                        || (i > 0 && b.is_ascii_digit())),
                "invalid variable"
            );
            // Shell/runtime injection knobs are not environment dependencies.
            ensure!(
                !name.starts_with("JCODE_")
                    && !name.starts_with("SSH_")
                    && !name.starts_with("LD_")
                    && !name.starts_with("DYLD_")
                    && !matches!(
                        name.as_str(),
                        "PATH"
                            | "HOME"
                            | "SHELL"
                            | "ENV"
                            | "BASH_ENV"
                            | "SHELLOPTS"
                            | "BASHOPTS"
                            | "CDPATH"
                            | "GLOBIGNORE"
                            | "IFS"
                    ),
                "reserved variable"
            );
        }
        Resource::Credential { name } => {
            credential_paths(name)?;
        }
    }
    Ok(())
}

/// Opaque wire encoding, used only inside the private SSH payload.
pub fn encode_bytes(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
pub fn decode_bytes(data: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    ensure!(
        data.len() <= MAX_PAYLOAD_BYTES,
        "encoded resource too large"
    );
    Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
}

pub fn validate_project_bytes(bytes: &[u8]) -> Result<()> {
    // A project-relative name must not provide a back door for SSH key export.
    for kind in ["OPENSSH ", "RSA ", "EC ", "DSA ", "", "ENCRYPTED "] {
        let marker = format!("-----BEGIN {kind}PRIVATE KEY-----");
        let marker = marker.as_bytes();
        ensure!(
            !bytes.windows(marker.len()).any(|w| w == marker),
            "private key denied"
        );
    }
    Ok(())
}

pub fn credential_paths(name: &str) -> Result<&'static [&'static str]> {
    match name {
        "github" => Ok(&[".config/gh/hosts.yml"]),
        "aws" => Ok(&[".aws/credentials", ".aws/config"]),
        "npm" => Ok(&[".npmrc"]),
        _ => bail!("credential not allowed"),
    }
}

pub fn parse_request(bytes: &[u8]) -> Result<Request> {
    ensure!(bytes.len() <= MAX_REQUEST_BYTES, "request too large");
    // Serde internally-tagged unit variants otherwise silently ignore extra
    // keys, even with deny_unknown_fields. Reject them before deserializing.
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value.as_object().context("request must be an object")?;
    let action = object
        .get("action")
        .and_then(|v| v.as_str())
        .context("missing action")?;
    let allowed: &[&str] = match action {
        "status" | "sync" => &["action"],
        "fetch" => &["action", "resource", "reason"],
        _ => bail!("unknown action"),
    };
    ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "unknown request field"
    );
    let request: Request = serde_json::from_value(value)?;
    if let Request::Fetch { resource, reason } = &request {
        validate_resource(resource)?;
        ensure!(
            reason.as_ref().is_none_or(|s| s.len() <= 1024),
            "reason too long"
        );
    }
    Ok(request)
}

/// Reject symlinks at every component, including the leaf. Used before any reads
/// and alongside O_NOFOLLOW directory-descriptor operations for writes.
pub fn checked_path(base: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(base.is_absolute(), "absolute root required");
    ensure!(
        relative
            .components()
            .all(|p| matches!(p, Component::Normal(_))),
        "invalid relative path"
    );
    let mut path = PathBuf::new();
    for part in base.components().chain(relative.components()) {
        path.push(part.as_os_str());
        match std::fs::symlink_metadata(&path) {
            Ok(m) => ensure!(!m.file_type().is_symlink(), "symlink denied"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}

#[cfg(unix)]
fn open_parent(
    base: &Path,
    relative: &Path,
    create: bool,
) -> Result<(std::fs::File, std::ffi::CString)> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    checked_path(base, relative)?;
    // Walk from / using anchored descriptors, so directory renames and symlink
    // substitution cannot redirect access outside the approved root.
    let mut dir = std::fs::File::open("/")?;
    let full = base.join(relative);
    let parent = full.parent().context("parent")?;
    for part in parent.components() {
        if let Component::Normal(p) = part {
            let name = std::ffi::CString::new(p.as_bytes())?;
            let mut fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0
                && create
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
            {
                let rc = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
                if rc < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                fd = unsafe {
                    libc::openat(
                        dir.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
            }
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            dir = unsafe { std::fs::File::from_raw_fd(fd) };
        }
    }
    Ok((
        dir,
        std::ffi::CString::new(full.file_name().context("file")?.as_bytes())?,
    ))
}

pub fn read_private(base: &Path, relative: &Path) -> Result<Vec<u8>> {
    #[cfg(unix)]
    let file = {
        use std::os::fd::{AsRawFd, FromRawFd};
        let (dir, name) = open_parent(base, relative, false)?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        ensure!(fd >= 0, "resource unavailable");
        unsafe { std::fs::File::from_raw_fd(fd) }
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(checked_path(base, relative)?)?;
    ensure!(file.metadata()?.is_file(), "regular file required");
    let mut bytes = Vec::new();
    file.take(MAX_PAYLOAD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_PAYLOAD_BYTES, "resource too large");
    Ok(bytes)
}

/// Publish atomically without overwriting any pre-existing object. Returns false
/// for differing existing data as well as identical data (both are safe skips).
pub fn publish_private(base: &Path, relative: &Path, bytes: &[u8]) -> Result<bool> {
    ensure!(bytes.len() <= MAX_PAYLOAD_BYTES, "resource too large");
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        let (dir, name) = open_parent(base, relative, true)?;
        let tmp = std::ffi::CString::new(format!(".jcode-env-{}", uuid::Uuid::new_v4()))?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                tmp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        ensure!(fd >= 0, "cannot stage resource");
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let result = (|| -> Result<bool> {
            file.write_all(bytes)?;
            file.sync_all()?;
            let rc = unsafe {
                libc::linkat(
                    dir.as_raw_fd(),
                    tmp.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            };
            if rc == 0 {
                dir.sync_all()?;
                return Ok(true);
            }
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                return Ok(false);
            }
            Err(std::io::Error::last_os_error().into())
        })();
        unsafe {
            libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0);
        }
        result
    }
    #[cfg(not(unix))]
    {
        bail!(
            "private publication of {} bytes to {} unsupported on this platform",
            bytes.len(),
            base.join(relative).display()
        )
    }
}

fn manifest_path() -> &'static Path {
    Path::new(".jcode/cloud-environment.json")
}
pub fn load_manifest(root: &Path) -> Result<Manifest> {
    if !checked_path(root, manifest_path())?.exists() {
        return Ok(Manifest::default());
    }
    let manifest: Manifest = serde_json::from_slice(&read_private(root, manifest_path())?)?;
    ensure!(
        manifest.requirements.len() <= MAX_REQUIREMENTS,
        "too many requirements"
    );
    for resource in &manifest.requirements {
        validate_resource(resource)?;
    }
    Ok(manifest)
}
/// An advisory cross-process lock serializes read-modify-write of our private
/// maps. The lock inode is never replaced, unlike the atomically published data.
fn lock_private(base: &Path, relative: &Path) -> Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        let (dir, name) = open_parent(base, relative, true)?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
                0o600,
            )
        };
        ensure!(fd >= 0, "cannot lock private state");
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        ensure!(file.metadata()?.is_file(), "invalid private lock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            ensure!(
                matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ),
                "cannot lock private state"
            );
            ensure!(
                std::time::Instant::now() < deadline,
                "private state lock timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        bail!(
            "private state lock for {} unsupported on this platform",
            base.join(relative).display()
        )
    }
}

pub fn save_requirement(root: &Path, resource: &Resource) -> Result<()> {
    validate_resource(resource)?;
    let _lock = lock_private(root, Path::new(".jcode/cloud-environment.lock"))?;
    let mut manifest = load_manifest(root)?;
    if manifest.requirements.contains(resource) {
        return Ok(());
    }
    ensure!(
        manifest.requirements.len() < MAX_REQUIREMENTS,
        "too many requirements"
    );
    manifest.requirements.push(resource.clone());
    replace_private(
        root,
        manifest_path(),
        &serde_json::to_vec_pretty(&manifest)?,
    )
}

// Metadata and environment maps require replacement, but only in our private
// namespace. Anchored rename replaces a symlink itself, never follows it.
fn replace_private(base: &Path, relative: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        ensure!(bytes.len() <= MAX_PAYLOAD_BYTES, "resource too large");
        let (dir, name) = open_parent(base, relative, true)?;
        let tmp = std::ffi::CString::new(format!(".jcode-env-{}", uuid::Uuid::new_v4()))?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                tmp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        ensure!(fd >= 0, "cannot stage metadata");
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            let rc = unsafe {
                libc::renameat(
                    dir.as_raw_fd(),
                    tmp.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                )
            };
            if rc < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            dir.sync_all()?;
            Ok(())
        })();
        unsafe {
            libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0);
        }
        result
    }
    #[cfg(not(unix))]
    {
        bail!(
            "private publication of {} bytes to {} unsupported on this platform",
            bytes.len(),
            base.join(relative).display()
        )
    }
}

fn environment_relative(root: &Path) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let canonical = root.canonicalize()?;
    // Shell tools execute in nested working directories. Key all of them to
    // the nearest repository root (including Git worktrees with a .git file).
    let root = canonical
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&canonical);
    Ok(PathBuf::from("cloud-environment").join(format!(
        "{:x}.json",
        Sha256::digest(root.as_os_str().as_encoded_bytes())
    )))
}

/// Serialize hydration for a shared checkout, without granting any transfer
/// authority. Keep this guard across validation and publication. Lock acquisition
/// is bounded to five seconds; dropping it (including process exit) releases it.
/// This is separate from the map locks acquired during publication.
pub fn lock_workspace_hydration(root: &Path) -> Result<std::fs::File> {
    let home = crate::storage::jcode_dir()?;
    let relative = environment_relative(root)?.with_extension("hydrate.lock");
    lock_private(&home, &relative)
}
pub fn load_environment(root: &Path) -> Result<BTreeMap<String, String>> {
    let home = crate::storage::jcode_dir()?;
    let relative = environment_relative(root)?;
    if !checked_path(&home, &relative)?.exists() {
        return Ok(BTreeMap::new());
    }
    let vars: BTreeMap<String, String> = serde_json::from_slice(&read_private(&home, &relative)?)?;
    for (name, value) in &vars {
        validate_resource(&Resource::EnvVar { name: name.clone() })?;
        validate_environment_value(value)?;
    }
    Ok(vars)
}
pub fn validate_environment_value(value: &str) -> Result<()> {
    ensure!(
        !value.contains('\0') && value.len() <= MAX_PAYLOAD_BYTES / 2,
        "invalid environment value"
    );
    validate_project_bytes(value.as_bytes())
}

pub fn save_environment(root: &Path, name: &str, value: &str) -> Result<bool> {
    validate_resource(&Resource::EnvVar {
        name: name.to_owned(),
    })?;
    validate_environment_value(value)?;
    let home = crate::storage::jcode_dir()?;
    let relative = environment_relative(root)?;
    let _lock = lock_private(&home, &relative.with_extension("lock"))?;
    let mut vars = load_environment(root)?;
    if vars.contains_key(name) {
        return Ok(false);
    }
    vars.insert(name.to_owned(), value.to_owned());
    let bytes = serde_json::to_vec(&vars)?;
    ensure!(bytes.len() <= MAX_PAYLOAD_BYTES, "environment too large");
    replace_private(
        &crate::storage::jcode_dir()?,
        &environment_relative(root)?,
        &bytes,
    )?;
    Ok(true)
}

pub fn validate_lease(
    lease: &crate::storage::SessionLease,
    session: &str,
    host: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PathBuf> {
    ensure!(
        lease.session_id == session
            && lease.epoch > 0
            && lease.away_host.as_deref() == Some(host)
            && !host.is_empty(),
        "invalid lease"
    );
    let updated = chrono::DateTime::parse_from_rfc3339(&lease.updated_at)?;
    let age = now.signed_duration_since(updated);
    ensure!(
        age >= chrono::Duration::zero() && age < chrono::Duration::seconds(LOCAL_ACCESS_SECONDS),
        "expired lease"
    );
    let root = PathBuf::from(lease.repo_root.as_ref().context("missing root")?);
    ensure!(
        root.is_absolute() && root.is_dir() && root.canonicalize()? == root,
        "invalid root"
    );
    Ok(root)
}

fn parity_guidance() -> &'static str {
    "Check the actual tools, runtimes, files, credentials and environment before work: this is not an identical machine. Record runtime and package dependencies in project setup documentation, and record only typed resource requirements in .jcode/cloud-environment.json. Never print, quote, summarize, or put secret values in messages or command arguments. Allowed transfers are explicit relative project files, named environment variables and fixed credentials (github, aws, npm), never SSH private keys, keychains or arbitrary home paths. The cloud_environment tool and a local callback are conditional capabilities, not guaranteed. When a callback is available, first call status and then sync saved requirements. Fetch only specific missing resources. If executing locally, run jcode cloud env push --host <cloud-host> --request '<request JSON>' yourself using local tools, with metadata-only request JSON; ask the user only when local execution is unavailable. If executing remotely without a callback, ask for that command to run on the local source machine."
}

pub fn migration_guidance(session_id: &str) -> String {
    format!(
        "You are continuing session {session_id} after a machine handoff. {} Local callback access expires 15 minutes after handoff and may already be unavailable.",
        parity_guidance()
    )
}

/// Group transfers share a canonical checkout and private environment map.
/// Guidance is not authorization: only the coordinator's explicitly scoped
/// temporary lease/callback may request local resources.
pub fn group_migration_guidance(session_id: &str, coordinator_session_id: &str) -> String {
    if session_id == coordinator_session_id {
        format!(
            "You are coordinator {session_id} continuing after a group machine handoff. Root and members share one repository and its hydrated environment. Coordinate runtime/package setup once for the workspace, checking actual state first; do not claim setup succeeded until verified. Only you own the temporary local bridge, if granted. Check callback status and sync saved requirements once for the group when needed, not once per member. Deduplicate members' metadata-only missing-resource requests and fetch only resources still missing. Relay nonsecret completion or failure to members. Callback access remains scoped to your session, destination, epoch and 15-minute lease; group membership grants no additional authorization. If unavailable, arrange an explicit local push, never delegate unrestricted callback access to members. Never print or relay secret values. Record runtime/package dependencies in setup documentation and only typed resource identifiers in .jcode/cloud-environment.json. Allowed resources are explicit relative project files, named environment variables and fixed github/aws/npm credentials, never SSH private keys, keychains or arbitrary home paths."
        )
    } else {
        format!(
            "You are member {session_id} continuing after a group machine handoff. Coordinator {coordinator_session_id} owns workspace environment setup and any temporary local bridge. You share the same repository and hydrated private environment as the coordinator and other members. Check actual prerequisites for your task, but do not independently repeat workspace setup, sync saved requirements, invoke the local callback, or request a separate local push. Send specific missing-resource identifiers and a nonsecret reason to the coordinator, then wait for confirmation and recheck availability. Group membership does not grant callback authorization. Newly hydrated environment variables apply to subsequent tool processes, not already-running processes. Never put secret values in messages, command arguments, logs or setup documentation. Do not assume this machine is identical or that setup has completed."
        )
    }
}
pub fn replacement_guidance(host: Option<&str>) -> String {
    let target = host.filter(|s| !s.is_empty()).unwrap_or("<cloud-host>");
    format!(
        "You are still the LOCAL agent after a repository-only replacement, not a moved session or a remote agent. {} Use the actual target {target} in the local push command. Run the local push yourself when resources are missing; do not require the user to do work that your local tools can perform. Replacement does not establish a callback or change where you execute.",
        parity_guidance()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!("jcode-env-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        p.canonicalize().unwrap()
    }
    struct HomeGuard(Option<std::ffi::OsString>);
    impl HomeGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", path);
            Self(previous)
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.0 {
                crate::env::set_var("JCODE_HOME", previous);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }
    #[test]
    fn environment_root_subdirectory_and_invalid_values() {
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        let subdirectory = project.path().join("src/nested");
        std::fs::create_dir_all(&subdirectory).unwrap();
        assert!(save_environment(project.path(), "TEST_TOKEN", "opaque-test-value").unwrap());
        assert_eq!(
            load_environment(project.path()).unwrap(),
            load_environment(&subdirectory).unwrap()
        );
        assert_eq!(
            load_environment(&subdirectory)
                .unwrap()
                .get("TEST_TOKEN")
                .map(String::as_str),
            Some("opaque-test-value")
        );
        let relative = environment_relative(project.path()).unwrap();
        replace_private(
            home.path(),
            &relative,
            br#"{"TEST_TOKEN":"bad\u0000value"}"#,
        )
        .unwrap();
        assert!(load_environment(&subdirectory).is_err());
    }
    #[test]
    fn concurrent_manifest_and_environment_updates_are_not_lost() {
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        let barrier = std::sync::Barrier::new(12);
        std::thread::scope(|scope| {
            for i in 0..12 {
                let barrier = &barrier;
                let root = project.path();
                scope.spawn(move || {
                    barrier.wait();
                    let name = format!("TEST_VAR_{i}");
                    save_requirement(root, &Resource::EnvVar { name: name.clone() }).unwrap();
                    assert!(save_environment(root, &name, "test-value").unwrap());
                });
            }
        });
        assert_eq!(
            load_manifest(project.path()).unwrap().requirements.len(),
            12
        );
        assert_eq!(load_environment(project.path()).unwrap().len(), 12);
        assert!(!save_environment(project.path(), "TEST_VAR_0", "replacement").unwrap());
        assert_eq!(
            load_environment(project.path()).unwrap()["TEST_VAR_0"],
            "test-value"
        );
    }
    #[test]
    fn group_guidance_keeps_callback_with_coordinator() {
        let coordinator = group_migration_guidance("root", "root");
        let member = group_migration_guidance("worker", "root");
        assert!(coordinator.contains("sync saved requirements once for the group"));
        assert!(coordinator.contains("do not claim setup succeeded until verified"));
        assert!(coordinator.contains("session, destination, epoch"));
        assert!(member.contains("Coordinator root"));
        assert!(member.contains("do not independently repeat workspace setup"));
        assert!(member.contains("Group membership does not grant callback authorization"));
        assert!(!member.contains("jcode cloud env push"));
    }

    #[test]
    #[cfg(unix)]
    fn workspace_hydration_lock_is_shared_and_released() {
        use std::os::fd::AsRawFd;
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        let nested = project.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let guard = lock_workspace_hydration(project.path()).unwrap();
        let relative = environment_relative(&nested)
            .unwrap()
            .with_extension("hydrate.lock");
        let contender = std::fs::File::open(home.path().join(relative)).unwrap();
        assert_ne!(
            unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        // Hydration does not recursively acquire the map lock.
        assert!(save_environment(&nested, "TEST_TOKEN", "test-value").unwrap());
        drop(guard);
        assert_eq!(
            unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
    }

    #[test]
    fn shared_guidance_preserves_local_replacement_role() {
        let migration = migration_guidance("session-test");
        let replacement = replacement_guidance(Some("test-host"));
        assert!(migration.contains(parity_guidance()));
        assert!(replacement.contains(parity_guidance()));
        assert!(replacement.contains("still the LOCAL agent"));
        assert!(replacement.contains("Run the local push yourself"));
        assert!(replacement.contains("test-host"));
    }
    #[test]
    fn traversal_and_secret_requests_rejected() {
        for path in [
            "../secret",
            "/etc/passwd",
            ".git/config",
            ".jcode/auth",
            "a/../../b",
            "a\\b",
        ] {
            assert!(validate_resource(&Resource::ProjectFile { path: path.into() }).is_err());
        }
        for raw in [
            br#"{"action":"status","value":"secret"}"#.as_slice(),
            br#"{"action":"sync","content":"secret"}"#,
            br#"{"action":"shell"}"#,
            br#"{"action":"fetch"}"#,
        ] {
            assert!(parse_request(raw).is_err());
        }
        assert!(parse_request(br#"{"action":"status"}"#).is_ok());
        assert!(parse_request(br#"{"action":"sync"}"#).is_ok());
        for kind in ["OPENSSH ", "RSA ", "EC ", "DSA ", "", "ENCRYPTED "] {
            let header = format!("-----BEGIN {kind}PRIVATE KEY-----");
            assert!(validate_project_bytes(header.as_bytes()).is_err(), "{kind}");
            assert!(validate_environment_value(&header).is_err(), "{kind}");
        }
        assert!(
            validate_resource(&Resource::EnvVar {
                name: "SSH_PRIVATE_KEY".into()
            })
            .is_err()
        );
        assert!(credential_paths("ssh").is_err());
        assert!(parse_request(br#"{"action":"fetch","resource":{"kind":"env_var","name":"TOKEN","value":"secret"}}"#).is_err());
    }
    #[test]
    fn opaque_publish_and_metadata() {
        let root = root();
        assert!(publish_private(&root, Path::new("nested/blob"), b"\0secret\xff").unwrap());
        assert!(!publish_private(&root, Path::new("nested/blob"), b"replacement").unwrap());
        assert_eq!(
            read_private(&root, Path::new("nested/blob")).unwrap(),
            b"\0secret\xff"
        );
        save_requirement(
            &root,
            &Resource::EnvVar {
                name: "TOKEN".into(),
            },
        )
        .unwrap();
        save_requirement(&root, &Resource::Credential { name: "aws".into() }).unwrap();
        let metadata = std::fs::read_to_string(root.join(manifest_path())).unwrap();
        assert!(!metadata.contains("secret"));
        assert_eq!(load_manifest(&root).unwrap().requirements.len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.join("nested/blob"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn existing_credentials_are_preserved() {
        let root = root();
        let path = Path::new(credential_paths("aws").unwrap()[0]);
        assert!(publish_private(&root, path, b"existing-secret").unwrap());
        assert!(!publish_private(&root, path, b"incoming-secret").unwrap());
        assert_eq!(read_private(&root, path).unwrap(), b"existing-secret");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn symlinks_denied() {
        let root = root();
        std::os::unix::fs::symlink("/tmp", root.join("escape")).unwrap();
        assert!(publish_private(&root, Path::new("escape/value"), b"secret").is_err());
        assert!(read_private(&root, Path::new("escape/value")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lease_expiry_and_return() {
        let root = root();
        let now = chrono::Utc::now();
        let mut lease = crate::storage::SessionLease {
            session_id: "id".into(),
            epoch: 1,
            away_host: Some("remote".into()),
            repo_root: Some(root.to_str().unwrap().into()),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: now.to_rfc3339(),
        };
        assert!(validate_lease(&lease, "id", "remote", now).is_ok());
        assert!(validate_lease(&lease, "id", "wrong", now).is_err());
        assert!(
            validate_lease(
                &lease,
                "id",
                "remote",
                now - chrono::Duration::milliseconds(1)
            )
            .is_err()
        );
        let epoch = lease.epoch;
        lease.epoch = 0;
        assert!(validate_lease(&lease, "id", "remote", now).is_err());
        lease.epoch = epoch;
        assert!(
            validate_lease(&lease, "id", "remote", now + chrono::Duration::seconds(900)).is_err()
        );
        lease.away_host = None;
        assert!(validate_lease(&lease, "id", "remote", now).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
