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
//! Desktop import refuses *every* existing destination store, including an empty,
//! malformed, symlinked or other-provider-only store. Its no-replace publication
//! remains safe even with writers from older binaries that do not share a lock.
//! Cloud imports below use the shared account-store lock for read/merge/rename.
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
    LabelMismatch,
    AccountConflict,
    ConcurrentUpdate,
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
            Self::LabelMismatch => {
                "The cloud credential label does not match the requested account."
            }
            Self::AccountConflict => {
                "The account label belongs to a different or unverifiable identity."
            }
            Self::ConcurrentUpdate => "The account changed since export. It was not replaced.",
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
fn secure_directory(home: &Path, create: bool) -> Result<std::fs::File, TransferError> {
    use std::ffi::CString;
    use std::fs::File;
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
        if create
            && fd < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
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
            return Err(
                if !create && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
                {
                    TransferError::Unavailable
                } else {
                    TransferError::UnsafePath
                },
            );
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
    Ok(directory)
}

#[cfg(unix)]
fn secure_publish(home: &Path, name: &str, bytes: &[u8]) -> Result<(), TransferError> {
    let directory = secure_directory(home, true)?;
    publish_in(&directory, name, bytes, false)
}

#[cfg(unix)]
fn publish_in(
    directory: &std::fs::File,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), TransferError> {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
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
    if exists == 0 && !replace {
        return Err(TransferError::ExistingStore);
    }
    if exists != 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
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
        directory,
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
    let result = if replace {
        unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                staged.name.as_ptr(),
                directory.as_raw_fd(),
                target.as_ptr(),
            )
        }
    } else {
        unsafe {
            libc::linkat(
                directory.as_raw_fd(),
                staged.name.as_ptr(),
                directory.as_raw_fd(),
                target.as_ptr(),
                0,
            )
        }
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

/// Cloud-only secret envelope. Deliberately has no Debug, Display, Clone or
/// Serialize implementation. Transport only over an authenticated private channel.
pub struct CloudCredentialTransfer {
    bytes: Vec<u8>,
    label: String,
    provider: TransferProvider,
}

impl CloudCredentialTransfer {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn provider(&self) -> TransferProvider {
        self.provider
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudEnvelope {
    version: u32,
    provider: TransferProvider,
    label: String,
    credential: serde_json::Value,
}

fn cloud_keys(
    provider: TransferProvider,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match provider {
        TransferProvider::OpenAi => (
            "openai_accounts",
            "active_openai_account",
            "access_token",
            "refresh_token",
        ),
        TransferProvider::Claude => (
            "anthropic_accounts",
            "active_anthropic_account",
            "access",
            "refresh",
        ),
    }
}

fn valid_cloud_label(label: &str) -> bool {
    !label.trim().is_empty() && label.len() <= 256 && !label.chars().any(char::is_control)
}

// Normalize only known account fields, never copy arbitrary store contents.
// Expired access tokens are intentional: the receiving runtime may refresh them.
fn cloud_account(
    provider: TransferProvider,
    value: &serde_json::Value,
) -> Result<serde_json::Value, TransferError> {
    let normalized = match provider {
        TransferProvider::OpenAi => {
            let account: super::codex::OpenAiAccount =
                serde_json::from_value(value.clone()).map_err(|_| TransferError::InvalidPayload)?;
            serde_json::to_value(account)
        }
        TransferProvider::Claude => {
            let account: super::claude::AnthropicAccount =
                serde_json::from_value(value.clone()).map_err(|_| TransferError::InvalidPayload)?;
            serde_json::to_value(account)
        }
    }
    .map_err(|_| TransferError::InvalidPayload)?;
    let (_, _, access, refresh) = cloud_keys(provider);
    if !normalized["label"].as_str().is_some_and(valid_cloud_label)
        || !normalized[access]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        || !normalized[refresh]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
    {
        return Err(TransferError::InvalidPayload);
    }
    Ok(normalized)
}

fn parse_cloud(
    provider: TransferProvider,
    label: &str,
    bytes: &[u8],
) -> Result<CloudEnvelope, TransferError> {
    if bytes.len() > MAX_TRANSFER_BYTES {
        return Err(TransferError::TooLarge);
    }
    let mut envelope: CloudEnvelope =
        serde_json::from_slice(bytes).map_err(|_| TransferError::InvalidPayload)?;
    if envelope.version != 2 {
        return Err(TransferError::UnsupportedVersion);
    }
    if envelope.provider != provider {
        return Err(TransferError::ProviderMismatch);
    }
    if !valid_cloud_label(label) || envelope.label != label {
        return Err(TransferError::LabelMismatch);
    }
    envelope.credential = cloud_account(provider, &envelope.credential)?;
    if envelope.credential["label"].as_str() != Some(label) {
        return Err(TransferError::LabelMismatch);
    }
    Ok(envelope)
}

fn cloud_store(
    bytes: &[u8],
    provider: TransferProvider,
) -> Result<serde_json::Value, TransferError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| TransferError::InvalidStore)?;
    if !value.is_object() {
        return Err(TransferError::InvalidStore);
    }
    let (accounts, active, _, _) = cloud_keys(provider);
    if let Some(selection) = value.get(active)
        && !selection.is_null()
        && !selection.as_str().is_some_and(valid_cloud_label)
    {
        return Err(TransferError::InvalidStore);
    }
    if let Some(entries) = value.get(accounts) {
        let entries = entries.as_array().ok_or(TransferError::InvalidStore)?;
        let mut labels = std::collections::HashSet::new();
        for entry in entries {
            let label = entry["label"]
                .as_str()
                .filter(|s| valid_cloud_label(s))
                .ok_or(TransferError::InvalidStore)?;
            if !labels.insert(label) {
                return Err(TransferError::InvalidStore);
            }
            cloud_account(provider, entry).map_err(|_| TransferError::InvalidStore)?;
        }
    }
    Ok(value)
}

/// Export an exact named account, or the runtime-selected/stored-active/first
/// Jcode-owned account. No external discovery, migration, refresh or writes.
pub fn export_cloud_local(
    provider: TransferProvider,
    selected: Option<&str>,
) -> Result<CloudCredentialTransfer, TransferError> {
    let home = crate::storage::jcode_dir().map_err(|_| TransferError::Io)?;
    let runtime = super::account_store::runtime_active_override(provider.as_str());
    export_cloud_account_at(&home, provider, selected.or(runtime.as_deref()))
}

/// Enumerate all explicitly stored cloud-transferable accounts. Missing stores
/// are normal, but malformed stores must not silently produce partial transfers.
/// This never discovers credentials from other tools or refreshes tokens.
pub fn cloud_accounts_at(home: &Path) -> Result<Vec<(TransferProvider, String)>, TransferError> {
    #[cfg(not(unix))]
    {
        let _ = home;
        Err(TransferError::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        let directory = match secure_directory(home, false) {
            Ok(directory) => directory,
            Err(TransferError::Unavailable) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut accounts = Vec::new();
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let bytes = match read_in(&directory, provider.store_name(), false) {
                Ok(bytes) => bytes,
                Err(TransferError::Unavailable) => continue,
                Err(error) => return Err(error),
            };
            let store = cloud_store(&bytes, provider)?;
            let (key, _, _, _) = cloud_keys(provider);
            if let Some(entries) = store.get(key).and_then(serde_json::Value::as_array) {
                for entry in entries {
                    let label = entry["label"].as_str().ok_or(TransferError::InvalidStore)?;
                    accounts.push((provider, label.to_owned()));
                }
            }
        }
        Ok(accounts)
    }
}

/// Isolated-store export, without runtime overrides. Legacy unlabeled Claude
/// credentials are refused rather than inventing a label or modifying the store.
pub fn export_cloud_account_at(
    home: &Path,
    provider: TransferProvider,
    selected: Option<&str>,
) -> Result<CloudCredentialTransfer, TransferError> {
    #[cfg(not(unix))]
    {
        let _ = (home, provider, selected);
        Err(TransferError::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        let directory = secure_directory(home, false)?;
        let bytes = read_in(&directory, provider.store_name(), false)?;
        let store = cloud_store(&bytes, provider)?;
        let (accounts, active, _, _) = cloud_keys(provider);
        let entries = store[accounts]
            .as_array()
            .ok_or(TransferError::Unavailable)?;
        let selection = selected.or(store[active].as_str());
        let account = match selection {
            Some(label) => entries
                .iter()
                .find(|entry| entry["label"].as_str() == Some(label)),
            None => entries.first(),
        }
        .ok_or(TransferError::Unavailable)?;
        let credential = cloud_account(provider, account)?;
        let label = credential["label"]
            .as_str()
            .ok_or(TransferError::InvalidStore)?
            .to_owned();
        let bytes = serde_json::to_vec(&CloudEnvelope {
            version: 2,
            provider,
            label: label.clone(),
            credential,
        })
        .map_err(|_| TransferError::InvalidPayload)?;
        if bytes.len() > MAX_TRANSFER_BYTES {
            return Err(TransferError::TooLarge);
        }
        Ok(CloudCredentialTransfer {
            bytes,
            label,
            provider,
        })
    }
}

/// Merge one labeled cloud credential, preserving unrelated accounts, unknown
/// store fields and an existing active selection (including explicit null).
///
/// Read/merge/publication holds the shared account-store lock used by current
/// provider writers. Older binaries and external editors do not honor this lock:
/// quiesce them before importing. A byte recheck detects most such interference,
/// but cannot close the final check/rename race against a noncooperating writer.
///
/// Same-label updates need a matching nonempty OpenAI account_id or Claude email,
/// with all other identity hints unchanged. These are UNVERIFIED stored hints,
/// not authentication evidence: only accept trusted, authenticated transports.
/// Changed tokens also need a strictly newer known expiry, otherwise the return
/// may be stale and is refused. Expiry ordering is not proof of token lineage.
/// For stale-return protection or identity-less accounts use import_cloud_return_at.
/// No method can prove refresh-token lineage offline or prevent server-side
/// rotation conflicts. Do not keep both copies refreshing simultaneously.
pub fn import_cloud_at(
    home: &Path,
    provider: TransferProvider,
    expected_label: &str,
    bytes: &[u8],
) -> Result<(), TransferError> {
    import_cloud_inner(home, provider, expected_label, bytes, None, false)
}

/// Explicit handoff replaces a stale destination copy of the same identity.
/// Unlike a return/merge, the authenticated source is authoritative, including
/// for accounts without identity hints or with equal access-token expiries.
/// Call only after the user has requested transferring their accounts.
pub fn import_cloud_handoff_at(
    home: &Path,
    provider: TransferProvider,
    expected_label: &str,
    bytes: &[u8],
) -> Result<(), TransferError> {
    import_cloud_inner(home, provider, expected_label, bytes, None, true)
}

/// Return updated tokens only if the destination credential still exactly
/// matches the original export. This permits identity-less accounts, provided
/// identity hints remain unchanged, and refuses intervening local token rotation.
/// Retain the actual departure envelope privately until return; never log it.
/// Do NOT take a fresh local export on return as a substitute: that does not
/// establish the cloud credential's lineage and could restore a replaced login.
/// The writer-lock and authenticated-transport requirements above apply.
pub fn import_cloud_return_at(
    home: &Path,
    provider: TransferProvider,
    expected_label: &str,
    bytes: &[u8],
    original: &CloudCredentialTransfer,
) -> Result<(), TransferError> {
    let baseline = parse_cloud(provider, expected_label, original.as_bytes())?;
    import_cloud_inner(
        home,
        provider,
        expected_label,
        bytes,
        Some(baseline.credential),
        false,
    )
}

pub fn import_cloud_local(
    provider: TransferProvider,
    expected_label: &str,
    bytes: &[u8],
) -> Result<(), TransferError> {
    let home = crate::storage::jcode_dir().map_err(|_| TransferError::Io)?;
    import_cloud_at(&home, provider, expected_label, bytes)
}

fn identity_hints_match(
    provider: TransferProvider,
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> bool {
    let fields: &[&str] = match provider {
        TransferProvider::OpenAi => &["account_id", "email"],
        TransferProvider::Claude => &["email"],
    };
    let present = |value: &serde_json::Value| value.as_str().is_some_and(|s| !s.trim().is_empty());
    // A hint missing on either side is not evidence of a different account:
    // older refreshes dropped `email` when the grant omitted id_token. Any hint
    // present on both sides must still match exactly, and at least one hint
    // must be shared so an identity-less entry cannot absorb another account.
    let mut shared = false;
    for key in fields {
        let (a, b) = (&old[*key], &new[*key]);
        if present(a) && present(b) {
            if a != b {
                return false;
            }
            shared = true;
        }
    }
    shared || fields.iter().all(|key| old[*key] == new[*key])
}

fn identity_hints_present(provider: TransferProvider, account: &serde_json::Value) -> bool {
    let fields: &[&str] = match provider {
        TransferProvider::OpenAi => &["account_id", "email"],
        TransferProvider::Claude => &["email"],
    };
    fields
        .iter()
        .any(|key| account[*key].as_str().is_some_and(|s| !s.trim().is_empty()))
}

fn cloud_expiry(provider: TransferProvider, account: &serde_json::Value) -> Option<i64> {
    match provider {
        TransferProvider::Claude => account["expires"].as_i64(),
        TransferProvider::OpenAi => account["expires_at"].as_i64().or_else(|| {
            account["access_token"]
                .as_str()
                .and_then(super::codex::expires_at_from_access_token)
        }),
    }
}

fn import_cloud_inner(
    home: &Path,
    provider: TransferProvider,
    label: &str,
    bytes: &[u8],
    baseline: Option<serde_json::Value>,
    authoritative: bool,
) -> Result<(), TransferError> {
    let envelope = parse_cloud(provider, label, bytes)?;
    #[cfg(not(unix))]
    {
        let _ = (home, envelope, baseline, authoritative);
        Err(TransferError::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;
        let directory = secure_directory(home, true)?;
        if directory.metadata().map_err(|_| TransferError::Io)?.mode() & 0o022 != 0 {
            return Err(TransferError::UnsafePath);
        }
        let _lock = super::account_store::lock_store(home, provider.store_name())
            .map_err(|_| TransferError::Io)?;
        let original = match read_in(&directory, provider.store_name(), true) {
            Ok(bytes) => Some(bytes),
            Err(TransferError::Unavailable) => None,
            Err(error) => return Err(error),
        };
        let mut store = match &original {
            Some(bytes) => cloud_store(bytes, provider)?,
            None => serde_json::json!({}),
        };
        let (accounts, active, _, _) = cloud_keys(provider);
        // A legacy Claude login has no stable original label. Do not silently
        // supersede it by selecting a new named account; let normal login migrate it.
        if provider == TransferProvider::Claude
            && !store["anthropic"].is_null()
            && store[accounts]
                .as_array()
                .is_none_or(|entries| entries.is_empty())
        {
            return Err(TransferError::InvalidStore);
        }
        let entries = store
            .as_object_mut()
            .ok_or(TransferError::InvalidStore)?
            .entry(accounts)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or(TransferError::InvalidStore)?;
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry["label"].as_str() == Some(label))
        {
            let old = cloud_account(provider, existing)?;
            // Explicit handoff: a destination copy with no identity hints at all
            // (older stores) is stale data for this label, not a different account.
            let old_identityless = !identity_hints_present(provider, &old);
            if !(authoritative && old_identityless)
                && !identity_hints_match(provider, &old, &envelope.credential)
            {
                return Err(TransferError::AccountConflict);
            }
            if let Some(baseline) = &baseline {
                if &old != baseline {
                    return Err(TransferError::ConcurrentUpdate);
                }
            } else if !authoritative && old != envelope.credential {
                let identity = match provider {
                    TransferProvider::OpenAi => "account_id",
                    TransferProvider::Claude => "email",
                };
                if !old[identity].as_str().is_some_and(|s| !s.trim().is_empty()) {
                    return Err(TransferError::AccountConflict);
                }
                let (_, _, access, refresh) = cloud_keys(provider);
                if old[access] != envelope.credential[access]
                    || old[refresh] != envelope.credential[refresh]
                {
                    match (
                        cloud_expiry(provider, &old),
                        cloud_expiry(provider, &envelope.credential),
                    ) {
                        (Some(old), Some(new)) if new > old => {}
                        _ => return Err(TransferError::ConcurrentUpdate),
                    }
                }
            }
            // Preserve unknown per-account fields too, updating only known fields.
            let object = existing
                .as_object_mut()
                .ok_or(TransferError::InvalidStore)?;
            let optional_fields: &[&str] = match provider {
                TransferProvider::OpenAi => &["id_token", "account_id", "email", "expires_at"],
                TransferProvider::Claude => &["email", "scopes", "subscription_type"],
            };
            for key in optional_fields {
                object.remove(*key);
            }
            object.extend(
                envelope
                    .credential
                    .as_object()
                    .ok_or(TransferError::InvalidPayload)?
                    .clone(),
            );
        } else {
            if baseline.is_some() {
                return Err(TransferError::ConcurrentUpdate);
            }
            entries.push(envelope.credential);
        }
        if store.get(active).is_none() {
            // Only a new provider account set receives a default active account.
            // Existing nonempty sets without an explicit selection keep first-account semantics.
            if store[accounts].as_array().is_some_and(|a| a.len() == 1) {
                store[active] = serde_json::Value::String(label.to_owned());
            }
        }
        let output = serde_json::to_vec(&store).map_err(|_| TransferError::InvalidStore)?;
        if output.len() > MAX_STORE_BYTES {
            return Err(TransferError::TooLarge);
        }
        let current = match read_in(&directory, provider.store_name(), true) {
            Ok(bytes) => Some(bytes),
            Err(TransferError::Unavailable) => None,
            Err(error) => return Err(error),
        };
        if current != original {
            return Err(TransferError::ConcurrentUpdate);
        }
        if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
            return Err(TransferError::Io);
        }
        publish_in(
            &directory,
            provider.store_name(),
            &output,
            original.is_some(),
        )?;
        super::AuthStatus::invalidate_cache();
        Ok(())
    }
}

#[cfg(unix)]
fn read_in(directory: &std::fs::File, name: &str, private: bool) -> Result<Vec<u8>, TransferError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    let name = std::ffi::CString::new(name).map_err(|_| TransferError::UnsafePath)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                TransferError::Unavailable
            } else {
                TransferError::UnsafePath
            },
        );
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| TransferError::Io)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || (private && metadata.mode() & 0o077 != 0)
    {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::sync::{Arc, Barrier};

    const FUTURE: i64 = 4_102_444_800_000;

    fn cloud_fixture(provider: TransferProvider, label: &str) -> Vec<u8> {
        let credential = match provider {
            TransferProvider::OpenAi => {
                json!({"label": label, "access_token": "synthetic-access", "refresh_token": "synthetic-refresh", "account_id": "synthetic-id", "expires_at": 1})
            }
            TransferProvider::Claude => {
                json!({"label": label, "access": "synthetic-access", "refresh": "synthetic-refresh", "email": "synthetic@example.invalid", "expires": 1})
            }
        };
        serde_json::to_vec(
            &json!({"version": 2, "provider": provider, "label": label, "credential": credential}),
        )
        .unwrap()
    }

    fn revised(bytes: &[u8], key: &str, value: Value) -> Vec<u8> {
        let mut envelope: Value = serde_json::from_slice(bytes).unwrap();
        envelope["credential"][key] = value;
        serde_json::to_vec(&envelope).unwrap()
    }

    #[test]
    fn cloud_enumerates_all_accounts_without_selecting_or_exporting_extra_fields() {
        let home = tempfile::tempdir().unwrap();
        assert!(cloud_accounts_at(home.path()).unwrap().is_empty());
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            for label in ["first", "second"] {
                import_cloud_at(
                    home.path(),
                    provider,
                    label,
                    &cloud_fixture(provider, label),
                )
                .unwrap();
            }
        }
        let accounts = cloud_accounts_at(home.path()).unwrap();
        assert_eq!(
            accounts,
            vec![
                (TransferProvider::OpenAi, "first".into()),
                (TransferProvider::OpenAi, "second".into()),
                (TransferProvider::Claude, "first".into()),
                (TransferProvider::Claude, "second".into()),
            ]
        );
        for (provider, label) in accounts {
            let exported = export_cloud_account_at(home.path(), provider, Some(&label)).unwrap();
            assert_eq!(exported.label(), label);
        }
        fs::write(home.path().join("auth.json"), b"not-json").unwrap();
        assert_eq!(
            cloud_accounts_at(home.path()),
            Err(TransferError::InvalidStore)
        );
    }

    #[test]
    fn cloud_handoff_replaces_stale_rotated_copy_but_not_other_identity() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let home = tempfile::tempdir().unwrap();
            let original = cloud_fixture(provider, "chosen");
            import_cloud_at(home.path(), provider, "chosen", &original).unwrap();
            let (_, _, access, refresh) = cloud_keys(provider);
            let rotated = revised(
                &revised(&original, access, json!("rotated-access")),
                refresh,
                json!("rotated-refresh"),
            );
            // Expiry did not change, so an ordinary unproven merge is refused.
            assert_eq!(
                import_cloud_at(home.path(), provider, "chosen", &rotated),
                Err(TransferError::ConcurrentUpdate)
            );
            import_cloud_handoff_at(home.path(), provider, "chosen", &rotated).unwrap();
            let exported = export_cloud_account_at(home.path(), provider, Some("chosen")).unwrap();
            let value: Value = serde_json::from_slice(exported.as_bytes()).unwrap();
            assert_eq!(value["credential"][refresh], "rotated-refresh");
            let identity = if provider == TransferProvider::OpenAi {
                "account_id"
            } else {
                "email"
            };
            let conflicting = revised(&rotated, identity, json!("different-identity"));
            assert_eq!(
                import_cloud_handoff_at(home.path(), provider, "chosen", &conflicting),
                Err(TransferError::AccountConflict)
            );
        }
    }

    #[test]
    fn cloud_handoff_accepts_destination_entry_missing_identity_hints() {
        // Older refreshes dropped `email` when the grant had no id_token, so a
        // cloud copy can lack hints the laptop still has. That must not block
        // the handoff, while a hint that is present and different still does.
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let home = tempfile::tempdir().unwrap();
            let full = cloud_fixture(provider, "chosen");
            let stripped = revised(
                &revised(&full, "email", Value::Null),
                "account_id",
                Value::Null,
            );
            import_cloud_handoff_at(home.path(), provider, "chosen", &stripped).unwrap();
            import_cloud_handoff_at(home.path(), provider, "chosen", &full).unwrap();
            let exported = export_cloud_account_at(home.path(), provider, Some("chosen")).unwrap();
            let value: Value = serde_json::from_slice(exported.as_bytes()).unwrap();
            let identity = if provider == TransferProvider::OpenAi {
                "account_id"
            } else {
                "email"
            };
            assert!(
                value["credential"][identity]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            );
            let other = revised(&full, identity, json!("someone-else"));
            assert_eq!(
                import_cloud_handoff_at(home.path(), provider, "chosen", &other),
                Err(TransferError::AccountConflict)
            );
        }
    }

    #[test]
    fn cloud_roundtrip_preserves_exact_label_and_expired_refreshable_account() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let local = tempfile::tempdir().unwrap();
            let remote = tempfile::tempdir().unwrap();
            let bytes = cloud_fixture(provider, "my original account");
            import_cloud_at(local.path(), provider, "my original account", &bytes).unwrap();
            let before = fs::read(local.path().join(provider.store_name())).unwrap();
            let metadata = fs::metadata(local.path().join(provider.store_name())).unwrap();
            let exported = export_cloud_account_at(local.path(), provider, None).unwrap();
            assert_eq!(exported.label(), "my original account");
            assert_eq!(exported.provider(), provider);
            assert!(export_cloud_account_at(local.path(), provider, Some("missing")).is_err());
            import_cloud_at(
                remote.path(),
                provider,
                exported.label(),
                exported.as_bytes(),
            )
            .unwrap();
            let returned =
                export_cloud_account_at(remote.path(), provider, Some(exported.label())).unwrap();
            assert!(exported.as_bytes() == returned.as_bytes());
            assert_eq!(
                fs::read(local.path().join(provider.store_name())).unwrap(),
                before
            );
            assert_eq!(
                fs::metadata(local.path().join(provider.store_name()))
                    .unwrap()
                    .mtime_nsec(),
                metadata.mtime_nsec()
            );
            assert_eq!(
                fs::metadata(remote.path().join(provider.store_name()))
                    .unwrap()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(fs::metadata(remote.path()).unwrap().mode() & 0o777, 0o700);
            assert!(matches!(
                export_at(local.path(), provider),
                Err(TransferError::Expired)
            ));
        }
    }

    #[test]
    fn cloud_merge_preserves_unrelated_accounts_fields_and_active_selection() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            for active_value in [json!("second"), Value::Null] {
                let home = tempfile::tempdir().unwrap();
                let source_bytes = source(home.path(), provider);
                let selected = export_cloud_account_at(home.path(), provider, None).unwrap();
                assert_eq!(selected.label(), "second");
                assert_eq!(
                    export_cloud_account_at(home.path(), provider, Some("first"))
                        .unwrap()
                        .label(),
                    "first"
                );
                let envelope: Value = serde_json::from_slice(selected.as_bytes()).unwrap();
                assert!(envelope.get("unrelated").is_none());
                assert!(
                    !std::str::from_utf8(selected.as_bytes())
                        .unwrap()
                        .contains("not-selected-refresh")
                );
                fs::set_permissions(
                    home.path().join(provider.store_name()),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
                let mut old: Value = serde_json::from_slice(&source_bytes).unwrap();
                let (accounts, active, _, _) = cloud_keys(provider);
                old[active] = active_value.clone();
                fs::write(
                    home.path().join(provider.store_name()),
                    serde_json::to_vec(&old).unwrap(),
                )
                .unwrap();
                import_cloud_at(
                    home.path(),
                    provider,
                    "third",
                    &cloud_fixture(provider, "third"),
                )
                .unwrap();
                let current: Value = serde_json::from_slice(
                    &fs::read(home.path().join(provider.store_name())).unwrap(),
                )
                .unwrap();
                assert_eq!(current[active], active_value);
                assert_eq!(current[accounts][0], old[accounts][0]);
                assert_eq!(current[accounts][1], old[accounts][1]);
                assert_eq!(current["unrelated"], old["unrelated"]);
            }
        }
    }

    #[test]
    fn cloud_refresh_requires_same_identity_and_preserves_unknown_account_fields() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let home = tempfile::tempdir().unwrap();
            let bytes = cloud_fixture(provider, "selected");
            import_cloud_at(home.path(), provider, "selected", &bytes).unwrap();
            let path = home.path().join(provider.store_name());
            let (accounts, _, _, refresh) = cloud_keys(provider);
            let mut store: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            store[accounts][0]["future_field"] = json!({"keep": true});
            fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
            let changed = revised(&bytes, refresh, json!("synthetic-rotated"));
            assert_eq!(
                import_cloud_at(home.path(), provider, "selected", &changed),
                Err(TransferError::ConcurrentUpdate)
            );
            let expiry = if provider == TransferProvider::OpenAi {
                "expires_at"
            } else {
                "expires"
            };
            let changed = revised(&changed, expiry, json!(2));
            import_cloud_at(home.path(), provider, "selected", &changed).unwrap();
            assert_eq!(
                import_cloud_at(home.path(), provider, "selected", &bytes),
                Err(TransferError::ConcurrentUpdate)
            );
            let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(current[accounts][0][refresh], "synthetic-rotated");
            assert_eq!(
                current[accounts][0]["future_field"],
                store[accounts][0]["future_field"]
            );
            let identity = if provider == TransferProvider::OpenAi {
                "account_id"
            } else {
                "email"
            };
            let different = revised(&changed, identity, json!("different-identity"));
            let before = fs::read(&path).unwrap();
            assert_eq!(
                import_cloud_at(home.path(), provider, "selected", &different),
                Err(TransferError::AccountConflict)
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn cloud_identityless_return_requires_original_departure_and_no_intervening_update() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let home = tempfile::tempdir().unwrap();
            let identity = if provider == TransferProvider::OpenAi {
                "account_id"
            } else {
                "email"
            };
            let bytes = revised(&cloud_fixture(provider, "selected"), identity, Value::Null);
            import_cloud_at(home.path(), provider, "selected", &bytes).unwrap();
            let departure = export_cloud_account_at(home.path(), provider, None).unwrap();
            let (_, _, _, refresh) = cloud_keys(provider);
            let changed = revised(&bytes, refresh, json!("synthetic-rotated"));
            assert_eq!(
                import_cloud_at(home.path(), provider, "selected", &changed),
                Err(TransferError::AccountConflict)
            );
            import_cloud_return_at(home.path(), provider, "selected", &changed, &departure)
                .unwrap();
            let newer = revised(&bytes, refresh, json!("synthetic-newer"));
            let before = fs::read(home.path().join(provider.store_name())).unwrap();
            assert_eq!(
                import_cloud_return_at(home.path(), provider, "selected", &newer, &departure),
                Err(TransferError::ConcurrentUpdate)
            );
            assert_eq!(
                fs::read(home.path().join(provider.store_name())).unwrap(),
                before
            );
            let switched = revised(&newer, identity, json!("different-identity"));
            assert_eq!(
                import_cloud_return_at(home.path(), provider, "selected", &switched, &departure),
                Err(TransferError::AccountConflict)
            );
        }
    }

    #[test]
    fn cloud_validation_is_bounded_and_does_not_modify_existing_store() {
        let provider = TransferProvider::OpenAi;
        let home = tempfile::tempdir().unwrap();
        let valid = cloud_fixture(provider, "chosen");
        import_cloud_at(home.path(), provider, "chosen", &valid).unwrap();
        let before = fs::read(home.path().join(provider.store_name())).unwrap();
        assert_eq!(
            import_cloud_at(home.path(), provider, "other", &valid),
            Err(TransferError::LabelMismatch)
        );
        assert_eq!(
            import_cloud_at(home.path(), TransferProvider::Claude, "chosen", &valid),
            Err(TransferError::ProviderMismatch)
        );
        assert_eq!(
            import_cloud_at(
                home.path(),
                provider,
                "chosen",
                &vec![b' '; MAX_TRANSFER_BYTES + 1]
            ),
            Err(TransferError::TooLarge)
        );
        assert_eq!(
            import_cloud_at(
                home.path(),
                provider,
                "chosen",
                &revised(&valid, "refresh_token", json!(" "))
            ),
            Err(TransferError::InvalidPayload)
        );
        assert_eq!(
            import_cloud_at(
                home.path(),
                provider,
                "chosen",
                &revised(&valid, "label", json!("other"))
            ),
            Err(TransferError::LabelMismatch)
        );
        let mut version: Value = serde_json::from_slice(&valid).unwrap();
        version["version"] = json!(1);
        assert_eq!(
            import_cloud_at(
                home.path(),
                provider,
                "chosen",
                &serde_json::to_vec(&version).unwrap()
            ),
            Err(TransferError::UnsupportedVersion)
        );
        assert_eq!(
            fs::read(home.path().join(provider.store_name())).unwrap(),
            before
        );
    }

    #[test]
    fn cloud_rejects_unsafe_files_and_ancestors_without_touching_targets() {
        let provider = TransferProvider::Claude;
        let bytes = cloud_fixture(provider, "chosen");
        let target = tempfile::tempdir().unwrap();
        import_cloud_at(target.path(), provider, "chosen", &bytes).unwrap();
        let before = fs::read(target.path().join(provider.store_name())).unwrap();
        fs::set_permissions(
            target.path().join(provider.store_name()),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(
            import_cloud_at(target.path(), provider, "chosen", &bytes),
            Err(TransferError::UnsafePath)
        );
        fs::set_permissions(
            target.path().join(provider.store_name()),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        for hardlink in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let path = home.path().join(provider.store_name());
            if hardlink {
                fs::hard_link(target.path().join(provider.store_name()), &path).unwrap();
            } else {
                symlink(target.path().join(provider.store_name()), &path).unwrap();
            }
            assert!(import_cloud_at(home.path(), provider, "chosen", &bytes).is_err());
            assert!(export_cloud_account_at(home.path(), provider, None).is_err());
        }
        let parent = tempfile::tempdir().unwrap();
        symlink(target.path(), parent.path().join("linked")).unwrap();
        assert!(
            import_cloud_at(&parent.path().join("linked"), provider, "chosen", &bytes).is_err()
        );
        assert!(export_cloud_account_at(&parent.path().join("linked"), provider, None).is_err());
        assert_eq!(
            fs::read(target.path().join(provider.store_name())).unwrap(),
            before
        );
        let untouched = tempfile::tempdir().unwrap();
        fs::write(untouched.path().join("sentinel"), b"unchanged").unwrap();
        symlink(untouched.path(), parent.path().join("fresh-link")).unwrap();
        assert!(
            import_cloud_at(
                &parent.path().join("fresh-link"),
                provider,
                "chosen",
                &bytes
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(untouched.path()).unwrap().count(), 1);
        assert_eq!(
            fs::read(untouched.path().join("sentinel")).unwrap(),
            b"unchanged"
        );
    }

    #[test]
    fn cloud_concurrent_imports_do_not_lose_unrelated_accounts() {
        for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
            let home = tempfile::tempdir().unwrap();
            let barrier = Arc::new(Barrier::new(8));
            let threads: Vec<_> = (0..8)
                .map(|i| {
                    let home = home.path().to_owned();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        let label = format!("synthetic-{i}");
                        barrier.wait();
                        import_cloud_at(&home, provider, &label, &cloud_fixture(provider, &label))
                            .unwrap();
                    })
                })
                .collect();
            for thread in threads {
                thread.join().unwrap();
            }
            let store: Value =
                serde_json::from_slice(&fs::read(home.path().join(provider.store_name())).unwrap())
                    .unwrap();
            assert_eq!(store[cloud_keys(provider).0].as_array().unwrap().len(), 8);
        }
    }

    #[test]
    fn cloud_rejects_unlabeled_legacy_and_duplicate_labels() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("auth.json");
        let legacy = br#"{"anthropic":{"access":"synthetic","refresh":"synthetic","expires":1}}"#;
        fs::write(&path, legacy).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            export_cloud_account_at(home.path(), TransferProvider::Claude, None),
            Err(TransferError::Unavailable)
        ));
        assert_eq!(
            import_cloud_at(
                home.path(),
                TransferProvider::Claude,
                "chosen",
                &cloud_fixture(TransferProvider::Claude, "chosen")
            ),
            Err(TransferError::InvalidStore)
        );
        assert_eq!(fs::read(&path).unwrap(), legacy);
        let mut duplicated: Value =
            serde_json::from_slice(&source(home.path(), TransferProvider::Claude)).unwrap();
        duplicated["anthropic_accounts"][1]["label"] = json!("first");
        fs::write(&path, serde_json::to_vec(&duplicated).unwrap()).unwrap();
        assert!(matches!(
            export_cloud_account_at(home.path(), TransferProvider::Claude, Some("first")),
            Err(TransferError::InvalidStore)
        ));
    }

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
