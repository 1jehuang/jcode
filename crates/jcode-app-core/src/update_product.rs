//! KCode has no maintained release channel compatible with this branch.
//! Disable both automatic and explicit self-updates, independently of semver,
//! environment overrides, or whether this executable is renamed.

pub(super) const SELF_UPDATE_ENABLED: bool = false;

pub(super) fn require_update_channel() -> anyhow::Result<()> {
    if !SELF_UPDATE_ENABLED {
        anyhow::bail!(
            "KCode self-update is disabled: install a verified KCode build from \
             KooshaPari/KCode; jcode releases and install paths are never used"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kcode_self_update_is_disabled_even_without_a_fork_version_suffix() {
        assert!(!SELF_UPDATE_ENABLED);
        assert!(
            require_update_channel()
                .unwrap_err()
                .to_string()
                .contains("KCode self-update is disabled")
        );
    }
}
