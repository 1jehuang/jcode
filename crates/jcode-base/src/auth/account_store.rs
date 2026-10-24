use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};

#[derive(Default)]
struct LockOwner {
    thread: Option<std::thread::ThreadId>,
    depth: usize,
    file: Option<std::fs::File>,
}

#[derive(Default)]
struct StoreLock {
    owner: Mutex<LockOwner>,
    available: Condvar,
}

static STORE_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<StoreLock>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Synchronous transaction guard. Deliberately !Send: release on the acquiring
/// thread, and never hold this guard across an await. Sidecars must never be
/// removed, since replacing the inode would split the cross-process lock.
pub(crate) struct StoreLockGuard {
    lock: Arc<StoreLock>,
    _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Drop for StoreLockGuard {
    fn drop(&mut self) {
        let mut owner = self.lock.owner.lock().unwrap_or_else(|e| e.into_inner());
        owner.depth -= 1;
        if owner.depth == 0 {
            // Closing the last descriptor releases flock, including on unwind.
            owner.file = None;
            owner.thread = None;
            self.lock.available.notify_one();
        }
    }
}

/// Lock one store in the jcode home directory for a complete read/modify/write
/// transaction. Nested acquisitions on the same thread are supported, including
/// load-time migration calling save. Unix also serializes independent processes.
/// Other platforms retain process-local serialization (no cross-process flock).
pub(crate) fn lock_store(home: &Path, store_name: &str) -> Result<StoreLockGuard> {
    anyhow::ensure!(
        !store_name.is_empty()
            && store_name != "."
            && store_name != ".."
            && store_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'),
        "Unsafe account store name"
    );
    std::fs::create_dir_all(home)?;
    let metadata = std::fs::symlink_metadata(home)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Unsafe account store directory"
    );
    let home = std::fs::canonicalize(home)?;
    let path = home.join(format!(".{store_name}.lock"));
    let lock = STORE_LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(path.clone())
        .or_default()
        .clone();
    let thread = std::thread::current().id();
    let mut owner = lock.owner.lock().unwrap_or_else(|e| e.into_inner());
    while owner.thread.is_some() && owner.thread != Some(thread) {
        owner = lock
            .available
            .wait(owner)
            .unwrap_or_else(|e| e.into_inner());
    }
    if owner.thread.is_none() {
        owner.file = Some(open_store_lock(&home, &path)?);
        owner.thread = Some(thread);
    }
    owner.depth += 1;
    drop(owner);
    Ok(StoreLockGuard {
        lock,
        _not_send: std::marker::PhantomData,
    })
}

#[cfg(unix)]
fn open_store_lock(home: &Path, path: &Path) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(home)?;
    let metadata = directory.metadata()?;
    anyhow::ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
        "Unsafe account store directory ownership or permissions"
    );
    let name = std::ffi::CString::new(path.file_name().unwrap().as_bytes())?;
    // Open relative to a pinned directory and never follow the sidecar symlink.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.nlink() == 1
            && metadata.mode() & 0o777 == 0o600,
        "Unsafe account store lock ownership, type, links or permissions"
    );
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_store_lock(_home: &Path, path: &Path) -> Result<std::fs::File> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Unsafe account store lock path"
        );
    }
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}

/// Runtime (process-local) active-account overrides, keyed by provider
/// prefix ("claude", "openai", ...). Lets `/account switch <label>` take
/// effect immediately without rewriting the provider auth file.
///
/// Centralized here so every provider shares one mechanism instead of
/// duplicating a `static ACTIVE_ACCOUNT_OVERRIDE` per module.
static RUNTIME_ACTIVE_OVERRIDES: LazyLock<RwLock<HashMap<&'static str, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn set_runtime_active_override(prefix: &'static str, label: Option<String>) {
    if let Ok(mut overrides) = RUNTIME_ACTIVE_OVERRIDES.write() {
        match label {
            Some(label) => {
                overrides.insert(prefix, label);
            }
            None => {
                overrides.remove(prefix);
            }
        }
    }
}

pub fn runtime_active_override(prefix: &str) -> Option<String> {
    RUNTIME_ACTIVE_OVERRIDES
        .read()
        .ok()
        .and_then(|overrides| overrides.get(prefix).cloned())
}

