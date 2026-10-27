//! Offline registry fixtures shared by skill and profile activation tests.
use std::path::{Path, PathBuf};

pub(crate) struct Fixture {
    pub temp: tempfile::TempDir,
    pub project: PathBuf,
    pub other: PathBuf,
    pub manager: crate::plugin::PluginManager,
    previous_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}
impl Fixture {
    pub fn new() -> Self {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        let other = temp.path().join("other");
        let market = temp.path().join("market");
        let source = market.join("registry-fixture");
        for dir in [
            &project,
            &other,
            &market.join(".claude-plugin"),
            &source.join(".claude-plugin"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write(
            &market.join(".claude-plugin/marketplace.json"),
            r#"{"name":"registry-test","plugins":[{"name":"registry-fixture","source":"./registry-fixture"}]}"#,
        );
        write(
            &source.join(".claude-plugin/plugin.json"),
            r#"{"name":"registry-fixture","version":"1.0.0","skills":["./extra-skills"],"commands":["./commands","./extra-commands"],"agents":["./agents","./extra-agents"]}"#,
        );
        write(
            &source.join("skills/default/SKILL.md"),
            "---\nname: default\ndescription: Default skill\n---\nDefault body",
        );
        write(
            &source.join("extra-skills/custom/SKILL.md"),
            "---\nname: custom\ndescription: Custom skill\n---\nCustom body",
        );
        write(
            &source.join("commands/plain.md"),
            "Run $ARGUMENTS in ${CLAUDE_PLUGIN_ROOT}",
        );
        write(
            &source.join("extra-commands/nested/check.md"),
            "---\ndescription: Check command\n---\nCheck $ARGUMENTS",
        );
        write(
            &source.join("agents/reviewer.md"),
            "---\nname: ignored-name\ndescription: Review\nmodel: ignored/model\n---\nReview carefully.",
        );
        write(
            &source.join("extra-agents/custom.md"),
            "Custom review body.",
        );
        let manager = crate::plugin::PluginManager::new(home.clone(), project.clone());
        manager.add_marketplace(market.to_str().unwrap()).unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", &home);
        Self {
            temp,
            project,
            other,
            manager,
            previous_home,
            _guard: guard,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        match self.previous_home.take() {
            Some(home) => crate::env::set_var("JCODE_HOME", home),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}
pub(crate) fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}
