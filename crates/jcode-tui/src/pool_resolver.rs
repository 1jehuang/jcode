use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    OAuth,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub tier: u8,
    pub kind: AccountKind,
    pub enrolled: bool,
}

fn eligible(account: &Account, provider: &str, unavailable: &HashSet<String>) -> bool {
    if account.provider != provider {
        return false;
    }
    if unavailable.contains(&account.id) {
        return false;
    }
    match account.kind {
        AccountKind::OAuth => true,
        AccountKind::ApiKey => account.enrolled,
    }
}

pub fn resolve_account<'a>(
    accounts: &'a [Account],
    provider: &str,
    unavailable: &HashSet<String>,
) -> Option<&'a Account> {
    let mut best_tier: Option<u8> = None;
    let mut best: Option<&'a Account> = None;

    for account in accounts {
        if !eligible(account, provider, unavailable) {
            continue;
        }
        match best_tier {
            None => {
                best_tier = Some(account.tier);
                best = Some(account);
            }
            Some(tier) if account.tier < tier => {
                best_tier = Some(account.tier);
                best = Some(account);
            }
            Some(_) => {}
        }
    }

    best
}

pub fn mark_unavailable(unavailable: &mut HashSet<String>, account_id: &str) {
    unavailable.insert(account_id.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn acct(id: &str, provider: &str, tier: u8, kind: AccountKind, enrolled: bool) -> Account {
        Account {
            id: id.to_string(),
            provider: provider.to_string(),
            tier,
            kind,
            enrolled,
        }
    }

    #[test]
    fn highest_tier_wins_over_a_lower_tier() {
        let accounts = vec![
            acct("low", "openai", 2, AccountKind::OAuth, false),
            acct("high", "openai", 1, AccountKind::OAuth, false),
        ];
        let unavailable = HashSet::new();
        let chosen = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(chosen.id, "high");
    }

    #[test]
    fn unavailable_tier1_falls_through_to_other_tier1_before_tier2() {
        let accounts = vec![
            acct("t1a", "openai", 1, AccountKind::OAuth, false),
            acct("t2a", "openai", 2, AccountKind::OAuth, false),
            acct("t1b", "openai", 1, AccountKind::OAuth, false),
        ];
        let mut unavailable = HashSet::new();
        unavailable.insert("t1a".to_string());
        let chosen = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(chosen.id, "t1b");
    }

    #[test]
    fn single_unavailable_with_no_tier1_alternative_drops_to_tier2() {
        let accounts = vec![
            acct("t1", "openai", 1, AccountKind::OAuth, false),
            acct("t2", "openai", 2, AccountKind::OAuth, false),
        ];
        let mut unavailable = HashSet::new();
        unavailable.insert("t1".to_string());
        let chosen = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(chosen.id, "t2");
    }

    #[test]
    fn non_enrolled_api_key_is_never_selected_even_when_only_account() {
        let accounts = vec![acct("k1", "openai", 1, AccountKind::ApiKey, false)];
        let unavailable = HashSet::new();
        assert!(resolve_account(&accounts, "openai", &unavailable).is_none());
    }

    #[test]
    fn enrolled_api_key_is_selected_when_best_eligible_option() {
        let accounts = vec![
            acct("k1", "openai", 1, AccountKind::ApiKey, true),
            acct("o2", "openai", 2, AccountKind::OAuth, false),
        ];
        let unavailable = HashSet::new();
        let chosen = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(chosen.id, "k1");
    }

    #[test]
    fn two_accounts_same_provider_both_reachable_in_order_multi_key_rotation() {
        let accounts = vec![
            acct("a", "openai", 1, AccountKind::OAuth, false),
            acct("b", "openai", 1, AccountKind::OAuth, false),
        ];
        let unavailable = HashSet::new();
        let first = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(first.id, "a");
    }

    #[test]
    fn mark_unavailable_then_resolve_returns_next_member_429_path() {
        let accounts = vec![
            acct("a", "openai", 1, AccountKind::OAuth, false),
            acct("b", "openai", 1, AccountKind::OAuth, false),
        ];
        let mut unavailable = HashSet::new();
        let first = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(first.id, "a");
        mark_unavailable(&mut unavailable, &first.id);
        let second = resolve_account(&accounts, "openai", &unavailable).unwrap();
        assert_eq!(second.id, "b");
    }

    #[test]
    fn resolve_account_returns_none_when_everything_unavailable() {
        let accounts = vec![
            acct("a", "openai", 1, AccountKind::OAuth, false),
            acct("b", "openai", 2, AccountKind::OAuth, false),
        ];
        let mut unavailable = HashSet::new();
        unavailable.insert("a".to_string());
        unavailable.insert("b".to_string());
        assert!(resolve_account(&accounts, "openai", &unavailable).is_none());
    }
}
