use std::path::{Path, PathBuf};

pub(super) struct Fixture {
    _temp: tempfile::TempDir,
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
        for path in [&project, &other] {
            std::fs::create_dir_all(path).unwrap();
        }
        write(
            &market.join(".claude-plugin/marketplace.json"),
            r#"{"name":"registry-test","plugins":[{"name":"registry-fixture","source":"./registry-fixture"}]}"#,
        );
        write(
            &market.join("registry-fixture/.claude-plugin/plugin.json"),
            r#"{"name":"registry-fixture"}"#,
        );
        let manager = crate::plugin::PluginManager::new(home.clone(), project.clone());
        manager.add_marketplace(market.to_str().unwrap()).unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", &home);
        Self {
            _temp: temp,
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
pub(super) fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}
