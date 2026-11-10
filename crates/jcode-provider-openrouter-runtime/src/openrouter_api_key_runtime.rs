use super::*;

impl OpenRouterProvider {
    pub fn new_openrouter_api_key_runtime() -> Result<Self> {
        let auth: AuthResolver = Arc::new(|| {
            let api_key = load_api_key_from_env_or_config(DEFAULT_API_KEY_NAME, DEFAULT_ENV_FILE)
                .ok_or_else(|| {
                let path = jcode_base::storage::app_config_dir()
                    .map(|dir| dir.join(DEFAULT_ENV_FILE).display().to_string())
                    .unwrap_or_else(|_| DEFAULT_ENV_FILE.to_string());
                anyhow::anyhow!(
                    "{} not found in environment or {}",
                    DEFAULT_API_KEY_NAME,
                    path
                )
            })?;
            Ok(ProviderAuth::AuthorizationBearer {
                token: api_key,
                label: DEFAULT_API_KEY_NAME.to_string(),
            })
        });
        auth()?;

        Ok(Self {
            client: jcode_provider_core::shared_http_client(),
            model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
            reasoning_effort: Arc::new(RwLock::new(None)),
            api_base: DEFAULT_API_BASE.to_string(),
            auth,
            session_account: Self::initial_session_account(None),
            supports_provider_features: true,
            supports_model_catalog: true,
            profile_id: None,
            builtin_azure: false,
            reasoning_effort_support: None,
            disable_reasoning_heuristics: false,
            static_reasoning_config: HashMap::new(),
            max_tokens: Self::configured_max_tokens(None),
            extra_body: Self::resolve_extra_body(None, DEFAULT_ENV_FILE),
            static_models: Vec::new(),
            static_context_limits: HashMap::new(),
            static_image_input_support: HashMap::new(),
            send_openrouter_headers: true,
            conversation_id: new_conversation_id(),
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            model_catalog_refresh: Arc::new(Mutex::new(ModelCatalogRefreshState::default())),
            provider_routing: Arc::new(RwLock::new(Self::parse_provider_routing())),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::new(RwLock::new(HashMap::new())),
            endpoint_refresh: Arc::new(Mutex::new(EndpointRefreshTracker::default())),
        })
    }
}
