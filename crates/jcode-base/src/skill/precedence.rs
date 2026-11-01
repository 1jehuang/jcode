//! Skill name precedence.
//!
//! Within one scope jcode-native directories beat compatibility directories:
//! project `.jcode/skills` > `.agents/skills` > `.claude/skills`, and global
//! `~/.jcode/skills` > `~/.agents/skills` > Claude plugin store. Loaders run
//! lowest priority first so the last insert wins; this module records the
//! shadowing so users learn which definition is active.

use super::Skill;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Insert `skill`, replacing any lower-priority skill with the same name and
/// warning once per (name, winner, loser) triple. Overlays reload on every
/// session access, so the dedupe set keeps the log quiet.
pub(super) fn insert_skill(skills: &mut HashMap<String, Skill>, skill: Skill) {
    let name = skill.name.clone();
    let winner = skill.path.clone();
    if let Some(shadowed) = skills.insert(name.clone(), skill)
        && shadowed.path != winner
    {
        warn_shadowed(&name, &winner, &shadowed.path);
    }
}

fn warn_shadowed(name: &str, winner: &Path, loser: &Path) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let key = format!("{name}\0{}\0{}", winner.display(), loser.display());
    let first = SEEN
        .get_or_init(Default::default)
        .lock()
        .map(|mut seen| seen.insert(key))
        .unwrap_or(false);
    if first {
        crate::logging::warn(&format!(
            "Skill '{name}' from {} shadows {}",
            winner.display(),
            loser.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::SkillRegistry;
    use std::path::Path;

    fn write_skill(root: &Path, dir: &str, name: &str, description: &str) {
        let skill_dir = root.join(dir).join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
        )
        .expect("write skill");
    }

    fn project_description(root: &Path, name: &str) -> String {
        let registry = SkillRegistry::load_project_overlay(Some(root)).expect("overlay");
        registry
            .get(name)
            .expect("skill present")
            .description
            .clone()
    }

    #[test]
    fn project_jcode_skill_wins_over_claude_and_agents() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), ".claude", "dup", "claude");
        write_skill(temp.path(), ".agents", "dup", "agents");
        write_skill(temp.path(), ".jcode", "dup", "jcode");
        assert_eq!(project_description(temp.path(), "dup"), "jcode");
    }

    #[test]
    fn project_agents_skill_wins_over_claude() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), ".claude", "dup", "claude");
        write_skill(temp.path(), ".agents", "dup", "agents");
        assert_eq!(project_description(temp.path(), "dup"), "agents");
    }

    #[test]
    fn single_directory_is_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), ".claude", "only", "claude");
        assert_eq!(project_description(temp.path(), "only"), "claude");
    }

    #[test]
    fn global_jcode_skill_wins_over_global_agents_including_after_reload() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        write_skill(temp.path(), "", "dup", "jcode");
        write_skill(&temp.path().join("external"), ".agents", "dup", "agents");
        let mut registry = SkillRegistry::load_global().expect("global");
        assert_eq!(registry.get("dup").expect("present").description, "jcode");
        registry.reload_global().expect("reload");
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        assert_eq!(registry.get("dup").expect("present").description, "jcode");
    }

    #[test]
    fn project_still_beats_global_after_merge() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), ".claude", "dup", "project");
        let mut base = SkillRegistry::default();
        let global = tempfile::tempdir().expect("tempdir");
        write_skill(global.path(), ".jcode", "dup", "global");
        base.load_from_dir(&global.path().join(".jcode/skills"))
            .expect("load");
        let effective = SkillRegistry::effective_for_working_dir(&base, Some(temp.path()));
        assert_eq!(
            effective.get("dup").expect("present").description,
            "project"
        );
    }
}
