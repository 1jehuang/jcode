use super::*;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Hash, PartialEq, Eq)]
struct Scope {
    program: String,
    args: Vec<String>,
    endpoint: Option<std::ffi::OsString>,
    cwd: PathBuf,
}

#[derive(Default)]
pub(super) struct SharedCatalog {
    pub models: Arc<RwLock<Vec<String>>>,
    pub images: Arc<RwLock<bool>>,
    pub snapshot: RwLock<ModelCatalog>,
    updated: RwLock<Option<Instant>>,
}

#[derive(Clone)]
pub(super) struct Binding {
    pub cwd: PathBuf,
    pub shared: Arc<SharedCatalog>,
}

pub(super) fn shared_catalog(
    command: &CursorAcpCommand,
    cwd: &Path,
    endpoint: Option<std::ffi::OsString>,
) -> Arc<SharedCatalog> {
    static SCOPES: OnceLock<std::sync::Mutex<HashMap<Scope, Arc<SharedCatalog>>>> = OnceLock::new();
    let key = Scope {
        program: command.program.clone(),
        args: command.args.clone(),
        endpoint,
        cwd: cwd.into(),
    };
    let mut scopes = SCOPES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    scopes.entry(key).or_default().clone()
}

#[derive(Clone)]
pub(super) struct PublishedState {
    catalog: Arc<RwLock<ModelCatalog>>,
    model: Arc<RwLock<Option<String>>>,
    shared: Arc<SharedCatalog>,
    binding: Arc<RwLock<Binding>>,
}

impl PublishedState {
    pub fn publish(&self, catalog: &ModelCatalog, images: bool) {
        if catalog.models.is_empty() {
            return;
        }
        *self
            .shared
            .snapshot
            .write()
            .unwrap_or_else(|e| e.into_inner()) = catalog.clone();
        *self
            .shared
            .models
            .write()
            .unwrap_or_else(|e| e.into_inner()) = catalog.models.clone();
        *self
            .shared
            .images
            .write()
            .unwrap_or_else(|e| e.into_inner()) = images;
        *self
            .shared
            .updated
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        let binding = self.binding.read().unwrap_or_else(|e| e.into_inner());
        if Arc::ptr_eq(&binding.shared, &self.shared) {
            *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog.clone();
            *self.model.write().unwrap_or_else(|e| e.into_inner()) = catalog.current.clone();
        }
    }
}

impl CursorAcpProvider {
    pub(super) fn shared(&self) -> Arc<SharedCatalog> {
        self.binding
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .shared
            .clone()
    }

    pub(super) fn bind_workspace(&self, cwd: &Path) {
        let mut binding = self.binding.write().unwrap_or_else(|e| e.into_inner());
        if binding.cwd == cwd {
            return;
        }
        binding.cwd = cwd.into();
        binding.shared = shared_catalog(&self.command, cwd, self.endpoint.clone());
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = binding
            .shared
            .snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .unconfirmed();
        *self.model.write().unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .requested_effort
            .write()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub(super) fn with_state(&self, process: &AcpProcess) {
        self.published_for(&process.cwd)
            .publish(&process.catalog, process.supports_images);
    }

    #[cfg(test)]
    pub(super) fn published(&self) -> PublishedState {
        self.published_for(&self.binding.read().unwrap_or_else(|e| e.into_inner()).cwd)
    }

    fn published_for(&self, cwd: &Path) -> PublishedState {
        PublishedState {
            catalog: self.catalog.clone(),
            model: self.model.clone(),
            shared: shared_catalog(&self.command, cwd, self.endpoint.clone()),
            binding: self.binding.clone(),
        }
    }

    pub(super) async fn ensure_process<'a>(
        &'a self,
        guard: &'a mut Option<AcpProcess>,
    ) -> Result<&'a mut AcpProcess> {
        let cwd = self
            .binding
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .cwd
            .clone();
        let usable = match guard.as_mut() {
            Some(process) => process.cwd == cwd && process.child.try_wait()?.is_none(),
            None => false,
        };
        if !usable {
            let replacement = self.replacement(&cwd).await?;
            if let Some(mut previous) = guard.replace(replacement) {
                previous.shutdown().await;
            }
        }
        guard
            .as_mut()
            .ok_or_else(|| anyhow!("Cursor ACP process was not initialized"))
    }

    async fn replacement(&self, cwd: &Path) -> Result<AcpProcess> {
        let mut process = AcpProcess::spawn(
            &self.command,
            cwd,
            self.endpoint.as_deref(),
            self.permission_mode_resolver.clone(),
        )
        .await?;
        if let Err(error) = self.configure_process(&mut process).await {
            process.shutdown().await;
            return Err(error);
        }
        process.published = Some(self.published_for(cwd));
        Ok(process)
    }

    pub(super) async fn configure_process(&self, process: &mut AcpProcess) -> Result<String> {
        let (prior_model, prior_effort) = {
            let catalog = self.catalog.read().unwrap_or_else(|e| e.into_inner());
            (
                catalog.current.clone(),
                catalog.effort.as_ref().and_then(|s| s.current.clone()),
            )
        };
        let requested = self
            .requested_model
            .write()
            .map_err(|_| anyhow!("Cursor ACP pending model state is poisoned"))?
            .take();
        let effort = self
            .requested_effort
            .write()
            .map_err(|_| anyhow!("Cursor ACP pending effort state is poisoned"))?
            .take();
        let confirmed = self
            .model
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|id| process.catalog.models.contains(id));
        let selected = resolve_model(
            requested.as_deref().or(confirmed.as_deref()),
            &process.catalog.models,
            process.catalog.current.as_deref(),
        )?;
        if process.catalog.models.is_empty() {
            bail!("Cursor ACP did not advertise available models");
        }
        if process.catalog.current.as_deref() != Some(&selected) {
            process.set_model(&selected).await?;
        }
        let effort = effort.or_else(|| {
            let current = process.catalog.current.as_deref().unwrap_or(&selected);
            let prior_effort = prior_effort?;
            (prior_model.as_deref() == Some(current)
                && process
                    .catalog
                    .effort
                    .as_ref()?
                    .choices
                    .contains(&prior_effort))
            .then_some(prior_effort)
        });
        if let Some(effort) = effort {
            let selector = process
                .catalog
                .effort
                .clone()
                .ok_or_else(|| anyhow!("Cursor ACP no longer advertises an effort selector"))?;
            if !selector.choices.contains(&effort) {
                bail!("Cursor ACP does not advertise effort '{effort}' for the selected model");
            }
            if selector.current.as_deref() != Some(&effort) {
                process.set_config(&selector.id, &effort).await?;
            }
        }
        self.with_state(process);
        Ok(process.catalog.current.clone().unwrap_or(selected))
    }

    pub(super) async fn discover(&self, force: bool) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        let mut guard = self.session.lock().await;
        let binding = self
            .binding
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if !force {
            if let Some(process) = guard.as_mut()
                && process.cwd == binding.cwd
                && process.child.try_wait()?.is_none()
                && !process.catalog.models.is_empty()
            {
                return Ok(());
            }
            if binding
                .shared
                .updated
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|t| t.elapsed() < Duration::from_secs(300))
            {
                return Ok(());
            }
        }
        let replacement = self.replacement(&binding.cwd).await?;
        if let Some(mut previous) = guard.replace(replacement) {
            previous.shutdown().await;
        }
        Ok(())
    }

    pub(super) fn effort_choices(&self) -> Vec<String> {
        self.catalog
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .effort_choices(&self.model())
    }
}
