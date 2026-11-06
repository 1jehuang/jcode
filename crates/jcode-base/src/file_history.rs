//! Durable, tool-scoped file checkpoints. Never snapshots or resets a worktree.
//!
//! Before-images and intended outcomes are durable before a tracked mutation.
//! Restores compare bytes, existence and permissions, and refuse external changes.
use anyhow::{Context, Result, ensure};
use jcode_session_types::RewindCheckpoint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[path = "file_history_conversation.rs"]
mod conversation;

const MAX_CHECKPOINTS: usize = 100;
static HISTORY_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_FINALIZE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileState {
    blob: Option<String>,
    mode: Option<u32>,
    #[serde(default)]
    metadata: Option<FileMetadata>,
}

impl FileState {
    /// Legacy snapshots constrain bytes and mode but know nothing about metadata.
    /// This is directional matching, not equality: a modern snapshot still
    /// requires its recorded metadata to be present and unchanged. A new file
    /// leaves mode unset because the kernel derives it (umask, default ACL).
    fn matches(&self, actual: &Self) -> bool {
        self.blob == actual.blob
            && self.mode.is_none_or(|mode| actual.mode == Some(mode))
            && self
                .metadata
                .as_ref()
                .is_none_or(|metadata| actual.metadata.as_ref() == Some(metadata))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileMetadata {
    #[cfg(unix)]
    uid: u32,
    #[cfg(unix)]
    gid: u32,
    #[cfg(unix)]
    xattrs: BTreeMap<String, Vec<u8>>,
    // macOS stores its access-control list outside the named xattrs that
    // `xattrs` captures, so it needs its own capture/restore via the acl(3)
    // APIs. Empty means the file had no extended ACL. Text form (`acl_to_text`).
    #[cfg(target_os = "macos")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    acl: Vec<u8>,
    // Self-relative Windows security descriptor (DACL). A file can carry a
    // stricter DACL than its parent; the atomic replace in `apply` must restore
    // it onto the staged sibling, which otherwise inherits the parent's.
    #[cfg(windows)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    security_descriptor: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    info: RewindCheckpoint,
    before: BTreeMap<PathBuf, FileState>,
    #[serde(default)]
    blocked: BTreeSet<PathBuf>,
    #[serde(default)]
    boundary_message_id: Option<String>,
    #[serde(default)]
    input_message_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ConversationBoundary {
    message_index: usize,
    boundary_message_id: Option<String>,
}

type PathLocks = Mutex<BTreeMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>;

/// Shared with the editing tools, including batch subcalls and local clients.
pub async fn lock(path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: OnceLock<PathLocks> = OnceLock::new();
    let mutex = {
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, mutex| Arc::strong_count(mutex) > 1);
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        locks.entry(key).or_default().clone()
    };
    mutex.lock_owned().await
}

pub async fn lock_all(
    paths: impl IntoIterator<Item = PathBuf>,
) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
    let mut paths: Vec<_> = paths
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect();
    paths.sort();
    paths.dedup();
    let mut guards = Vec::with_capacity(paths.len());
    for path in paths {
        guards.push(lock(&path).await);
    }
    guards
}

#[derive(Clone, Serialize, Deserialize)]
struct Change {
    path: PathBuf,
    before: FileState,
    after: FileState,
}

#[derive(Clone, Serialize, Deserialize)]
struct ConversationCommit {
    session_id: String,
    message_index: usize,
    boundary_message_id: Option<String>,
}

fn conversation_saved(commit: &ConversationCommit) -> bool {
    crate::session::Session::load(&commit.session_id).is_ok_and(|session| {
        session.messages.len() == commit.message_index
            && session.messages.last().map(|message| message.id.clone())
                == commit.boundary_message_id
            && session.provider_session_id.is_none()
            && session.compaction.is_none()
    })
}

#[derive(Default, Serialize, Deserialize)]
struct History {
    checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    conversation_boundaries: BTreeMap<String, ConversationBoundary>,
    expected: BTreeMap<PathBuf, FileState>,
    #[serde(default)]
    pending: Option<Change>,
    #[serde(default)]
    restoring: Vec<Change>,
    #[serde(default)]
    conversation_commit: Option<ConversationCommit>,
}

fn directory(session_id: &str) -> Result<PathBuf> {
    // Hash the ID rather than allowing a session identifier to escape the sidecar.
    let key = hex::encode(Sha256::digest(session_id.as_bytes()));
    Ok(crate::storage::jcode_dir()?
        .join("sessions/file-history")
        .join(key))
}

fn save(dir: &Path, history: &History) -> Result<()> {
    #[cfg(test)]
    if history.restoring.is_empty() && FAIL_NEXT_FINALIZE.with(|flag| flag.replace(false)) {
        anyhow::bail!("injected checkpoint finalization failure");
    }
    crate::storage::write_json_secret(&dir.join("history.json"), history)
}

/// Resolve the nearest existing parent to a physical identity. Parent aliases
/// such as macOS /var are harmless; the destination itself must be a regular
/// non-aliased file so restoration cannot overwrite a symlink target.
fn safe_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute.parent().context("File has no parent")?;
    let name = absolute.file_name().context("File has no name")?;
    fn physical_parent(path: &Path) -> Result<PathBuf> {
        match fs::canonicalize(path) {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = path.parent().context("Missing file parent")?;
                Ok(physical_parent(parent)?.join(path.file_name().context("Invalid parent")?))
            }
            Err(error) => Err(error.into()),
        }
    }
    let path = physical_parent(parent)?.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            ensure!(
                !meta.file_type().is_symlink(),
                "Cannot checkpoint symlink: {}",
                path.display()
            );
            ensure!(
                meta.is_file(),
                "Cannot checkpoint non-regular file: {}",
                path.display()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    meta.nlink() == 1,
                    "Cannot checkpoint hardlinked file: {}",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

fn aliased_target(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        if metadata.file_type().is_symlink() {
            return true;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.is_file() && metadata.nlink() > 1 {
                return true;
            }
        }
        false
    })
}

