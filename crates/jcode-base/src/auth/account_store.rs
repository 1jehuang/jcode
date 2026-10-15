use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

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

/// Stored default account: the persisted active label if it still exists,
/// else the first account. Ignores the process-local runtime override, which
/// is reserved for one-shot CLI processes.
pub fn default_account_label<T, F>(
    stored_active_label: Option<&str>,
    accounts: &[T],
    label_of: F,
) -> Option<String>
where
    F: Fn(&T) -> &str + Copy,
{
    stored_active_label
        .and_then(|label| {
            accounts
                .iter()
                .find(|account| label_of(account) == label)
                .map(|account| label_of(account).to_string())
        })
        .or_else(|| {
            accounts
                .first()
                .map(|account| label_of(account).to_string())
        })
}

/// Stable identity for a stored account: email, else the provider account id,
/// else a short sha256 prefix of the refresh token.
pub fn account_identity(
    email: Option<&str>,
    account_id: Option<&str>,
    refresh_token: &str,
) -> Option<String> {
    let non_empty = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    non_empty(email)
        .map(|email| email.to_ascii_lowercase())
        .or_else(|| non_empty(account_id).map(|id| format!("id:{id}")))
        .or_else(|| {
            let refresh = refresh_token.trim();
            (!refresh.is_empty()).then(|| {
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(refresh.as_bytes());
                format!("rt:{}", &hex::encode(digest)[..16])
            })
        })
}

/// Resolve a pin to the current label of the account it names.
///
/// Identity wins: labels are positional and shift when an earlier account is
/// removed. The label is used only when the pin has no identity, or when the
/// account at that label has no identity to compare against. A pin whose
/// identity no longer matches any account resolves to `None`.
pub fn resolve_pin<T, FLabel, FIdentity>(
    pin: &crate::auth::AccountPin,
    accounts: &[T],
    label_of: FLabel,
    identity_of: FIdentity,
) -> Option<String>
where
    FLabel: Fn(&T) -> &str + Copy,
    FIdentity: Fn(&T) -> Option<String> + Copy,
{
    if let Some(identity) = pin.identity.as_deref() {
        if let Some(account) = accounts
            .iter()
            .find(|account| identity_of(account).as_deref() == Some(identity))
        {
            return Some(label_of(account).to_string());
        }
        // An account at the pinned label without a stable identity cannot be
        // told apart, so accept it. Refresh-token fingerprints (`rt:`) rotate
        // with every refresh, so they are weak too. An account with a
        // different email or account id is another subscription.
        let weak = |identity: Option<&str>| identity.is_none_or(|id| id.starts_with("rt:"));
        if !weak(Some(identity)) {
            return accounts
                .iter()
                .find(|account| label_of(account) == pin.label && identity_of(account).is_none())
                .map(|account| label_of(account).to_string());
        }
        return accounts
            .iter()
            .find(|account| label_of(account) == pin.label && weak(identity_of(account).as_deref()))
            .map(|account| label_of(account).to_string());
    }
    accounts
        .iter()
        .find(|account| label_of(account) == pin.label)
        .map(|account| label_of(account).to_string())
}

/// Build a pin for the stored account at `label`, capturing its identity.
pub fn pin_for_label<T, FLabel, FIdentity>(
    label: &str,
    accounts: &[T],
    label_of: FLabel,
    identity_of: FIdentity,
    missing_message: &str,
) -> Result<crate::auth::AccountPin>
where
    FLabel: Fn(&T) -> &str + Copy,
    FIdentity: Fn(&T) -> Option<String> + Copy,
{
    let account = accounts
        .iter()
        .find(|account| label_of(account) == label)
        .ok_or_else(|| anyhow::anyhow!(missing_message.replace("{}", label)))?;
    Ok(crate::auth::AccountPin::new(label, identity_of(account)))
}

/// A pinned account that no longer exists (removed or re-logged into another
/// subscription). Callers fall back to the default account and tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedAccountMissing(pub String);

impl std::fmt::Display for PinnedAccountMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pinned account '{}' is no longer available", self.0)
    }
}

impl std::error::Error for PinnedAccountMissing {}

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
    fn account_identity_prefers_email_then_id_then_refresh_hash() {
        assert_eq!(
            account_identity(Some("A@B.com"), Some("acct"), "rt").as_deref(),
            Some("a@b.com")
        );
        assert_eq!(
            account_identity(None, Some("acct"), "rt").as_deref(),
            Some("id:acct")
        );
        let hashed = account_identity(Some(" "), None, "refresh").unwrap();
        assert!(hashed.starts_with("rt:") && hashed.len() == 19, "{hashed}");
        assert_eq!(account_identity(None, None, ""), None);
    }

    #[test]
    fn resolve_pin_uses_identity_and_rejects_other_subscription_at_label() {
        struct Acc(&'static str, Option<&'static str>);
        fn label(a: &Acc) -> &str {
            a.0
        }
        fn id(a: &Acc) -> Option<String> {
            a.1.map(String::from)
        }
        let accounts = vec![
            Acc("claude-otter", Some("fox@x")),
            Acc("claude-fox", Some("panda@x")),
            Acc("claude-panda", None),
        ];
        let pin = |l: &str, i: Option<&str>| crate::auth::AccountPin::new(l, i.map(String::from));

        // Identity moved to another label after relabel.
        assert_eq!(
            resolve_pin(&pin("claude-fox", Some("fox@x")), &accounts, label, id).as_deref(),
            Some("claude-otter")
        );
        // Identity gone and label now belongs to someone else.
        assert_eq!(
            resolve_pin(&pin("claude-fox", Some("gone@x")), &accounts, label, id),
            None
        );
        // Label holder has no identity: accepted.
        assert_eq!(
            resolve_pin(&pin("claude-panda", Some("gone@x")), &accounts, label, id).as_deref(),
            Some("claude-panda")
        );
        // Label-only pins resolve by label.
        assert_eq!(
            resolve_pin(&pin("claude-fox", None), &accounts, label, id).as_deref(),
            Some("claude-fox")
        );
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
