//! Explicit, one-time transfer of one Jcode-owned OpenAI or Claude OAuth account.
//!
//! This is not credential discovery or synchronization. Only the selected/active
//! account in `openai-auth.json` or `auth.json` is read. External tools (including
//! previously trusted tools), keychains, environment credentials, API keys, AWS
//! credentials and general configuration are deliberately outside the supported
//! set. Callers must obtain consent before exporting and use a private transport.
//! No provider loaders are used: those may migrate, harden, refresh or discover
//! credentials. Export never writes to its source.
//!
//! Import refuses *every* existing destination store, including an empty,
//! malformed, symlinked or other-provider-only store. Existing provider writers
//! do not share a lock, so read/merge/rename would silently clobber concurrent
//! updates. A no-replace atomic publication is the only supported operation.
//! Other stores are never opened or modified. This is a copy, not a token move:
//! providers may later invalidate either copy when rotating refresh tokens.
//! OpenAI expiry prefers the stored timestamp, then the token's unverified JWT
//! `exp` claim. This is an offline expiry check, not authentication validation.
//! Opaque tokens without an explicit expiry remain supported.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

pub const MAX_TRANSFER_BYTES: usize = 64 * 1024;
const MAX_STORE_BYTES: usize = 1024 * 1024;
const TRANSFER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferProvider {
    OpenAi,
    Claude,
}

impl TransferProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Claude => "claude",
        }
    }

    fn store_name(self) -> &'static str {
        match self {
            Self::OpenAi => "openai-auth.json",
            Self::Claude => "auth.json",
        }
    }
}

impl FromStr for TransferProvider {
    type Err = TransferError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "openai" => Ok(Self::OpenAi),
            "claude" => Ok(Self::Claude),
            _ => Err(TransferError::UnsupportedProvider),
        }
    }
}

/// Only static diagnostics. Never attach a parser/I/O error or credential value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    UnsupportedProvider,
    UnsupportedPlatform,
    Unavailable,
    InvalidStore,
    InvalidPayload,
    UnsupportedVersion,
    ProviderMismatch,
    TooLarge,
    Expired,
    ExistingStore,
    UnsafePath,
    Io,
}

