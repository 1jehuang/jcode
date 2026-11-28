use super::*;

#[derive(Default)]
struct WorkspaceProvider(std::sync::Mutex<(String, std::path::PathBuf)>);

#[async_trait::async_trait]
impl Provider for WorkspaceProvider {
    fn name(&self) -> &str {
        "workspace-recorder"
    }
    fn set_model(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn set_session_id(&self, id: &str) {
        self.0.lock().unwrap().0 = id.into();
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self(std::sync::Mutex::new(self.0.lock().unwrap().clone())))
    }

    fn set_working_dir(&self, root: &std::path::Path) {
        self.0.lock().unwrap().1 = root.to_owned();
    }
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[tokio::test]
async fn workspace_binding_tracks_new_restore_pending_and_clear() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let provider = Arc::new(WorkspaceProvider::default());
    let mut agent = Agent::new(provider.clone(), Registry::empty());
    assert_eq!(provider.0.lock().unwrap().0, agent.session.id);
    agent.set_working_dir("/synthetic/original");
    let saved_id = agent.session.id.clone();
    agent.session.save_prepared().unwrap();
    agent.set_working_dir_for_pending_context(Some("/synthetic/pending".into()));
    assert_eq!(
        provider.0.lock().unwrap().1,
        std::path::Path::new("/synthetic/pending")
    );
    agent
        .restore_session_with_working_dir(&saved_id, Some("/synthetic/restored"))
        .unwrap();
    assert_eq!(
        provider.0.lock().unwrap().1,
        std::path::Path::new("/synthetic/restored")
    );
    agent.clear();
    assert_ne!(agent.session.id, saved_id);
    let context = provider.0.lock().unwrap();
    assert_eq!(context.0, agent.session.id);
    assert_eq!(context.1, std::path::Path::new("/synthetic/restored"));
}
