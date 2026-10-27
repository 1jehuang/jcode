//! Per-skill visibility chosen in the `/skills` dialog, stored in
//! `~/.jcode/skill-overrides.json` as `{"skill-name": "off", ...}`.
//! Same four states and cycle order as Claude Code's `skillOverrides`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The skills listed for the model: `disable-model-invocation` skills and
/// ones set to user-only or off in `/skills` are left out, name-only ones
/// lose their description. Shared by every prompt builder so they agree.
pub fn model_skill_infos(skills: &crate::skill::SkillRegistry) -> Vec<crate::prompt::SkillInfo> {
    let overrides = load();
    skills
        .list()
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .filter_map(|skill| {
            let mode = mode(&overrides, &skill.name);
            mode.model_can_invoke().then(|| crate::prompt::SkillInfo {
                name: skill.name.clone(),
                description: if mode == SkillMode::NameOnly {
                    String::new()
                } else {
                    skill.description.clone()
                },
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SkillMode {
    /// Listed for the model with its description; `/name` works.
    #[default]
    On,
    /// Listed for the model by name only, saving the description's tokens.
    NameOnly,
    /// Hidden from the model; only the user can run it with `/name`.
    UserInvocableOnly,
    /// Not available to the model or the user.
    Off,
}

impl SkillMode {
    pub const CYCLE: [SkillMode; 4] = [
        SkillMode::On,
        SkillMode::NameOnly,
        SkillMode::UserInvocableOnly,
        SkillMode::Off,
    ];

    pub fn next(self) -> Self {
        let index = Self::CYCLE
            .iter()
            .position(|mode| *mode == self)
            .unwrap_or(0);
        Self::CYCLE[(index + 1) % Self::CYCLE.len()]
    }

    pub fn model_can_invoke(self) -> bool {
        matches!(self, SkillMode::On | SkillMode::NameOnly)
    }
}

pub type SkillOverrides = BTreeMap<String, SkillMode>;

fn path() -> Option<std::path::PathBuf> {
    crate::storage::jcode_dir()
        .ok()
        .map(|dir| dir.join("skill-overrides.json"))
}

/// Missing or unreadable file means every skill is `On`.
pub fn load() -> SkillOverrides {
    path()
        .filter(|path| path.exists())
        .and_then(|path| crate::storage::read_json(&path).ok())
        .unwrap_or_default()
}

/// Persist overrides, dropping `On` entries since that is the default.
pub fn save(overrides: &SkillOverrides) -> anyhow::Result<()> {
    let path = path().ok_or_else(|| anyhow::anyhow!("jcode home directory is unavailable"))?;
    let stored: SkillOverrides = overrides
        .iter()
        .filter(|(_, mode)| **mode != SkillMode::On)
        .map(|(name, mode)| (name.clone(), *mode))
        .collect();
    crate::storage::write_json(&path, &stored)
}

pub fn mode(overrides: &SkillOverrides, name: &str) -> SkillMode {
    overrides.get(name).copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_matches_claude_code_order_and_wraps() {
        assert_eq!(SkillMode::On.next(), SkillMode::NameOnly);
        assert_eq!(SkillMode::NameOnly.next(), SkillMode::UserInvocableOnly);
        assert_eq!(SkillMode::UserInvocableOnly.next(), SkillMode::Off);
        assert_eq!(SkillMode::Off.next(), SkillMode::On);
    }

    #[test]
    fn save_drops_default_entries_and_round_trips() {
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::tempdir().expect("home");
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());

        let mut overrides = SkillOverrides::new();
        overrides.insert("a".into(), SkillMode::Off);
        overrides.insert("b".into(), SkillMode::On);
        overrides.insert("c".into(), SkillMode::NameOnly);
        save(&overrides).expect("save");
        let loaded = load();

        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        assert_eq!(mode(&loaded, "a"), SkillMode::Off);
        assert_eq!(mode(&loaded, "c"), SkillMode::NameOnly);
        assert!(!loaded.contains_key("b"));
        assert_eq!(mode(&loaded, "missing"), SkillMode::On);
    }
}