impl TransferError {
    pub fn message(self) -> &'static str {
        match self {
            Self::UnsupportedProvider => {
                "Local login import supports only OpenAI and Claude OAuth."
            }
            Self::UnsupportedPlatform => {
                "Secure local login import is not supported on this platform."
            }
            Self::Unavailable => {
                "No selected Jcode-owned OAuth login is available for transfer. Use /login instead."
            }
            Self::InvalidStore => "The local OAuth store is malformed. Use /login instead.",
            Self::InvalidPayload => "The credential transfer payload is invalid.",
            Self::UnsupportedVersion => "The credential transfer version is not supported.",
            Self::ProviderMismatch => {
                "The credential transfer does not match the selected provider."
            }
            Self::TooLarge => "The credential transfer or source store exceeds the size limit.",
            Self::Expired => "The selected OAuth login has expired. Use /login instead.",
            Self::ExistingStore => {
                "The selected provider's destination store already exists. It was not changed. Use /login instead."
            }
            Self::UnsafePath => {
                "The credential store path is not a safe regular file or private directory."
            }
            Self::Io => "The credential store could not be accessed securely.",
        }
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TransferError {}

/// Secret bytes, intentionally without Debug, Display, Clone or Serialize.
/// Do not log these bytes, put them in argv, or persist a transport copy.
pub struct CredentialTransfer {
    bytes: Vec<u8>,
}

impl CredentialTransfer {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    provider: TransferProvider,
    credential: T,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAiCredential {
    access_token: String,
    refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeCredential {
    access: String,
    refresh: String,
    expires: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_type: Option<String>,
}

fn validate_tokens(access: &str, refresh: &str, expires: Option<i64>) -> Result<(), TransferError> {
    if access.trim().is_empty() || refresh.trim().is_empty() {
        return Err(TransferError::InvalidPayload);
    }
    if expires.is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp_millis()) {
        return Err(TransferError::Expired);
    }
    Ok(())
}

fn serialize<T: Serialize>(
    provider: TransferProvider,
    credential: T,
) -> Result<CredentialTransfer, TransferError> {
    let bytes = serde_json::to_vec(&Envelope {
        version: TRANSFER_VERSION,
        provider,
        credential,
    })
    .map_err(|_| TransferError::InvalidPayload)?;
    if bytes.len() > MAX_TRANSFER_BYTES {
        return Err(TransferError::TooLarge);
    }
    Ok(CredentialTransfer { bytes })
}

/// Read-only availability, without refresh, migration or external discovery.
/// A missing/expired login is unavailable. Malformed stores remain errors.
pub fn available_local(provider: TransferProvider) -> Result<bool, TransferError> {
    match export_local(provider) {
        Ok(_) => Ok(true),
        Err(TransferError::Unavailable | TransferError::Expired) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Read-only availability in an explicitly isolated Jcode data directory.
pub fn available_at(home: &Path, provider: TransferProvider) -> Result<bool, TransferError> {
    match export_at(home, provider) {
        Ok(_) => Ok(true),
        Err(TransferError::Unavailable | TransferError::Expired) => Ok(false),
        Err(error) => Err(error),
    }
}

fn validate_labels<'a>(labels: impl Iterator<Item = &'a str>) -> Result<(), TransferError> {
    let mut seen = std::collections::HashSet::new();
    for label in labels {
        if label.trim().is_empty() || !seen.insert(label) {
            return Err(TransferError::InvalidStore);
        }
    }
    Ok(())
}

/// Export exactly the runtime-selected, stored-active, or first account, in that
/// order. An invalid explicit/active selection is refused, never substituted.
pub fn export_local(provider: TransferProvider) -> Result<CredentialTransfer, TransferError> {
    let home = crate::storage::jcode_dir().map_err(|_| TransferError::Io)?;
    let selected = super::account_store::runtime_active_override(provider.as_str());
    export_account_at(&home, provider, selected.as_deref())
}

/// Isolated-store equivalent of export, without process-global account overrides.
pub fn export_at(
    home: &Path,
    provider: TransferProvider,
) -> Result<CredentialTransfer, TransferError> {
    export_account_at(home, provider, None)
}

/// Read a specifically selected account, or the stored-active/first account.
/// `home` is a Jcode data directory, not a user home or external tool directory.
pub fn export_account_at(
    home: &Path,
    provider: TransferProvider,
    selected: Option<&str>,
) -> Result<CredentialTransfer, TransferError> {
    let bytes = read_store(&home.join(provider.store_name()))?;
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(TransferError::InvalidStore);
    }
    match provider {
        TransferProvider::OpenAi => {
            let store: super::codex::JcodeOpenAiAuthFile =
                serde_json::from_slice(&bytes).map_err(|_| TransferError::InvalidStore)?;
            validate_labels(
                store
                    .openai_accounts
                    .iter()
                    .map(|account| account.label.as_str()),
            )?;
            let label = selected.or(store.active_openai_account.as_deref());
            let account = match label {
                Some(label) => store
                    .openai_accounts
                    .iter()
                    .find(|account| account.label == label),
                None => store.openai_accounts.first(),
            }
            .ok_or(TransferError::Unavailable)?;
            let expires_at = account
                .expires_at
                .or_else(|| super::codex::expires_at_from_access_token(&account.access_token));
            validate_tokens(&account.access_token, &account.refresh_token, expires_at)?;
            serialize(
                provider,
                OpenAiCredential {
                    access_token: account.access_token.clone(),
                    refresh_token: account.refresh_token.clone(),
                    id_token: account.id_token.clone(),
                    account_id: account.account_id.clone(),
                    expires_at,
                },
            )
        }
        TransferProvider::Claude => {
            // Read legacy accounts without invoking the mutating migration loader.
            #[derive(Deserialize)]
            struct Store {
                #[serde(default)]
                anthropic_accounts: Vec<super::claude::AnthropicAccount>,
                active_anthropic_account: Option<String>,
                anthropic: Option<Legacy>,
            }
            #[derive(Deserialize)]
            struct Legacy {
                access: String,
                refresh: String,
                expires: i64,
            }
            let store: Store =
                serde_json::from_slice(&bytes).map_err(|_| TransferError::InvalidStore)?;
            validate_labels(
                store
                    .anthropic_accounts
                    .iter()
                    .map(|account| account.label.as_str()),
            )?;
            let label = selected.or(store.active_anthropic_account.as_deref());
            let credential = if store.anthropic_accounts.is_empty() && label.is_none() {
                let legacy = store.anthropic.ok_or(TransferError::Unavailable)?;
                ClaudeCredential {
                    access: legacy.access,
                    refresh: legacy.refresh,
                    expires: legacy.expires,
                    scopes: Vec::new(),
                    subscription_type: None,
                }
            } else {
                let account = match label {
                    Some(label) => store
                        .anthropic_accounts
                        .iter()
                        .find(|account| account.label == label),
                    None => store.anthropic_accounts.first(),
                }
                .ok_or(TransferError::Unavailable)?;
                ClaudeCredential {
                    access: account.access.clone(),
                    refresh: account.refresh.clone(),
                    expires: account.expires,
                    scopes: account.scopes.clone(),
                    subscription_type: account.subscription_type.clone(),
                }
            };
            validate_tokens(
                &credential.access,
                &credential.refresh,
                Some(credential.expires),
            )?;
            serialize(provider, credential)
        }
    }
}

fn read_store(path: &Path) -> Result<Vec<u8>, TransferError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TransferError::Unavailable
        } else {
            TransferError::Io
        }
    })?;
    let metadata = file.metadata().map_err(|_| TransferError::Io)?;
    if !metadata.is_file() {
        return Err(TransferError::UnsafePath);
    }
    if metadata.len() > MAX_STORE_BYTES as u64 {
        return Err(TransferError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take((MAX_STORE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| TransferError::Io)?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err(TransferError::TooLarge);
    }
    Ok(bytes)
}

/// Validate a bounded versioned payload and atomically create the selected
/// provider's store. Never merges with or replaces an existing destination.
pub fn import_local(provider: TransferProvider, bytes: &[u8]) -> Result<(), TransferError> {
    let home = crate::storage::jcode_dir().map_err(|_| TransferError::Io)?;
    import_at(&home, provider, bytes)?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn import_at(
    home: &Path,
    provider: TransferProvider,
    bytes: &[u8],
) -> Result<(), TransferError> {
    if bytes.len() > MAX_TRANSFER_BYTES {
        return Err(TransferError::TooLarge);
    }
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(TransferError::InvalidPayload);
    }
    let envelope: Envelope<Box<serde_json::value::RawValue>> =
        serde_json::from_slice(bytes).map_err(|_| TransferError::InvalidPayload)?;
    if envelope.version != TRANSFER_VERSION {
        return Err(TransferError::UnsupportedVersion);
    }
    if envelope.provider != provider {
        return Err(TransferError::ProviderMismatch);
    }
    if !envelope.credential.get().starts_with('{') {
        return Err(TransferError::InvalidPayload);
    }
    let store = match provider {
        TransferProvider::OpenAi => {
            let mut credential: OpenAiCredential = serde_json::from_str(envelope.credential.get())
                .map_err(|_| TransferError::InvalidPayload)?;
            credential.expires_at = credential
                .expires_at
                .or_else(|| super::codex::expires_at_from_access_token(&credential.access_token));
            validate_tokens(
                &credential.access_token,
                &credential.refresh_token,
                credential.expires_at,
            )?;
            serde_json::to_vec(&super::codex::JcodeOpenAiAuthFile {
                openai_accounts: vec![super::codex::OpenAiAccount {
                    label: "openai-otter".into(),
                    access_token: credential.access_token,
                    refresh_token: credential.refresh_token,
                    id_token: credential.id_token,
                    account_id: credential.account_id,
                    expires_at: credential.expires_at,
                    email: None,
                }],
                active_openai_account: Some("openai-otter".into()),
            })
        }
        TransferProvider::Claude => {
            let credential: ClaudeCredential = serde_json::from_str(envelope.credential.get())
                .map_err(|_| TransferError::InvalidPayload)?;
            validate_tokens(
                &credential.access,
                &credential.refresh,
                Some(credential.expires),
            )?;
            #[derive(Serialize)]
            struct Store {
                anthropic_accounts: Vec<super::claude::AnthropicAccount>,
                active_anthropic_account: &'static str,
            }
            serde_json::to_vec(&Store {
                anthropic_accounts: vec![super::claude::AnthropicAccount {
                    label: "claude-otter".into(),
                    access: credential.access,
                    refresh: credential.refresh,
                    expires: credential.expires,
                    email: None,
                    scopes: credential.scopes,
                    subscription_type: credential.subscription_type,
                }],
                active_anthropic_account: "claude-otter",
            })
        }
    }
    .map_err(|_| TransferError::InvalidPayload)?;
    secure_publish(home, provider.store_name(), &store)
}

#[cfg(not(unix))]
fn secure_publish(_: &Path, _: &str, _: &[u8]) -> Result<(), TransferError> {
    // Do not pretend Unix permission bits enforce a private Windows ACL.
    Err(TransferError::UnsupportedPlatform)
}

#[cfg(unix)]
fn secure_publish(home: &Path, name: &str, bytes: &[u8]) -> Result<(), TransferError> {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    // Walk directory descriptors, never following symlinks, including ancestors.
    // Publication remains anchored even if a parent path is renamed concurrently.
    let start = if home.is_absolute() { c"/" } else { c"." };
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let fd = unsafe { libc::open(start.as_ptr(), flags) };
    if fd < 0 {
        return Err(TransferError::Io);
    }
    let mut directory = unsafe { File::from_raw_fd(fd) };
    let mut saw_component = false;
    for component in home.components() {
        let component = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(component) => component,
            _ => return Err(TransferError::UnsafePath),
        };
        saw_component = true;
        let component =
            CString::new(component.as_bytes()).map_err(|_| TransferError::UnsafePath)?;
        let mut fd = unsafe { libc::openat(directory.as_raw_fd(), component.as_ptr(), flags) };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            let created =
                unsafe { libc::mkdirat(directory.as_raw_fd(), component.as_ptr(), 0o700) };
            if created < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(TransferError::Io);
            }
            fd = unsafe { libc::openat(directory.as_raw_fd(), component.as_ptr(), flags) };
        }
        if fd < 0 {
            return Err(TransferError::UnsafePath);
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    if !saw_component {
        return Err(TransferError::UnsafePath);
    }
    let metadata = directory.metadata().map_err(|_| TransferError::Io)?;
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(TransferError::UnsafePath);
    }
    let target = CString::new(name).map_err(|_| TransferError::UnsafePath)?;
    // lstat-style check rejects dangling symlinks, directories and malformed files
    // without reading any existing credentials. linkat below is the race guard.
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let exists = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            target.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if exists == 0 {
        return Err(TransferError::ExistingStore);
    }
    if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
        return Err(TransferError::Io);
    }
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
        return Err(TransferError::Io);
    }

    struct Staged<'a> {
        directory: &'a File,
        name: CString,
    }
    impl Drop for Staged<'_> {
        fn drop(&mut self) {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0);
            }
        }
    }
    let temporary = CString::new(format!(".oauth-transfer-{}", uuid::Uuid::new_v4()))
        .map_err(|_| TransferError::Io)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(TransferError::Io);
    }
    let staged = Staged {
        directory: &directory,
        name: temporary,
    };
    let mut file = unsafe { File::from_raw_fd(fd) };
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        return Err(TransferError::Io);
    }
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| TransferError::Io)?;
    // Hard-link creation is atomic and cannot replace an existing destination.
    let result = unsafe {
        libc::linkat(
            directory.as_raw_fd(),
            staged.name.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
            0,
        )
    };
    if result != 0 {
        return Err(
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                TransferError::ExistingStore
            } else {
                TransferError::Io
            },
        );
    }
    drop(staged);
    // Publication has already committed. A directory sync failure must not imply
    // the credential was not installed, encouraging a retry or exposing a token.
    let _ = directory.sync_all();
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::sync::{Arc, Barrier};

    const FUTURE: i64 = 4_102_444_800_000;

    fn fixture(provider: TransferProvider, token: &str) -> Vec<u8> {
        let credential = match provider {
            TransferProvider::OpenAi => json!({
                "access_token": token, "refresh_token": "synthetic-refresh",
                "expires_at": FUTURE, "account_id": "synthetic-account"
            }),
            TransferProvider::Claude => json!({
                "access": token, "refresh": "synthetic-refresh", "expires": FUTURE,
                "scopes": ["user:inference"], "subscription_type": "max"
            }),
        };
        serde_json::to_vec(&json!({"version": 1, "provider": provider, "credential": credential}))
            .unwrap()
    }

    fn source(home: &Path, provider: TransferProvider) -> Vec<u8> {
        let accounts = match provider {
            TransferProvider::OpenAi => json!({
                "openai_accounts": [
                    {"label": "first", "access_token": "not-selected", "refresh_token": "not-selected-refresh", "expires_at": FUTURE},
                    {"label": "second", "access_token": "selected", "refresh_token": "selected-refresh", "expires_at": FUTURE, "email": "not-copied@example.invalid"}
                ], "active_openai_account": "second", "unrelated": "do-not-copy"
            }),
            TransferProvider::Claude => json!({
                "anthropic_accounts": [
                    {"label": "first", "access": "not-selected", "refresh": "not-selected-refresh", "expires": FUTURE},
                    {"label": "second", "access": "selected", "refresh": "selected-refresh", "expires": FUTURE, "email": "not-copied@example.invalid"}
                ], "active_anthropic_account": "second", "unrelated": "do-not-copy"
            }),
        };
        let bytes = serde_json::to_vec(&accounts).unwrap();
        fs::write(home.join(provider.store_name()), &bytes).unwrap();
        bytes
    }

    #[test]
    fn round_trip_only_active_account_leaves_source_and_other_stores_unchanged() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let local = tempfile::tempdir().unwrap();
            let remote = tempfile::tempdir().unwrap();
            let home = remote.path().join("jcode");
            fs::create_dir(&home).unwrap();
            let original = source(local.path(), provider);
            let path = local.path().join(provider.store_name());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            let before = fs::metadata(&path).unwrap();
            let other = match provider {
                TransferProvider::OpenAi => "auth.json",
                TransferProvider::Claude => "openai-auth.json",
            };
            for name in [other, "config.toml", "openai.env", "anthropic.env"] {
                fs::write(home.join(name), b"unrelated sentinel").unwrap();
            }
            let payload = export_at(local.path(), provider).unwrap();
            let text = std::str::from_utf8(payload.as_bytes()).unwrap();
            assert!(text.contains("selected-refresh"));
            for excluded in ["not-selected", "not-copied", "do-not-copy", "second"] {
                assert!(!text.contains(excluded));
            }
            import_at(&home, provider, payload.as_bytes()).unwrap();
            let imported = export_at(&home, provider).unwrap();
            assert!(payload.as_bytes() == imported.as_bytes());
            assert!(fs::read(&path).unwrap() == original);
            let after = fs::metadata(&path).unwrap();
            assert_eq!(before.mode(), after.mode());
            assert_eq!(before.mtime(), after.mtime());
            assert_eq!(before.mtime_nsec(), after.mtime_nsec());
            assert_eq!(fs::metadata(&home).unwrap().mode() & 0o777, 0o700);
            assert_eq!(
                fs::metadata(home.join(provider.store_name()))
                    .unwrap()
                    .mode()
                    & 0o777,
                0o600
            );
            for name in [other, "config.toml", "openai.env", "anthropic.env"] {
                assert!(fs::read(home.join(name)).unwrap() == b"unrelated sentinel");
            }
            let installed = fs::read(home.join(provider.store_name())).unwrap();
            assert_eq!(
                import_at(&home, provider, payload.as_bytes()),
                Err(TransferError::ExistingStore)
            );
            assert!(fs::read(home.join(provider.store_name())).unwrap() == installed);
        }
    }

    #[test]
    fn explicit_selection_is_respected_and_missing_selection_never_falls_back() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let local = tempfile::tempdir().unwrap();
            source(local.path(), provider);
            let chosen = export_account_at(local.path(), provider, Some("first")).unwrap();
            assert!(
                std::str::from_utf8(chosen.as_bytes())
                    .unwrap()
                    .contains("not-selected-refresh")
            );
            assert!(matches!(
                export_account_at(local.path(), provider, Some("missing")),
                Err(TransferError::Unavailable)
            ));
        }
    }

    #[test]
    fn availability_never_discovers_external_or_unrelated_credentials() {
        let local = tempfile::tempdir().unwrap();
        for (directory, filename) in [
            (".codex", "auth.json"),
            (".claude", ".credentials.json"),
            (".local/share/opencode", "auth.json"),
            (".aws", "credentials"),
        ] {
            let directory = local.path().join(directory);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(filename), b"external sentinel").unwrap();
        }
        let home = local.path().join("jcode");
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            assert_eq!(available_at(&home, provider), Ok(false));
            assert!(!home.exists());
        }
        fs::create_dir(&home).unwrap();
        source(&home, TransferProvider::OpenAi);
        assert_eq!(available_at(&home, TransferProvider::OpenAi), Ok(true));
        assert_eq!(available_at(&home, TransferProvider::Claude), Ok(false));
        let path = home.join("openai-auth.json");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, original.replace("\"first\"", "\"second\"")).unwrap();
        assert_eq!(
            available_at(&home, TransferProvider::OpenAi),
            Err(TransferError::InvalidStore)
        );
        for (directory, filename) in [
            (".codex", "auth.json"),
            (".claude", ".credentials.json"),
            (".local/share/opencode", "auth.json"),
            (".aws", "credentials"),
        ] {
            assert!(
                fs::read(local.path().join(directory).join(filename)).unwrap()
                    == b"external sentinel"
            );
        }
    }

    #[test]
    fn supported_provider_set_and_expiry_rules_are_explicit() {
        assert_eq!("openai".parse(), Ok(TransferProvider::OpenAi));
        assert_eq!("claude".parse(), Ok(TransferProvider::Claude));
        for unsupported in ["aws", "anthropic", "codex", "auto", "OPENAI", ""] {
            assert_eq!(
                unsupported.parse::<TransferProvider>(),
                Err(TransferError::UnsupportedProvider)
            );
        }
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let remote = tempfile::tempdir().unwrap();
            let mut payload: Value =
                serde_json::from_slice(&fixture(provider, "synthetic")).unwrap();
            let expiry = match provider {
                TransferProvider::OpenAi => "expires_at",
                TransferProvider::Claude => "expires",
            };
            for expired in [0, -1, 1] {
                payload["credential"][expiry] = json!(expired);
                assert_eq!(
                    import_at(
                        remote.path(),
                        provider,
                        &serde_json::to_vec(&payload).unwrap()
                    ),
                    Err(TransferError::Expired)
                );
            }
            payload["credential"]
                .as_object_mut()
                .unwrap()
                .remove(expiry);
            let result = import_at(
                remote.path(),
                provider,
                &serde_json::to_vec(&payload).unwrap(),
            );
            if provider == TransferProvider::OpenAi {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(result, Err(TransferError::InvalidPayload));
            }
        }
    }

    #[test]
    fn openai_jwt_expiry_fallback_is_checked_and_persisted_without_source_mutation() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let jwt = |seconds: i64| {
            let claims = serde_json::to_vec(&json!({"exp": seconds})).unwrap();
            format!("synthetic.{}.unsigned", URL_SAFE_NO_PAD.encode(claims))
        };
        let provider = TransferProvider::OpenAi;
        for (token, explicit, expected) in [
            (jwt(1), None, Some(1_000)),
            (jwt(FUTURE / 1_000), None, Some(FUTURE)),
            (jwt(1), Some(FUTURE), Some(FUTURE)),
            ("synthetic-opaque-token".to_string(), None, None),
        ] {
            let local = tempfile::tempdir().unwrap();
            let remote = tempfile::tempdir().unwrap();
            let source = serde_json::to_vec(&json!({
                "openai_accounts": [{
                    "label": "active", "access_token": token,
                    "refresh_token": "synthetic-refresh", "expires_at": explicit
                }], "active_openai_account": "active"
            }))
            .unwrap();
            let path = local.path().join("openai-auth.json");
            fs::write(&path, &source).unwrap();
            let incoming = serde_json::to_vec(&json!({
                "version": 1, "provider": "openai", "credential": {
                    "access_token": token, "refresh_token": "synthetic-refresh",
                    "expires_at": explicit
                }
            }))
            .unwrap();
            if expected == Some(1_000) {
                assert!(matches!(
                    export_at(local.path(), provider),
                    Err(TransferError::Expired)
                ));
                assert_eq!(
                    import_at(remote.path(), provider, &incoming),
                    Err(TransferError::Expired)
                );
                assert_eq!(fs::read_dir(remote.path()).unwrap().count(), 0);
            } else {
                let exported = export_at(local.path(), provider).unwrap();
                let value: Value = serde_json::from_slice(exported.as_bytes()).unwrap();
                assert_eq!(value["credential"]["expires_at"].as_i64(), expected);
                // Exercise incoming envelopes without the export-side normalization.
                import_at(remote.path(), provider, &incoming).unwrap();
                let installed: Value = serde_json::from_slice(
                    &fs::read(remote.path().join("openai-auth.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(
                    installed["openai_accounts"][0]["expires_at"].as_i64(),
                    expected
                );
                let reexported = export_at(remote.path(), provider).unwrap();
                assert!(exported.as_bytes() == reexported.as_bytes());
            }
            assert!(fs::read(path).unwrap() == source);
        }
    }

    #[test]
    fn legacy_claude_export_does_not_migrate_the_source() {
        let local = tempfile::tempdir().unwrap();
        let bytes = serde_json::to_vec(&json!({"anthropic":{"access":"legacy-access","refresh":"legacy-refresh","expires":FUTURE}})).unwrap();
        fs::write(local.path().join("auth.json"), &bytes).unwrap();
        let payload = export_at(local.path(), TransferProvider::Claude).unwrap();
        assert!(
            std::str::from_utf8(payload.as_bytes())
                .unwrap()
                .contains("legacy-access")
        );
        assert!(fs::read(local.path().join("auth.json")).unwrap() == bytes);
    }

    #[test]
    fn every_existing_destination_is_refused_without_reading_or_modifying_it() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            for existing in [
                b"".as_slice(),
                b"{",
                b"{}",
                b"{\"other_provider\":{\"access\":\"untouched\"}}",
                b"{\"OPENAI_API_KEY\":\"untouched\"}",
            ] {
                let remote = tempfile::tempdir().unwrap();
                let path = remote.path().join(provider.store_name());
                fs::write(&path, existing).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                let before = fs::metadata(&path).unwrap();
                assert_eq!(
                    import_at(remote.path(), provider, &fixture(provider, "new")),
                    Err(TransferError::ExistingStore)
                );
                assert!(fs::read(&path).unwrap() == existing);
                assert_eq!(before.mode(), fs::metadata(&path).unwrap().mode());
                assert_eq!(fs::read_dir(remote.path()).unwrap().count(), 1);
            }
        }
    }

    #[test]
    fn payload_validation_precedes_all_filesystem_writes_and_errors_are_static() {
        let remote = tempfile::tempdir().unwrap();
        let home = remote.path().join("not-created");
        let provider = TransferProvider::OpenAi;
        let valid: Value =
            serde_json::from_slice(&fixture(provider, "synthetic-sensitive-marker")).unwrap();
        let cases = [
            ("version", json!(99), TransferError::UnsupportedVersion),
            ("provider", json!("claude"), TransferError::ProviderMismatch),
            ("provider", json!("aws"), TransferError::InvalidPayload),
            (
                "unexpected",
                json!("synthetic-sensitive-marker"),
                TransferError::InvalidPayload,
            ),
            (
                "credential",
                json!({"access_token":"synthetic-sensitive-marker","refresh_token":""}),
                TransferError::InvalidPayload,
            ),
            (
                "credential",
                json!({"access_token":"synthetic-sensitive-marker","refresh_token":"r","expires_at":1}),
                TransferError::Expired,
            ),
            (
                "credential",
                json!({"access_token":"synthetic-sensitive-marker","refresh_token":"r","api_key":"forbidden"}),
                TransferError::InvalidPayload,
            ),
            (
                "credential",
                json!({"access_token":"synthetic-sensitive-marker","refresh_token":"r","expires_at":"not-an-integer"}),
                TransferError::InvalidPayload,
            ),
        ];
        for (key, value, expected) in cases {
            let mut malformed = valid.clone();
            malformed[key] = value;
            let error =
                import_at(&home, provider, &serde_json::to_vec(&malformed).unwrap()).unwrap_err();
            assert_eq!(error, expected);
            assert!(!format!("{error:?}: {error}").contains("synthetic-sensitive-marker"));
            assert!(!home.exists());
        }
        for bytes in [b"{synthetic-sensitive-marker".as_slice(), b"null", b"[]", b"{\"version\":1,\"version\":1,\"provider\":\"openai\",\"credential\":{}}", b"{\"version\":1,\"provider\":\"openai\",\"credential\":{\"access_token\":\"a\",\"access_token\":\"b\",\"refresh_token\":\"r\"}}"] {
            assert_eq!(import_at(&home, provider, bytes), Err(TransferError::InvalidPayload));
            assert!(!home.exists());
        }
        assert_eq!(
            import_at(&home, provider, &vec![b' '; MAX_TRANSFER_BYTES + 1]),
            Err(TransferError::TooLarge)
        );
        assert!(!home.exists());
    }

    #[test]
    fn source_bounds_malformed_and_expired_stores_fail_closed() {
        let local = tempfile::tempdir().unwrap();
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            assert!(matches!(
                export_at(local.path(), provider),
                Err(TransferError::Unavailable)
            ));
            let path = local.path().join(provider.store_name());
            for bytes in [
                b"null".as_slice(),
                b"[{}]",
                b"{invalid",
                b"{\"openai_accounts\":false,\"anthropic_accounts\":false}",
            ] {
                fs::write(&path, bytes).unwrap();
                assert!(matches!(
                    export_at(local.path(), provider),
                    Err(TransferError::InvalidStore)
                ));
                assert!(fs::read(&path).unwrap() == bytes);
            }
            fs::write(&path, vec![b' '; MAX_STORE_BYTES + 1]).unwrap();
            assert!(matches!(
                export_at(local.path(), provider),
                Err(TransferError::TooLarge)
            ));
            let original = source(local.path(), provider);
            let text = String::from_utf8(original)
                .unwrap()
                .replace(&FUTURE.to_string(), "1");
            fs::write(&path, text).unwrap();
            assert!(matches!(
                export_at(local.path(), provider),
                Err(TransferError::Expired)
            ));
            let original = source(local.path(), provider);
            let text = String::from_utf8(original)
                .unwrap()
                .replace("selected-refresh", &"x".repeat(MAX_TRANSFER_BYTES));
            fs::write(&path, text).unwrap();
            assert!(matches!(
                export_at(local.path(), provider),
                Err(TransferError::TooLarge)
            ));
        }
    }

    #[test]
    fn symlink_and_nonregular_destinations_cannot_be_followed() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote");
        fs::create_dir(&remote).unwrap();
        let target = root.path().join("unrelated");
        fs::write(&target, b"unchanged").unwrap();
        let destination = remote.join("auth.json");
        symlink(&target, &destination).unwrap();
        let payload = fixture(TransferProvider::Claude, "new");
        assert_eq!(
            import_at(&remote, TransferProvider::Claude, &payload),
            Err(TransferError::ExistingStore)
        );
        assert!(fs::read(&target).unwrap() == b"unchanged");
        assert!(export_at(&remote, TransferProvider::Claude).is_err());
        fs::remove_file(&destination).unwrap();
        symlink(root.path().join("missing"), &destination).unwrap();
        assert_eq!(
            import_at(&remote, TransferProvider::Claude, &payload),
            Err(TransferError::ExistingStore)
        );
        assert!(!root.path().join("missing").exists());
        fs::remove_file(&destination).unwrap();
        fs::create_dir(&destination).unwrap();
        assert_eq!(
            import_at(&remote, TransferProvider::Claude, &payload),
            Err(TransferError::ExistingStore)
        );
        let alias = root.path().join("alias");
        symlink(&remote, &alias).unwrap();
        assert_eq!(
            import_at(&alias.join("new"), TransferProvider::Claude, &payload),
            Err(TransferError::UnsafePath)
        );
        assert!(!remote.join("new").exists());
    }

    #[test]
    fn racing_imports_have_exactly_one_winner_and_no_partial_or_leftover_files() {
        let remote = tempfile::tempdir().unwrap();
        let home = Arc::new(remote.path().join("jcode"));
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let home = home.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let payload = fixture(
                        TransferProvider::OpenAi,
                        &format!("synthetic-winner-{index}"),
                    );
                    barrier.wait();
                    import_at(&home, TransferProvider::OpenAi, &payload)
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(TransferError::ExistingStore))
                .count(),
            7
        );
        let installed = export_at(&home, TransferProvider::OpenAi).unwrap();
        assert!(
            std::str::from_utf8(installed.as_bytes())
                .unwrap()
                .contains("synthetic-winner-")
        );
        assert_eq!(fs::read_dir(home.as_path()).unwrap().count(), 1);
    }
}