/// Memorable, provider-independent account names. Keeping this list fixed makes
/// labels stable across restarts and gives the same ordinal account the same
/// animal for every provider (for example `claude-otter` and `openai-otter`).
const ACCOUNT_ANIMALS: &[&str] = &[
    "otter", "fox", "panda", "wolf", "owl", "lynx", "badger", "raven", "tiger", "koala", "falcon",
    "gecko", "bison", "heron", "moose", "orca", "rabbit", "yak", "zebra", "beaver", "cougar",
    "dolphin", "ibis", "jaguar", "lemur", "marten", "newt", "quail", "seal", "wombat", "alpaca",
    "penguin",
];

pub fn canonical_account_label(prefix: &str, index: usize) -> String {
    let animal = index
        .checked_sub(1)
        .and_then(|index| ACCOUNT_ANIMALS.get(index).copied());
    match animal {
        Some(animal) => format!("{prefix}-{animal}"),
        // Extremely large account sets remain unique without making the common
        // case less friendly.
        None => format!("{prefix}-animal-{index}"),
    }
}

fn legacy_account_ordinal(prefix: &str, label: &str) -> Option<usize> {
    // Primary-account sentinels from before numbered/animal labels.
    if matches!(label, "default" | "main") {
        return Some(1);
    }
    let suffix = label.strip_prefix(prefix)?.strip_prefix('-')?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    parse_account_ordinal(suffix).filter(|index| *index > 0)
}

fn account_ordinal(prefix: &str, label: &str) -> Option<usize> {
    let suffix = label.strip_prefix(prefix)?.strip_prefix('-')?;
    ACCOUNT_ANIMALS
        .iter()
        .position(|animal| *animal == suffix)
        .map(|index| index + 1)
        .or_else(|| parse_account_ordinal(suffix.strip_prefix("animal-")?))
        .or_else(|| legacy_account_ordinal(prefix, label))
}

// User-chosen labels and out-of-range suffixes are not numbered accounts.
fn parse_account_ordinal(suffix: &str) -> Option<usize> {
    match suffix.parse() {
        Ok(index) => Some(index),
        Err(error) => {
            crate::logging::debug(&format!(
                "Account label suffix is not a representable ordinal: {error}"
            ));
            None
        }
    }
}

pub fn next_account_label<'a>(prefix: &str, labels: impl IntoIterator<Item = &'a str>) -> String {
    let labels = labels.into_iter().collect::<HashSet<_>>();
    let highest = labels
        .iter()
        .filter_map(|label| account_ordinal(prefix, label))
        .max()
        .unwrap_or(0);
    let mut index = highest.checked_add(1).unwrap_or(1);
    loop {
        let candidate = canonical_account_label(prefix, index);
        if !labels.contains(candidate.as_str()) {
            return candidate;
        }
        index = index.checked_add(1).unwrap_or(1);
    }
}

pub fn login_target_label<T, F>(
    prefix: &str,
    requested: Option<&str>,
    active_label: Option<String>,
    accounts: &[T],
    label_of: F,
) -> String
where
    F: Fn(&T) -> &str + Copy,
{
    if let Some(requested) = requested
        .map(str::trim)
        .filter(|requested| !requested.is_empty())
    {
        if accounts
            .iter()
            .any(|account| label_of(account) == requested)
        {
            return requested.to_string();
        }
        return next_account_label(prefix, accounts.iter().map(label_of));
    }

    active_label
        .or_else(|| {
            accounts
                .first()
                .map(|account| label_of(account).to_string())
        })
        .unwrap_or_else(|| canonical_account_label(prefix, 1))
}

pub fn active_account_label<T, F>(
    override_label: Option<String>,
    stored_active_label: Option<String>,
    accounts: &[T],
    label_of: F,
) -> Option<String>
where
    F: Fn(&T) -> &str + Copy,
{
    override_label.or(stored_active_label).or_else(|| {
        accounts
            .first()
            .map(|account| label_of(account).to_string())
    })
}

pub fn set_active_account<T, F>(
    label: &str,
    accounts: &[T],
    stored_active_label: &mut Option<String>,
    missing_message: &str,
    label_of: F,
) -> Result<()>
where
    F: Fn(&T) -> &str + Copy,
{
    if !accounts.iter().any(|account| label_of(account) == label) {
        anyhow::bail!(missing_message.replace("{}", label));
    }
    *stored_active_label = Some(label.to_string());
    Ok(())
}