fn state(dir: &Path, path: &Path) -> Result<FileState> {
    ensure!(
        safe_path(path)? == path,
        "Tracked path identity changed: {}",
        path.display()
    );
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileState {
                blob: None,
                mode: None,
                metadata: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let bytes = fs::read(path).with_context(|| format!("Capturing {}", path.display()))?;
    let blob = store_blob(dir, &bytes)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let mode = Some(u32::from(metadata.permissions().readonly()));
    #[cfg(unix)]
    let metadata_state = {
        use std::os::unix::fs::MetadataExt;
        Some(FileMetadata {
            uid: metadata.uid(),
            gid: metadata.gid(),
            xattrs: read_xattrs(path)?,
            #[cfg(target_os = "macos")]
            acl: read_acl(path)?,
        })
    };
    #[cfg(windows)]
    let metadata_state = read_security_descriptor(path).map(|descriptor| FileMetadata {
        security_descriptor: descriptor,
    });
    #[cfg(not(any(unix, windows)))]
    let metadata_state = None;
    Ok(FileState {
        blob: Some(blob),
        mode,
        metadata: metadata_state,
    })
}

fn store_blob(dir: &Path, bytes: &[u8]) -> Result<String> {
    let hash = hex::encode(Sha256::digest(bytes));
    let path = dir.join("blobs").join(&hash);
    if !path.exists() {
        crate::storage::ensure_dir(path.parent().unwrap())?;
        crate::storage::write_bytes(&path, bytes)?;
        crate::storage::harden_secret_file_permissions(&path);
    }
    Ok(hash)
}

#[cfg(target_os = "linux")]
fn read_xattrs(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let size = unsafe { libc::llistxattr(c_path.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        let error = std::io::Error::last_os_error();
        if xattrs_unsupported(&error) {
            return Ok(BTreeMap::new());
        }
        return Err(error).context("Could not list file extended attributes");
    }
    let mut names = vec![0u8; size as usize];
    let got = unsafe { libc::llistxattr(c_path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if got < 0 {
        let error = std::io::Error::last_os_error();
        if xattrs_unsupported(&error) {
            return Ok(BTreeMap::new());
        }
        return Err(error).context("Could not read file extended attribute names");
    }
    ensure!(
        got as usize <= names.len(),
        "File extended attributes changed during capture"
    );
    names.truncate(got as usize);
    let mut attrs = BTreeMap::new();
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name_c = std::ffi::CString::new(name)?;
        let size =
            unsafe { libc::lgetxattr(c_path.as_ptr(), name_c.as_ptr(), std::ptr::null_mut(), 0) };
        ensure!(size >= 0, "Could not read extended attribute size");
        let mut value = vec![0u8; size as usize];
        let got = unsafe {
            libc::lgetxattr(
                c_path.as_ptr(),
                name_c.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        ensure!(got >= 0, "Could not read extended attribute");
        ensure!(
            got as usize <= value.len(),
            "File extended attribute changed during capture"
        );
        value.truncate(got as usize);
        attrs.insert(String::from_utf8(name.to_vec())?, value);
    }
    Ok(attrs)
}

#[cfg(target_os = "linux")]
fn xattrs_unsupported(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOTSUP)
}

#[cfg(target_os = "macos")]
fn read_xattrs(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let size = unsafe {
        libc::listxattr(
            c_path.as_ptr(),
            std::ptr::null_mut(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if size < 0 {
        return Err(std::io::Error::last_os_error())
            .context("Could not list file extended attributes");
    }
    let mut names = vec![0u8; size as usize];
    let got = unsafe {
        libc::listxattr(
            c_path.as_ptr(),
            names.as_mut_ptr().cast(),
            names.len(),
            libc::XATTR_NOFOLLOW,
        )
    };
    if got < 0 {
        return Err(std::io::Error::last_os_error())
            .context("Could not read file extended attribute names");
    }
    // A zero-sized call only queries the size. Refuse a concurrently grown list.
    ensure!(
        got as usize <= names.len(),
        "File extended attributes changed during capture"
    );
    names.truncate(got as usize);
    let mut attrs = BTreeMap::new();
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        // macOS generates provenance on new inodes, including our staging
        // files. It is platform bookkeeping, not restorable user metadata.
        if name == b"com.apple.provenance" {
            continue;
        }
        let name_c = std::ffi::CString::new(name)?;
        let size = unsafe {
            libc::getxattr(
                c_path.as_ptr(),
                name_c.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if size < 0 {
            return Err(std::io::Error::last_os_error())
                .context("Could not read extended attribute size");
        }
        let mut value = vec![0u8; size as usize];
        let got = unsafe {
            libc::getxattr(
                c_path.as_ptr(),
                name_c.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if got < 0 {
            return Err(std::io::Error::last_os_error())
                .context("Could not read extended attribute");
        }
        ensure!(
            got as usize <= value.len(),
            "File extended attribute changed during capture"
        );
        value.truncate(got as usize);
        attrs.insert(String::from_utf8(name.to_vec())?, value);
    }
    Ok(attrs)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn read_xattrs(_path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    Ok(BTreeMap::new())
}

#[cfg(target_os = "linux")]
fn write_xattrs(path: &Path, attrs: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // A staged inode can inherit a parent ACL or security label of its own.
    for name in read_xattrs(path)?
        .keys()
        .filter(|name| !attrs.contains_key(*name))
    {
        let name = std::ffi::CString::new(name.as_bytes())?;
        if unsafe { libc::lremovexattr(c_path.as_ptr(), name.as_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Could not remove inherited extended attribute");
        }
    }
    for (name, value) in attrs {
        let name = std::ffi::CString::new(name.as_bytes())?;
        let result = unsafe {
            libc::lsetxattr(
                c_path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            return Err(error).context("Could not restore extended attribute");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn write_xattrs(path: &Path, attrs: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    for (name, value) in attrs
        .iter()
        .filter(|(name, _)| name.as_str() != "com.apple.provenance")
    {
        let name = std::ffi::CString::new(name.as_bytes())?;
        let result = unsafe {
            libc::setxattr(
                c_path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Could not restore extended attribute");
        }
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn write_xattrs(_path: &Path, _attrs: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    Ok(())
}

// macOS ACL access via acl(3). `libc` does not expose these, so declare them.
#[cfg(target_os = "macos")]
const ACL_TYPE_EXTENDED: u32 = 0x0000_0100;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn acl_get_file(path: *const libc::c_char, acl_type: u32) -> *mut libc::c_void;
    fn acl_set_file(
        path: *const libc::c_char,
        acl_type: u32,
        acl: *mut libc::c_void,
    ) -> libc::c_int;
    fn acl_to_text(acl: *mut libc::c_void, len: *mut isize) -> *mut libc::c_char;
    fn acl_from_text(text: *const libc::c_char) -> *mut libc::c_void;
    fn acl_init(count: libc::c_int) -> *mut libc::c_void;
    fn acl_free(obj: *mut libc::c_void) -> libc::c_int;
}

/// Read a macOS extended ACL as its text form. Empty means no extended ACL
/// (the common case: `acl_get_file` returns null). Best effort, like xattrs.
#[cfg(target_os = "macos")]
fn read_acl(path: &Path) -> Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let acl = unsafe { acl_get_file(c_path.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        return Ok(Vec::new());
    }
    let mut len: isize = 0;
    let text = unsafe { acl_to_text(acl, &mut len) };
    let result = if text.is_null() {
        Err(std::io::Error::last_os_error()).context("Could not serialize ACL")
    } else {
        let bytes =
            unsafe { std::slice::from_raw_parts(text.cast::<u8>(), len.max(0) as usize) }.to_vec();
        unsafe { acl_free(text.cast::<libc::c_void>()) };
        Ok(bytes)
    };
    unsafe { acl_free(acl) };
    result
}

/// Restore a captured macOS ACL onto the staged file before it replaces the
/// original. An empty capture means the original had none, so clear any ACL
/// the sibling inherited rather than let the replacement widen access.
#[cfg(target_os = "macos")]
fn write_acl(path: &Path, acl_text: &[u8]) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    if acl_text.is_empty() {
        let empty = unsafe { acl_init(0) };
        if empty.is_null() {
            return Err(std::io::Error::last_os_error()).context("Could not build empty ACL");
        }
        let rc = unsafe { acl_set_file(c_path.as_ptr(), ACL_TYPE_EXTENDED, empty) };
        unsafe { acl_free(empty) };
        // Clearing an ACL that is already absent reports ENOENT; that is fine.
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ENOENT) {
                return Err(error).context("Could not clear inherited ACL");
            }
        }
        return Ok(());
    }
    let c_text = std::ffi::CString::new(acl_text)?;
    let acl = unsafe { acl_from_text(c_text.as_ptr()) };
    if acl.is_null() {
        return Err(std::io::Error::last_os_error()).context("Could not parse stored ACL");
    }
    let rc = unsafe { acl_set_file(c_path.as_ptr(), ACL_TYPE_EXTENDED, acl) };
    unsafe { acl_free(acl) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("Could not restore ACL");
    }
    Ok(())
}

/// Read a file's DACL as a self-relative security descriptor. Best effort: a
/// file whose DACL cannot be read is simply left unrecorded, so normal edits
/// keep working; the preservation guarantee applies to DACLs we captured.
#[cfg(windows)]
fn read_security_descriptor(path: &Path) -> Option<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, GetFileSecurityW};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // First call with a null buffer reports the required length.
    let mut needed = 0u32;
    unsafe {
        GetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = unsafe {
        GetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }
    buf.truncate(needed as usize);
    Some(buf)
}

/// Apply a captured DACL to the staged file before it replaces the original.
/// A failure here aborts the atomic replace, so the caller falls back to an
/// in-place write that keeps the original DACL rather than dropping it.
#[cfg(windows)]
fn write_security_descriptor(path: &Path, descriptor: &[u8]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, SetFileSecurityW};
    if descriptor.is_empty() {
        return Ok(());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SetFileSecurityW takes a mutable pointer but treats the descriptor as
    // read-only input; only its DACL is applied per the information flag.
    let ok = unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION,
            descriptor.as_ptr() as *mut std::ffi::c_void,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error())
            .context("Could not restore Windows security descriptor");
    }
    Ok(())
}

fn bytes(dir: &Path, state: &FileState) -> Result<Option<Vec<u8>>> {
    let Some(hash) = &state.blob else {
        return Ok(None);
    };
    let data =
        fs::read(dir.join("blobs").join(hash)).context("Missing checkpoint file contents")?;
    ensure!(
        hex::encode(Sha256::digest(&data)) == *hash,
        "Corrupt checkpoint file contents"
    );
    Ok(Some(data))
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn apply(dir: &Path, change: &Change, reverse: bool) -> Result<()> {
    let (expected, desired) = if reverse {
        (&change.after, &change.before)
    } else {
        (&change.before, &change.after)
    };
    let actual = state(dir, &change.path)?;
    ensure!(
        expected.matches(&actual),
        "File changed outside tracked tools: {}",
        change.path.display()
    );
    let Some(data) = bytes(dir, desired)? else {
        if expected.blob.is_some() {
            fs::remove_file(&change.path)?;
            sync_parent(&change.path)?;
        }
        return Ok(());
    };
    let parent = change.path.parent().context("File has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".jcode-rewind-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Without a recorded mode this is a new file: create it like an
            // ordinary write so umask, default ACLs and setgid apply natively.
            options.mode(if desired.mode.is_some() { 0o600 } else { 0o666 });
        }
        let mut file = options.open(&temporary)?;
        // Restore the original DACL onto the still-empty sibling before any
        // contents land: it was created with the parent's inherited access, so
        // writing first would briefly expose a private file. The open handle
        // keeps write access. A new file records no descriptor and keeps the
        // inherited access, like an ordinary write.
        #[cfg(windows)]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            write_security_descriptor(&temporary, &metadata.security_descriptor)?;
        }
        // macOS: an inheritable parent ACL reaches the sibling despite mode
        // 0600. Apply (or clear) the original ACL before any bytes land; it is
        // re-applied below, after owner and mode changes, as the final state.
        #[cfg(target_os = "macos")]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            write_acl(&temporary, &metadata.acl)?;
        }
        file.write_all(&data)?;
        #[cfg(unix)]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
            let current = file.metadata()?;
            if current.uid() != metadata.uid || current.gid() != metadata.gid {
                let c_path = std::ffi::CString::new(temporary.as_os_str().as_bytes())?;
                let result = unsafe { libc::chown(c_path.as_ptr(), metadata.uid, metadata.gid) };
                ensure!(result == 0, "Could not preserve file owner and group");
            }
        }
        #[cfg(unix)]
        if let Some(mode) = desired.mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        // chown/chmod can clear capabilities or alter ACL masks. Restore access
        // attributes last, and leave the original untouched if any cannot be set.
        #[cfg(unix)]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            write_xattrs(&temporary, &metadata.xattrs)?;
            // macOS ACLs live outside named xattrs; restore them separately so
            // a deny rule on the original cannot vanish across the replace.
            #[cfg(target_os = "macos")]
            write_acl(&temporary, &metadata.acl)?;
        }
        #[cfg(not(unix))]
        if let Some(mode) = desired.mode {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(mode != 0);
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        // Recheck after staging, rather than overwriting a newer external edit.
        ensure!(
            actual.matches(&state(dir, &change.path)?),
            "File changed during restore: {}",
            change.path.display()
        );
        fs::rename(&temporary, &change.path)?;
        sync_parent(&change.path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn load(dir: &Path) -> Result<History> {
    let path = dir.join("history.json");
    let mut history: History = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid file checkpoint history")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => History::default(),
        Err(error) => return Err(error.into()),
    };
    if let Some(pending) = history.pending.clone() {
        let actual = state(dir, &pending.path)?;
        let completed_new_file = pending.before.blob.is_none() && actual.blob == pending.after.blob;
        ensure!(
            pending.before.matches(&actual) || pending.after.matches(&actual) || completed_new_file,
            "Interrupted file mutation conflicts with external changes: {}",
            pending.path.display()
        );
        history.expected.insert(pending.path, actual);
        history.pending = None;
        save(dir, &history)?;
    }
    if !history.restoring.is_empty() {
        if history
            .conversation_commit
            .as_ref()
            .is_some_and(conversation_saved)
        {
            for change in &history.restoring {
                ensure!(
                    change.after.matches(&state(dir, &change.path)?),
                    "Committed rewind conflicts with external changes: {}",
                    change.path.display()
                );
                history
                    .expected
                    .insert(change.path.clone(), change.after.clone());
            }
            history.restoring.clear();
            if let Some(commit) = &history.conversation_commit {
                let boundary = commit.message_index;
                conversation::retain_before(&mut history, boundary);
            }
            history.conversation_commit = None;
            save(dir, &history)?;
            return Ok(history);
        }
        // Crash recovery rolls files back to the pre-restore state. Transcript
        // persistence is owned by Agent and must not observe half a restore.
        for change in history.restoring.iter().rev() {
            let actual = state(dir, &change.path)?;
            if change.before.matches(&actual) {
                continue;
            }
            ensure!(
                change.after.matches(&actual),
                "Interrupted restore conflicts with external changes: {}",
                change.path.display()
            );
            apply(dir, change, true)?;
        }
        history.restoring.clear();
        history.conversation_commit = None;
        save(dir, &history)?;
    }
    Ok(history)
}

fn note_external(history: &mut History, path: &Path, actual: &FileState) {
    if history
        .expected
        .get(path)
        .is_some_and(|expected| !expected.matches(actual))
    {
        for checkpoint in &mut history.checkpoints {
            checkpoint.blocked.insert(path.to_owned());
        }
    }
}

pub fn checkpoint(
    session_id: &str,
    message_index: usize,
    prompt: &str,
) -> Result<RewindCheckpoint> {
    checkpoint_with_anchor(session_id, message_index, prompt, None)
}

fn checkpoint_with_anchor(
    session_id: &str,
    message_index: usize,
    prompt: &str,
    boundary_message_id: Option<String>,
) -> Result<RewindCheckpoint> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let mut history = load(&dir)?;
    let mut before = BTreeMap::new();
    for path in history.expected.keys().cloned().collect::<Vec<_>>() {
        let actual = match state(&dir, &path) {
            Ok(actual) => actual,
            Err(error) => {
                for checkpoint in &mut history.checkpoints {
                    checkpoint.blocked.insert(path.clone());
                }
                history.expected.remove(&path);
                crate::logging::warn(&format!(
                    "File checkpoints disabled for {}: {error:#}",
                    path.display()
                ));
                continue;
            }
        };
        note_external(&mut history, &path, &actual);
        history.expected.insert(path.clone(), actual.clone());
        before.insert(path, actual);
    }
    let info = RewindCheckpoint {
        target_kind: jcode_session_types::RewindTargetKind::UserPrompt,
        id: uuid::Uuid::new_v4().to_string(),
        message_index,
        prompt: prompt.to_owned(),
        timestamp: Some(chrono::Utc::now().to_rfc3339()),
        can_restore_files: true,
        changed_file_count: 0,
    };
    history.checkpoints.push(Checkpoint {
        info: info.clone(),
        before,
        blocked: BTreeSet::new(),
        boundary_message_id,
        input_message_id: None,
    });
    let evicted = history.checkpoints.len() > MAX_CHECKPOINTS;
    if evicted {
        let checkpoint = history.checkpoints.remove(0);
        // Also migrate older sidecars that predate independent boundaries.
        if let Some(input_id) = checkpoint.input_message_id {
            history
                .conversation_boundaries
                .entry(input_id)
                .or_insert(ConversationBoundary {
                    message_index: checkpoint.info.message_index,
                    boundary_message_id: checkpoint.boundary_message_id,
                });
        }
    }
    save(&dir, &history)?;
    if evicted {
        collect_blobs_best_effort(&dir, &history);
    }
    Ok(info)
}

pub fn checkpoints(session_id: &str) -> Result<Vec<RewindCheckpoint>> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let history = load(&dir)?;
    Ok(history
        .checkpoints
        .iter()
        .map(|checkpoint| conversation::checkpoint_info(&history, checkpoint))
        .collect())
}

/// Paths are exposed for the caller to take the same locks as editing tools.
/// Restore still performs a complete fresh preflight under the history lock.
pub fn restore_paths(session_id: &str, checkpoint_id: &str) -> Result<Vec<PathBuf>> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let history = load(&directory(session_id)?)?;
    let checkpoint = history
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.info.id == checkpoint_id)
        .context("Checkpoint not found")?;
    Ok(checkpoint.before.keys().cloned().collect())
}

pub fn restore(session_id: &str, checkpoint_id: &str) -> Result<usize> {
    restore_with_commit(session_id, checkpoint_id, None, || Ok(()))
}

fn restore_with_commit(
    session_id: &str,
    checkpoint_id: &str,
    conversation_commit: Option<ConversationCommit>,
    commit: impl FnOnce() -> Result<()>,
) -> Result<usize> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let mut history = load(&dir)?;
    let checkpoint = history
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.info.id == checkpoint_id)
        .context("Checkpoint not found")?;
    ensure!(
        checkpoint.blocked.is_empty(),
        "Checkpoint conflicts with external changes: {}",
        checkpoint
            .blocked
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut changes = Vec::new();
    for (path, desired) in &checkpoint.before {
        let actual = state(&dir, path)?;
        if desired.matches(&actual) {
            continue;
        }
        ensure!(
            history
                .expected
                .get(path)
                .is_some_and(|expected| expected.matches(&actual)),
            "File changed outside tracked tools: {}",
            path.display()
        );
        // Validate every backup before touching any file.
        bytes(&dir, desired)?;
        let mut after = desired.clone();
        if after.blob.is_some() && after.metadata.is_none() {
            // Persist the live metadata in the transaction too, so recovery and
            // subsequent conflict checks know what the legacy restore retained.
            after.metadata = actual.metadata.clone();
        }
        changes.push(Change {
            path: path.clone(),
            before: actual,
            after,
        });
    }
    history.restoring = changes.clone();
    history.conversation_commit = conversation_commit;
    save(&dir, &history)?;
    for change in &changes {
        if let Err(error) = apply(&dir, change, false) {
            // load() performs conflict-checked rollback, retaining the transaction
            // on disk if an external writer prevents safe recovery.
            let rollback = load(&dir);
            return match rollback {
                Ok(_) => Err(error),
                Err(rollback) => {
                    Err(error.context(format!("Restore rollback requires attention: {rollback:#}")))
                }
            };
        }
    }
    if let Err(error) = commit() {
        if history
            .conversation_commit
            .as_ref()
            .is_some_and(conversation_saved)
        {
            // Completing the pending transcript marker can recover a transient
            // snapshot/journal write failure. Finalize the same durable commit.
            load(&dir)?;
            return Ok(changes.len());
        }
        if history.conversation_commit.as_ref().is_some_and(|commit| {
            crate::session::Session::pending_rewind_exists(&commit.session_id)
        }) {
            return Err(error
                .context("Rewind commit is durable but requires recovery before further changes"));
        }
        // A failed save must not make recovery infer a successful transcript
        // commit from an earlier, coincidentally identical boundary.
        history.conversation_commit = None;
        save(&dir, &history)?;
        let rollback = load(&dir);
        return match rollback {
            Ok(_) => Err(error),
            Err(rollback) => {
                Err(error.context(format!("Restore rollback requires attention: {rollback:#}")))
            }
        };
    }
    for change in &changes {
        history
            .expected
            .insert(change.path.clone(), change.after.clone());
    }
    if let Some(commit) = &history.conversation_commit {
        let boundary = commit.message_index;
        conversation::retain_before(&mut history, boundary);
    }
    history.restoring.clear();
    history.conversation_commit = None;
    save(&dir, &history)?;
    Ok(changes.len())
}

impl crate::session::Session {
    pub fn create_rewind_checkpoint(&mut self, prompt: &str) -> Result<RewindCheckpoint> {
        checkpoint_with_anchor(
            &self.id,
            self.messages.len(),
            prompt,
            self.messages.last().map(|message| message.id.clone()),
        )
    }

    pub fn rewind_checkpoints(&self) -> std::result::Result<Vec<RewindCheckpoint>, String> {
        conversation::checkpoints(self).map_err(|error| format!("{error:#}"))
    }

    pub fn bind_rewind_checkpoint(&mut self, checkpoint_id: &str, message_id: &str) -> Result<()> {
        let _guard = HISTORY_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let dir = directory(&self.id)?;
        let mut history = load(&dir)?;
        let checkpoint = history
            .checkpoints
            .iter_mut()
            .find(|checkpoint| checkpoint.info.id == checkpoint_id)
            .context("Checkpoint not found")?;
        ensure!(
            self.messages
                .iter()
                .skip(checkpoint.info.message_index)
                .any(|message| message.id == message_id
                    && crate::session::is_authentic_user_prompt(message)),
            "Checkpoint input is not an authentic user message"
        );
        checkpoint.input_message_id = Some(message_id.to_owned());
        let boundary = ConversationBoundary {
            message_index: checkpoint.info.message_index,
            boundary_message_id: checkpoint.boundary_message_id.clone(),
        };
        history
            .conversation_boundaries
            .insert(message_id.to_owned(), boundary);
        save(&dir, &history)
    }

    pub async fn rewind_checkpoint(
        &mut self,
        checkpoint_id: &str,
        mode: jcode_session_types::RewindMode,
    ) -> std::result::Result<jcode_session_types::RewindResult, String> {
        self.rewind_checkpoint_inner(checkpoint_id, mode)
            .await
            .map_err(|error| format!("{error:#}"))
    }

    async fn rewind_checkpoint_inner(
        &mut self,
        checkpoint_id: &str,
        mode: jcode_session_types::RewindMode,
    ) -> Result<jcode_session_types::RewindResult> {
        let target = conversation::resolve_checkpoint(self, checkpoint_id, mode)?;
        let boundary = target.info().message_index;
        let assistant_target = matches!(
            &target,
            conversation::ResolvedTarget::AssistantMessage { .. }
        );
        if mode.restores_conversation() {
            ensure!(
                boundary <= self.messages.len(),
                "Checkpoint belongs to a removed conversation branch"
            );
            let anchor = boundary
                .checked_sub(1)
                .and_then(|index| self.messages.get(index))
                .map(|message| message.id.clone());
            ensure!(
                anchor.as_deref() == target.boundary_message_id(),
                "Checkpoint belongs to a different conversation branch"
            );
            ensure!(
                self.migration_lease_block().is_none(),
                "Session is owned by another machine"
            );
        }
        if assistant_target && boundary == self.messages.len() {
            return Ok(target.into_result(mode, 0, 0));
        }
        let paths = if mode.restores_files() {
            restore_paths(&self.id, &target.info().id)?
        } else {
            Vec::new()
        };
        let _locks = lock_all(paths).await;
        let previous = self.clone();
        let removed_messages = if mode.restores_conversation() {
            self.messages.len() - boundary
        } else {
            0
        };
        let session_id = self.id.clone();
        let conversation_commit = mode.restores_conversation().then(|| ConversationCommit {
            session_id: session_id.clone(),
            message_index: boundary,
            boundary_message_id: target.boundary_message_id().map(str::to_owned),
        });
        let mut persist = || -> Result<()> {
            if mode.restores_conversation() {
                self.truncate_messages(boundary);
                self.persist_conversation_rewind()?;
            }
            Ok(())
        };
        let result = if mode.restores_files() {
            restore_with_commit(&session_id, &target.info().id, conversation_commit, persist)
        } else {
            persist().and_then(|()| {
                // Keep unreachable metadata for assistant transcript undo. IDs
                // and reachability hide it on the discarded branch; bounded
                // file snapshot eviction and conflict checks still apply.
                if assistant_target {
                    return Ok(0);
                }
                let _guard = HISTORY_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let dir = directory(&session_id)?;
                let mut history = load(&dir)?;
                conversation::retain_before(&mut history, boundary);
                save(&dir, &history)?;
                Ok(0)
            })
        };
        match result {
            Ok(restored_files) => Ok(target.into_result(mode, removed_messages, restored_files)),
            Err(error) => {
                // Once a durable transcript commit exists, reverting only the
                // live Session would split memory from disk. Leave the target
                // state installed and retain the recovery marker on failure.
                let committed = mode.restores_conversation()
                    && (self.has_pending_rewind()
                        || conversation_saved(&ConversationCommit {
                            session_id: self.id.clone(),
                            message_index: boundary,
                            boundary_message_id: target.boundary_message_id().map(str::to_owned),
                        }));
                if !committed {
                    *self = previous;
                }
                Err(error)
            }
        }
    }
}

pub(crate) fn rewind_boundary_saved(session: &crate::session::Session) -> bool {
    crate::session::Session::load(&session.id).is_ok_and(|saved| {
        saved.updated_at == session.updated_at
            && saved
                .messages
                .iter()
                .map(|message| &message.id)
                .eq(session.messages.iter().map(|message| &message.id))
            && saved.provider_session_id.is_none()
            && saved.compaction.is_none()
    })
}

fn reachable(session: &crate::session::Session, checkpoint: &Checkpoint) -> bool {
    let boundary = checkpoint.info.message_index;
    let anchor = boundary
        .checked_sub(1)
        .and_then(|index| session.messages.get(index))
        .map(|message| &message.id);
    boundary < session.messages.len()
        && anchor == checkpoint.boundary_message_id.as_ref()
        && checkpoint.input_message_id.as_ref().is_some_and(|id| {
            session.messages.iter().skip(boundary).any(|message| {
                &message.id == id && crate::session::is_authentic_user_prompt(message)
            })
        })
}

fn mutate(session_id: &str, path: &Path, content: Option<&[u8]>) -> Result<()> {
    let _guard = HISTORY_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = directory(session_id)?;
    let mut history = load(&dir)?;
    if history.checkpoints.is_empty() {
        return match content {
            Some(bytes) => fs::write(path, bytes).map_err(Into::into),
            None => fs::remove_file(path).map_err(Into::into),
        };
    }
    if aliased_target(path) {
        // Existing tools deliberately permit links. Preserve that behavior,
        // but never advertise an incomplete checkpoint as safely restorable.
        for checkpoint in &mut history.checkpoints {
            checkpoint.blocked.insert(path.to_owned());
        }
        save(&dir, &history)?;
        return match content {
            Some(bytes) => fs::write(path, bytes).map_err(Into::into),
            None => fs::remove_file(path).map_err(Into::into),
        };
    }
    let path = safe_path(path)?;
    let before = state(&dir, &path)?;
    // Atomic rename must not grant editing rights that an ordinary write-open
    // would deny (read-only mode, ACLs). This open never truncates or writes.
    if content.is_some() && before.blob.is_some() {
        OpenOptions::new().write(true).open(&path)?;
    }
    // New files leave mode and metadata unset: the kernel assigns them on
    // creation, and imposing ours would strip inherited default ACLs.
    let after = FileState {
        blob: content.map(|bytes| store_blob(&dir, bytes)).transpose()?,
        mode: content.and(before.mode),
        metadata: content.and(before.metadata.clone()),
    };
    note_external(&mut history, &path, &before);
    for checkpoint in &mut history.checkpoints {
        checkpoint
            .before
            .entry(path.clone())
            .or_insert_with(|| before.clone());
    }
    let change = Change {
        path: path.clone(),
        before,
        after,
    };
    history.pending = Some(change.clone());
    save(&dir, &history)?;
    // Prepared regular-file writes replace atomically: death during staging
    // leaves the original intact, never a torn in-place truncation.
    let result = apply(&dir, &change, false);
    // A writable existing file may live in a directory where creating the
    // rewind staging sibling is forbidden. Preserve the ordinary write
    // contract and make this path unavailable for future file restoration.
    let result = match (result, content) {
        (Err(error), Some(bytes))
            if error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied) =>
        {
            match fs::write(&path, bytes) {
                Ok(()) => {
                    for checkpoint in &mut history.checkpoints {
                        checkpoint.blocked.insert(path.clone());
                    }
                    Ok(())
                }
                Err(write_error) => Err(write_error.into()),
            }
        }
        (result, _) => result,
    };
    let actual = state(&dir, &path)?;
    if (result.is_ok() && !change.after.matches(&actual))
        || (!change.before.matches(&actual) && !change.after.matches(&actual))
    {
        for checkpoint in &mut history.checkpoints {
            checkpoint.blocked.insert(path.clone());
        }
    }
    history.expected.insert(path, actual);
    history.pending = None;
    save(&dir, &history)?;
    collect_blobs_best_effort(&dir, &history);
    result.map_err(Into::into)
}

fn collect_blobs_best_effort(dir: &Path, history: &History) {
    if let Err(error) = collect_blobs(dir, history) {
        crate::logging::warn(&format!("Checkpoint blob cleanup failed: {error:#}"));
    }
}

fn collect_blobs(dir: &Path, history: &History) -> Result<()> {
    let mut used = BTreeSet::new();
    for state in history.expected.values().chain(
        history
            .checkpoints
            .iter()
            .flat_map(|checkpoint| checkpoint.before.values()),
    ) {
        if let Some(blob) = &state.blob {
            used.insert(blob.clone());
        }
    }
    for change in history.pending.iter().chain(history.restoring.iter()) {
        used.extend(
            change
                .before
                .blob
                .iter()
                .chain(change.after.blob.iter())
                .cloned(),
        );
    }
    let entries = match fs::read_dir(dir.join("blobs")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !used.contains(&entry.file_name().to_string_lossy().into_owned())
            && entry.file_type()?.is_file()
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub async fn tracked_write(session_id: &str, path: &Path, content: &[u8]) -> Result<()> {
    let (session_id, path, content) = (session_id.to_owned(), path.to_owned(), content.to_owned());
    tokio::task::spawn_blocking(move || mutate(&session_id, &path, Some(&content))).await?
}

pub async fn tracked_remove(session_id: &str, path: &Path) -> Result<()> {
    let (session_id, path) = (session_id.to_owned(), path.to_owned());
    tokio::task::spawn_blocking(move || mutate(&session_id, &path, None)).await?
}

#[cfg(test)]
#[path = "file_history_tests.rs"]
mod tests;
