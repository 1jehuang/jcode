use super::*;

#[derive(Clone)]
struct CatalogProvider;

#[async_trait]
impl Provider for CatalogProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("unused")
    }
    fn name(&self) -> &str {
        "catalog-test"
    }
    fn model(&self) -> String {
        "future-model-2099".into()
    }
    fn available_models_display(&self) -> Vec<String> {
        vec![self.model()]
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[derive(Clone)]
struct CombinedCatalogProvider;

#[async_trait]
impl Provider for CombinedCatalogProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("unused")
    }
    fn name(&self) -> &str {
        "combined-test"
    }
    fn available_models_display(&self) -> Vec<String> {
        panic!("separate lookup")
    }
    fn model_routes(&self) -> Vec<ModelRoute> {
        panic!("separate lookup")
    }
    fn model_catalog_snapshot(&self) -> ModelCatalogSnapshot {
        ModelCatalogSnapshot::new(
            Some("combined".into()),
            Some("future-model-2100".into()),
            vec!["future-model-2100".into()],
            vec![],
        )
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn default_catalog_snapshot_preserves_legacy_fields() {
    let provider = CatalogProvider;
    let expected = ModelCatalogSnapshot::new(
        Some(provider.display_name()),
        Some(provider.model()),
        provider.available_models_display(),
        provider.model_routes(),
    );
    assert_eq!(ModelCatalogSnapshot::from_provider(&provider), expected);
}

#[test]
fn catalog_snapshot_dispatches_combined_hook_through_trait_object() {
    let provider: Arc<dyn Provider> = Arc::new(CombinedCatalogProvider);
    let snapshot = ModelCatalogSnapshot::from_provider(provider.as_ref());
    assert_eq!(snapshot.provider_name.as_deref(), Some("combined"));
    assert_eq!(snapshot.available_models, ["future-model-2100"]);
    let mut changed = snapshot;
    changed.available_models.clear();
    assert_eq!(
        ModelCatalogSnapshot::from_provider(provider.as_ref())
            .available_models
            .len(),
        1
    );
}