pub fn upsert_account<T, FGet, FSet>(
    prefix: &str,
    accounts: &mut Vec<T>,
    stored_active_label: &mut Option<String>,
    account: T,
    label_of: FGet,
    set_label: FSet,
) -> String
where
    FGet: Fn(&T) -> &str + Copy,
    FSet: Fn(&mut T, String) + Copy,
{
    let requested_label = label_of(&account).to_string();
    if let Some(existing) = accounts
        .iter_mut()
        .find(|existing| label_of(existing) == requested_label)
    {
        *existing = account;
        return requested_label;
    }

    let label = next_account_label(prefix, accounts.iter().map(label_of));
    let mut account = account;
    set_label(&mut account, label.clone());
    accounts.push(account);

    if stored_active_label.is_none() || accounts.len() == 1 {
        *stored_active_label = Some(label.clone());
    }

    label
}

pub struct RelabelOutcome {
    pub changed: bool,
    pub canonical_override_label: Option<String>,
}

pub fn relabel_accounts<T, FGet, FSet>(
    prefix: &str,
    accounts: &mut [T],
    stored_active_label: &mut Option<String>,
    override_label: Option<String>,
    label_of: FGet,
    set_label: FSet,
) -> RelabelOutcome
where
    FGet: Fn(&T) -> &str + Copy,
    FSet: Fn(&mut T, String) + Copy,
{
    // Account labels are identities, not positions. Never renumber surviving
    // canonical (or user-named) accounts when an earlier account is removed.
    // Reserve every existing identity before migrating numeric legacy labels
    // and the old primary-account sentinels (default/main).
    let mut used = accounts
        .iter()
        .map(|account| label_of(account).to_string())
        .collect::<HashSet<_>>();
    let label_map = accounts
        .iter()
        .map(|account| {
            let original = label_of(account).to_string();
            let canonical = match legacy_account_ordinal(prefix, &original) {
                Some(index) => {
                    let candidate = canonical_account_label(prefix, index);
                    if used.contains(&candidate) {
                        next_account_label(prefix, used.iter().map(String::as_str))
                    } else {
                        candidate
                    }
                }
                None => original.clone(),
            };
            used.insert(canonical.clone());
            (original, canonical)
        })
        .collect::<Vec<_>>();
    let mut changed = false;

    for (account, (_, canonical_label)) in accounts.iter_mut().zip(label_map.iter()) {
        if label_of(account) != canonical_label {
            set_label(account, canonical_label.clone());
            changed = true;
        }
    }

    let desired_active = if accounts.is_empty() {
        None
    } else {
        stored_active_label
            .as_deref()
            .and_then(|label| {
                label_map
                    .iter()
                    .find(|(original, _)| original == label)
                    .map(|(_, canonical)| canonical.clone())
            })
            .or_else(|| {
                accounts
                    .first()
                    .map(|account| label_of(account).to_string())
            })
    };

    if *stored_active_label != desired_active {
        *stored_active_label = desired_active;
        changed = true;
    }

    let canonical_override_label = override_label.and_then(|override_label| {
        label_map
            .iter()
            .find(|(original, _)| original == &override_label)
            .and_then(|(_, canonical)| (override_label != *canonical).then(|| canonical.clone()))
    });

    RelabelOutcome {
        changed,
        canonical_override_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn increment_store(home: &Path) {
        for _ in 0..40 {
            let outer = lock_store(home, "counter.json").unwrap();
            let inner = lock_store(home, "counter.json").unwrap();
            // Out-of-order destruction must also retain the process lock.
            drop(outer);
            let path = home.join("counter.json");
            let value: usize = std::fs::read_to_string(&path)
                .ok()
                .map(|s| s.parse().unwrap())
                .unwrap_or(0);
            std::thread::yield_now();
            std::fs::write(path, (value + 1).to_string()).unwrap();
            drop(inner);
        }
    }

    #[test]
    fn store_lock_subprocess_worker() {
        if let Some(home) = std::env::var_os("JCODE_ACCOUNT_LOCK_TEST_HOME") {
            increment_store(Path::new(&home));
        }
    }

    #[test]
    fn store_lock_serializes_threads_and_nested_transactions() {
        let home = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let home = home.path();
                scope.spawn(move || increment_store(home));
            }
        });
        assert_eq!(
            std::fs::read_to_string(home.path().join("counter.json")).unwrap(),
            "320"
        );
    }

    #[cfg(unix)]
    #[test]
    fn store_lock_serializes_process_transactions() {
        let home = tempfile::tempdir().unwrap();
        let mut children = (0..4)
            .map(|_| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "auth::account_store::tests::store_lock_subprocess_worker",
                        "--nocapture",
                    ])
                    .env("JCODE_ACCOUNT_LOCK_TEST_HOME", home.path())
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        increment_store(home.path());
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(
            std::fs::read_to_string(home.path().join("counter.json")).unwrap(),
            "200"
        );
    }

    #[test]
    fn store_lock_provider_transactions_preserve_concurrent_accounts() {
        use crate::auth::{claude, codex};
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let home = sandbox.root();
                scope.spawn(move || {
                    for index in 0..4 {
                        let identity = format!("worker-{worker}-{index}");
                        {
                            // Simulate a cloud transaction sharing the same lock.
                            let _guard = lock_store(home, "openai-auth.json").unwrap();
                            let account = serde_json::from_value(serde_json::json!({
                                "label": identity, "access_token": "old", "refresh_token": "refresh"
                            })).unwrap();
                            let label = codex::upsert_account(account).unwrap();
                            codex::update_account_tokens(&label, "new", "refresh", None, None, None).unwrap();
                            codex::update_account_profile(&label, Some(identity.clone())).unwrap();
                        }
                        {
                            let _guard = lock_store(home, "auth.json").unwrap();
                            let account = serde_json::from_value(serde_json::json!({
                                "label": identity, "access": "old", "refresh": "refresh", "expires": 0
                            })).unwrap();
                            let label = claude::upsert_account(account).unwrap();
                            claude::update_account_tokens(&label, "new", "refresh", 42).unwrap();
                            claude::update_account_profile(&label, Some(identity.clone())).unwrap();
                        }
                    }
                });
            }
        });
        let openai = codex::list_accounts().unwrap();
        let claude = claude::list_accounts().unwrap();
        assert_eq!(openai.len(), 32);
        assert_eq!(claude.len(), 32);
        assert_eq!(
            openai
                .iter()
                .filter_map(|a| a.email.as_ref())
                .collect::<HashSet<_>>()
                .len(),
            32
        );
        assert_eq!(
            claude
                .iter()
                .filter_map(|a| a.email.as_ref())
                .collect::<HashSet<_>>()
                .len(),
            32
        );
        assert!(openai.iter().all(|a| a.access_token == "new"));
        assert!(claude.iter().all(|a| a.access == "new" && a.expires == 42));
    }

    #[test]
    fn store_lock_rejects_invalid_names() {
        let home = tempfile::tempdir().unwrap();
        for name in [
            "",
            ".",
            "..",
            "../auth.json",
            "x/y",
            "x\\y",
            "/auth",
            "x\0y",
        ] {
            assert!(lock_store(home.path(), name).is_err(), "{name:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn store_lock_rejects_unsafe_paths() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("target");
        std::fs::write(&target, "unchanged").unwrap();
        let path = home.path().join(".auth.json.lock");
        symlink(&target, &path).unwrap();
        assert!(lock_store(home.path(), "auth.json").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(lock_store(home.path(), "auth.json").is_err());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(lock_store(home.path(), "auth.json").is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&path, home.path().join("hardlink")).unwrap();
        assert!(lock_store(home.path(), "auth.json").is_err());
        let alias = home.path().join("alias");
        symlink(home.path(), &alias).unwrap();
        assert!(lock_store(&alias, "other.json").is_err());
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(lock_store(home.path(), "other.json").is_err());
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[derive(Clone)]
    struct Account {
        label: String,
    }

    #[test]
    fn relabel_accounts_canonicalizes_labels_and_active_label() {
        let mut accounts = vec![
            Account {
                label: "openai-1".to_string(),
            },
            Account {
                label: "openai-2".to_string(),
            },
        ];
        let mut active = Some("openai-2".to_string());

        let outcome = relabel_accounts(
            "openai",
            &mut accounts,
            &mut active,
            Some("openai-1".to_string()),
            |account| account.label.as_str(),
            |account, label| account.label = label,
        );

        assert!(outcome.changed);
        assert_eq!(accounts[0].label, "openai-otter");
        assert_eq!(accounts[1].label, "openai-fox");
        assert_eq!(active.as_deref(), Some("openai-fox"));
        assert_eq!(
            outcome.canonical_override_label.as_deref(),
            Some("openai-otter")
        );
    }

    #[test]
    fn upsert_account_assigns_next_label_and_sets_initial_active() {
        let mut accounts = Vec::<Account>::new();
        let mut active = None;

        let label = upsert_account(
            "claude",
            &mut accounts,
            &mut active,
            Account {
                label: "ignored".to_string(),
            },
            |account| account.label.as_str(),
            |account, label| account.label = label,
        );

        assert_eq!(label, "claude-otter");
        assert_eq!(accounts[0].label, "claude-otter");
        assert_eq!(active.as_deref(), Some("claude-otter"));
    }

    #[test]
    fn account_labels_use_animals_and_stay_unique_after_the_named_pool() {
        assert_eq!(canonical_account_label("claude", 1), "claude-otter");
        assert_eq!(canonical_account_label("claude", 2), "claude-fox");
        assert_eq!(canonical_account_label("openai", 32), "openai-penguin");
        assert_eq!(canonical_account_label("openai", 33), "openai-animal-33");
    }

    #[test]
    fn deleting_first_account_keeps_survivor_identity_and_allocates_unique_label() {
        for prefix in ["claude", "openai"] {
            let first = canonical_account_label(prefix, 1);
            let survivor = canonical_account_label(prefix, 2);
            let mut accounts = vec![
                Account { label: first },
                Account {
                    label: survivor.clone(),
                },
            ];
            accounts.remove(0);
            let mut active = Some(survivor.clone());
            let outcome = relabel_accounts(
                prefix,
                &mut accounts,
                &mut active,
                Some(survivor.clone()),
                |account| &account.label,
                |account, label| account.label = label,
            );
            assert!(!outcome.changed);
            assert!(outcome.canonical_override_label.is_none());
            assert_eq!(accounts[0].label, survivor);
            assert_eq!(active.as_deref(), Some(survivor.as_str()));
            let next = next_account_label(
                prefix,
                accounts.iter().map(|account| account.label.as_str()),
            );
            assert_eq!(next, canonical_account_label(prefix, 3));
            assert_eq!(
                login_target_label(prefix, Some("new"), active.clone(), &accounts, |account| {
                    &account.label
                }),
                next
            );
            let inserted = upsert_account(
                prefix,
                &mut accounts,
                &mut active,
                Account {
                    label: "new".into(),
                },
                |account| &account.label,
                |account, label| account.label = label,
            );
            assert_eq!(inserted, next);
            assert_eq!(accounts[0].label, survivor);
            assert_ne!(accounts[0].label, accounts[1].label);
        }
    }

    #[test]
    fn legacy_primary_aliases_migrate_without_renaming_named_accounts() {
        for alias in ["default", "main"] {
            let mut accounts = [alias, "work"].map(|label| Account {
                label: label.into(),
            });
            let mut active = Some(alias.into());
            let outcome = relabel_accounts(
                "openai",
                &mut accounts,
                &mut active,
                Some(alias.into()),
                |account| &account.label,
                |account, label| account.label = label,
            );
            assert!(outcome.changed);
            assert_eq!(accounts[0].label, "openai-otter");
            assert_eq!(accounts[1].label, "work");
            assert_eq!(active.as_deref(), Some("openai-otter"));
            assert_eq!(
                outcome.canonical_override_label.as_deref(),
                Some("openai-otter")
            );
        }
    }

    #[test]
    fn numeric_migration_never_steals_existing_canonical_or_named_identity() {
        let mut accounts =
            ["claude-1", "claude-otter", "work", "claude-animal-33"].map(|label| Account {
                label: label.into(),
            });
        let mut active = Some("claude-1".into());
        let outcome = relabel_accounts(
            "claude",
            &mut accounts,
            &mut active,
            Some("claude-1".into()),
            |account| &account.label,
            |account, label| account.label = label,
        );
        assert!(outcome.changed);
        assert_eq!(accounts[0].label, "claude-animal-34");
        assert_eq!(accounts[1].label, "claude-otter");
        assert_eq!(accounts[2].label, "work");
        assert_eq!(accounts[3].label, "claude-animal-33");
        assert_eq!(active.as_deref(), Some("claude-animal-34"));
        assert_eq!(
            outcome.canonical_override_label.as_deref(),
            Some("claude-animal-34")
        );
        assert_eq!(
            next_account_label(
                "claude",
                accounts.iter().map(|account| account.label.as_str())
            ),
            "claude-animal-35"
        );
    }
}
