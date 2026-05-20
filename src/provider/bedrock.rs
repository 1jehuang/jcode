use super::{
    DEFAULT_CONTEXT_LIMIT, EventStream, ModelCatalogRefreshSummary, ModelRoute, Provider,
    RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource, summarize_model_catalog_refresh,
};
use crate::message::{
    ContentBlock as JContentBlock, Message as JMessage, Role as JRole, StreamEvent, ToolDefinition,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_bedrock::Client as BedrockControlClient;
use aws_sdk_bedrockruntime::Client as BedrockRuntimeClient;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ContentBlockDelta, ContentBlockStart, ConversationRole, ConverseStreamOutput,
    ImageBlock, ImageFormat, ImageSource, InferenceConfiguration, Message,
    ReasoningContentBlockDelta, SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema,
    ToolSpecification,
};
use aws_smithy_types::Blob;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "anthropic.claude-3-5-sonnet-20241022-v2:0";
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 4096;
pub const ENV_FILE: &str = "bedrock.env";
pub const API_KEY_ENV: &str = "AWS_BEARER_TOKEN_BEDROCK";
pub const REGION_ENV: &str = "JCODE_BEDROCK_REGION";

#[derive(Debug, Clone)]
struct BedrockModelInfo {
    context_tokens: usize,
    max_output_tokens: usize,
    supports_tools: bool,
    supports_vision: bool,
    supports_reasoning: bool,
    pricing: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedCatalog {
    models: Vec<String>,
    inference_profiles: Vec<String>,
    #[serde(default)]
    profile_required_models: Vec<String>,
    #[serde(default)]
    inference_profile_routes: HashMap<String, String>,
    #[serde(default)]
    legacy_models: Vec<String>,
    region: Option<String>,
    fetched_at_rfc3339: String,
}

pub struct BedrockProvider {
    model: Arc<RwLock<String>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
    fetched_inference_profiles: Arc<RwLock<Vec<String>>>,
    profile_required_models: Arc<RwLock<HashSet<String>>>,
    inference_profile_routes: Arc<RwLock<HashMap<String, String>>>,
    legacy_models: Arc<RwLock<HashSet<String>>>,
}

impl BedrockProvider {
    pub fn new() -> Self {
        let model =
            std::env::var("JCODE_BEDROCK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let provider = Self {
            model: Arc::new(RwLock::new(model)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
            fetched_inference_profiles: Arc::new(RwLock::new(Vec::new())),
            profile_required_models: Arc::new(RwLock::new(HashSet::new())),
            inference_profile_routes: Arc::new(RwLock::new(HashMap::new())),
            legacy_models: Arc::new(RwLock::new(HashSet::new())),
        };
        provider.seed_cached_catalog();
        provider
    }

    pub fn has_credentials() -> bool {
        let explicitly_enabled = std::env::var("JCODE_BEDROCK_ENABLE")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if explicitly_enabled {
            return true;
        }

        let has_region = Self::configured_region().is_some();
        let has_credential_hint = Self::configured_bearer_token().is_some()
            || std::env::var_os("AWS_ACCESS_KEY_ID").is_some()
            || std::env::var_os("AWS_PROFILE").is_some()
            || std::env::var_os("JCODE_BEDROCK_PROFILE").is_some()
            || std::env::var_os("AWS_WEB_IDENTITY_TOKEN_FILE").is_some()
            || std::env::var_os("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").is_some()
            || std::env::var_os("AWS_CONTAINER_CREDENTIALS_FULL_URI").is_some()
            || std::env::var_os("AWS_SHARED_CREDENTIALS_FILE").is_some()
            || std::env::var_os("AWS_CONFIG_FILE").is_some();

        has_region && has_credential_hint
    }

    async fn sdk_config() -> aws_types::SdkConfig {
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(token) = Self::configured_bearer_token() {
            crate::env::set_var(API_KEY_ENV, token);
        }
        if let Some(region) = Self::configured_region() {
            loader = loader.region(aws_types::region::Region::new(region));
        }
        if let Ok(profile) =
            std::env::var("JCODE_BEDROCK_PROFILE").or_else(|_| std::env::var("AWS_PROFILE"))
        {
            if let Some(credentials) = Self::credentials_from_aws_login_profile(&profile).await {
                loader = loader.credentials_provider(credentials);
            }
            loader = loader.profile_name(profile);
        }
        loader.load().await
    }

    async fn credentials_from_aws_login_profile(profile: &str) -> Option<Credentials> {
        if std::env::var_os("AWS_ACCESS_KEY_ID").is_some()
            || std::env::var_os("AWS_SECRET_ACCESS_KEY").is_some()
            || std::env::var_os("AWS_BEARER_TOKEN_BEDROCK").is_some()
        {
            return None;
        }

        let output = tokio::process::Command::new("aws")
            .args([
                "configure",
                "export-credentials",
                "--profile",
                profile,
                "--format",
                "env-no-export",
            ])
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8(output.stdout).ok()?;
        let mut access_key_id = None;
        let mut secret_access_key = None;
        let mut session_token = None;
        for line in stdout.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "AWS_ACCESS_KEY_ID" => access_key_id = Some(value.trim().to_string()),
                "AWS_SECRET_ACCESS_KEY" => secret_access_key = Some(value.trim().to_string()),
                "AWS_SESSION_TOKEN" => session_token = Some(value.trim().to_string()),
                _ => {}
            }
        }

        Some(Credentials::new(
            access_key_id?,
            secret_access_key?,
            session_token,
            None,
            "aws-cli-export-credentials",
        ))
    }

    async fn runtime_client() -> BedrockRuntimeClient {
        let config = Self::sdk_config().await;
        BedrockRuntimeClient::new(&config)
    }

    async fn control_client() -> BedrockControlClient {
        let config = Self::sdk_config().await;
        BedrockControlClient::new(&config)
    }

    async fn validate_credentials_if_requested() -> Result<()> {
        let validate = std::env::var("JCODE_BEDROCK_VALIDATE_STS")
            .ok()
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
            .unwrap_or(false);
        if !validate {
            return Ok(());
        }
        let config = Self::sdk_config().await;
        let client = aws_sdk_sts::Client::new(&config);
        client
            .get_caller_identity()
            .send()
            .await
            .map(|_| ())
            .map_err(|err| {
                anyhow::anyhow!(Self::classify_error_message(&Self::sdk_error_message(&err)))
            })
    }

    fn configured_region() -> Option<String> {
        Self::env_or_config(REGION_ENV)
            .or_else(|| Self::env_or_config("AWS_REGION"))
            .or_else(|| Self::env_or_config("AWS_DEFAULT_REGION"))
    }

    pub fn configured_bearer_token() -> Option<String> {
        crate::provider_catalog::load_api_key_from_env_or_config(API_KEY_ENV, ENV_FILE)
    }

    fn env_or_config(name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| crate::provider_catalog::load_env_value_from_env_or_config(name, ENV_FILE))
    }

    fn persisted_catalog_path() -> Result<std::path::PathBuf> {
        Ok(crate::storage::app_config_dir()?.join("bedrock_models_cache.json"))
    }

    fn load_persisted_catalog() -> Option<PersistedCatalog> {
        let path = Self::persisted_catalog_path().ok()?;
        crate::storage::read_json(&path).ok()
    }

    fn persist_catalog(
        models: &[String],
        inference_profiles: &[String],
        profile_required_models: &HashSet<String>,
        inference_profile_routes: &HashMap<String, String>,
        legacy_models: &HashSet<String>,
    ) {
        let Ok(path) = Self::persisted_catalog_path() else {
            return;
        };
        let payload = PersistedCatalog {
            models: models.to_vec(),
            inference_profiles: inference_profiles.to_vec(),
            profile_required_models: profile_required_models.iter().cloned().collect(),
            inference_profile_routes: inference_profile_routes.clone(),
            legacy_models: legacy_models.iter().cloned().collect(),
            region: Self::configured_region(),
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(err) = crate::storage::write_json(&path, &payload) {
            crate::logging::warn(&format!(
                "Failed to persist Bedrock model catalog {}: {}",
                path.display(),
                err
            ));
        }
    }

    fn seed_cached_catalog(&self) {
        if let Some(catalog) = Self::load_persisted_catalog() {
            let configured_region = Self::configured_region();
            if catalog.region.as_deref() != configured_region.as_deref() {
                crate::logging::info(&format!(
                    "Ignoring Bedrock model cache for region {:?}; configured region is {:?}",
                    catalog.region, configured_region
                ));
                return;
            }
            let PersistedCatalog {
                models: cached_models,
                inference_profiles,
                profile_required_models,
                inference_profile_routes,
                legacy_models,
                ..
            } = catalog;
            let mut inference_profile_routes = inference_profile_routes;
            Self::merge_profile_routes_from_profile_ids(
                &mut inference_profile_routes,
                inference_profiles.iter(),
            );
            if let Ok(mut guard) = self.fetched_models.write() {
                *guard = cached_models;
            }
            if let Ok(mut profiles) = self.fetched_inference_profiles.write() {
                *profiles = inference_profiles;
            }
            if let Ok(mut required) = self.profile_required_models.write() {
                *required = profile_required_models.into_iter().collect();
            }
            if let Ok(mut routes) = self.inference_profile_routes.write() {
                *routes = inference_profile_routes;
            }
            if let Ok(mut legacy) = self.legacy_models.write() {
                *legacy = legacy_models.into_iter().collect();
            }
        }
    }

    fn classify_error_message(raw: &str) -> String {
        let lower = raw.to_ascii_lowercase();
        let is_legacy_model_error = lower.contains("marked by provider as legacy")
            || lower.contains("model is marked") && lower.contains("legacy")
            || lower.contains("have not been actively using the model in the last 30 days");
        if is_legacy_model_error {
            return format!(
                "{} Original error: {}",
                "This Bedrock model is marked as legacy for this account. Choose an active Bedrock model or an active inference profile instead.",
                raw.trim()
            );
        } else if lower.contains("doesn't support tool use")
            || lower.contains("does not support tool use")
            || lower.contains("tool use in streaming mode")
        {
            return format!(
                "{} Original error: {}",
                "This Bedrock model does not support tool use with streaming. Choose a Bedrock model with tool support, such as a Claude or Nova profile, or use a no-tools Bedrock model route.",
                raw.trim()
            );
        } else if lower.contains("no credentials")
            || lower.contains("could not load credentials")
            || lower.contains("credentials") && lower.contains("not loaded")
        {
            return "AWS credentials were not found. Set AWS_BEARER_TOKEN_BEDROCK, AWS_PROFILE, AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY, or run `aws sso login`.".to_string();
        } else if lower.contains("expired") || lower.contains("sso") && lower.contains("token") {
            return "AWS SSO/session credentials look expired. Run `aws sso login --profile <profile>` and retry.".to_string();
        }

        let hint = if lower.contains("accessdenied")
            || lower.contains("access denied")
            || lower.contains("not authorized")
        {
            "AWS IAM denied the Bedrock request. Ensure the principal can call bedrock:InvokeModel, bedrock:InvokeModelWithResponseStream, bedrock:ListFoundationModels, and bedrock:ListInferenceProfiles as needed."
        } else if lower.contains("validationexception") && lower.contains("model")
            || lower.contains("model") && lower.contains("not found")
            || lower.contains("resource not found")
        {
            "Bedrock did not recognize this model in the selected region/account. Check model ID, inference profile ID, region, and model access."
        } else if lower.contains("throttl")
            || lower.contains("too many requests")
            || lower.contains("rate exceeded")
        {
            "Bedrock throttled the request. Retry later or request a quota increase."
        } else if lower.contains("region") && lower.contains("missing") {
            "AWS region is missing. Set AWS_REGION or JCODE_BEDROCK_REGION."
        } else {
            "Bedrock request failed. Check AWS credentials, region, model access, and IAM permissions."
        };
        format!("{} Original error: {}", hint, raw.trim())
    }

    fn sdk_error_message(err: &(impl std::fmt::Display + std::fmt::Debug)) -> String {
        let display = err.to_string();
        let trimmed = display.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("service error")
            || trimmed.eq_ignore_ascii_case("dispatch failure")
        {
            format!("{err:?}")
        } else {
            display
        }
    }

    fn json_to_document(value: &serde_json::Value) -> aws_smithy_types::Document {
        match value {
            serde_json::Value::Null => aws_smithy_types::Document::Null,
            serde_json::Value::Bool(v) => aws_smithy_types::Document::Bool(*v),
            serde_json::Value::Number(n) => {
                if let Some(v) = n.as_u64() {
                    aws_smithy_types::Document::from(v)
                } else if let Some(v) = n.as_i64() {
                    aws_smithy_types::Document::from(v)
                } else if let Some(v) = n.as_f64() {
                    aws_smithy_types::Document::from(v)
                } else {
                    aws_smithy_types::Document::Null
                }
            }
            serde_json::Value::String(v) => aws_smithy_types::Document::String(v.clone()),
            serde_json::Value::Array(values) => aws_smithy_types::Document::Array(
                values.iter().map(Self::json_to_document).collect(),
            ),
            serde_json::Value::Object(map) => aws_smithy_types::Document::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), Self::json_to_document(value)))
                    .collect::<HashMap<_, _>>(),
            ),
        }
    }

    fn image_format_for_media_type(media_type: &str) -> Option<ImageFormat> {
        match media_type.trim().to_ascii_lowercase().as_str() {
            "image/png" => Some(ImageFormat::Png),
            "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
            "image/gif" => Some(ImageFormat::Gif),
            "image/webp" => Some(ImageFormat::Webp),
            _ => None,
        }
    }

    fn image_block(media_type: &str, data: &str) -> Result<ImageBlock> {
        let format = Self::image_format_for_media_type(media_type).ok_or_else(|| {
            anyhow::anyhow!(
                "Bedrock image input does not support media type `{}`",
                media_type
            )
        })?;
        let bytes = BASE64.decode(data).with_context(|| {
            format!("Failed to decode {} image payload for Bedrock", media_type)
        })?;
        ImageBlock::builder()
            .format(format)
            .source(ImageSource::Bytes(Blob::new(bytes)))
            .build()
            .context("Failed to build Bedrock image block")
    }

    fn to_bedrock_messages(messages: &[JMessage], allow_images: bool) -> Result<Vec<Message>> {
        messages
            .iter()
            .filter_map(|msg| {
                let role = match msg.role {
                    JRole::User => ConversationRole::User,
                    JRole::Assistant => ConversationRole::Assistant,
                };
                let mut content = Vec::new();
                for block in &msg.content {
                    match block {
                        JContentBlock::Text { text, .. } => {
                            content.push(ContentBlock::Text(text.clone()))
                        }
                        JContentBlock::Image { media_type, data } => {
                            if !allow_images {
                                return Some(Err(anyhow::anyhow!(
                                    "Current Bedrock model does not advertise image input support"
                                )));
                            }
                            match Self::image_block(media_type, data) {
                                Ok(image) => content.push(ContentBlock::Image(image)),
                                Err(err) => return Some(Err(err)),
                            }
                        }
                        JContentBlock::ToolResult {
                            tool_use_id,
                            content: text,
                            is_error,
                        } => {
                            let status = if is_error.unwrap_or(false) {
                                aws_sdk_bedrockruntime::types::ToolResultStatus::Error
                            } else {
                                aws_sdk_bedrockruntime::types::ToolResultStatus::Success
                            };
                            let result =
                                match aws_sdk_bedrockruntime::types::ToolResultBlock::builder()
                                    .tool_use_id(tool_use_id)
                                    .status(status)
                                    .content(
                                        aws_sdk_bedrockruntime::types::ToolResultContentBlock::Text(
                                            text.clone(),
                                        ),
                                    )
                                    .build()
                                {
                                    Ok(result) => result,
                                    Err(err) => return Some(Err(anyhow::anyhow!(err))),
                                };
                            content.push(ContentBlock::ToolResult(result));
                        }
                        JContentBlock::ToolUse { id, name, input } => {
                            let tool_use =
                                match aws_sdk_bedrockruntime::types::ToolUseBlock::builder()
                                    .tool_use_id(id)
                                    .name(name)
                                    .input(Self::json_to_document(input))
                                    .build()
                                {
                                    Ok(tool_use) => tool_use,
                                    Err(err) => return Some(Err(anyhow::anyhow!(err))),
                                };
                            content.push(ContentBlock::ToolUse(tool_use));
                        }
                        _ => {}
                    }
                }
                if content.is_empty() {
                    return None;
                }
                Some(
                    Message::builder()
                        .role(role)
                        .set_content(Some(content))
                        .build()
                        .map_err(|err| anyhow::anyhow!(err)),
                )
            })
            .collect()
    }

    fn tool_config(tools: &[ToolDefinition]) -> Option<ToolConfiguration> {
        if tools.is_empty() {
            return None;
        }
        let bedrock_tools = tools
            .iter()
            .filter_map(|tool| {
                let schema = ToolInputSchema::Json(Self::json_to_document(&tool.input_schema));
                ToolSpecification::builder()
                    .name(&tool.name)
                    .description(tool.description.clone())
                    .input_schema(schema)
                    .build()
                    .ok()
                    .map(Tool::ToolSpec)
            })
            .collect::<Vec<_>>();
        if bedrock_tools.is_empty() {
            None
        } else {
            ToolConfiguration::builder()
                .set_tools(Some(bedrock_tools))
                .build()
                .ok()
        }
    }

    fn inference_config() -> Option<InferenceConfiguration> {
        let max_tokens = std::env::var("JCODE_BEDROCK_MAX_TOKENS")
            .ok()
            .and_then(|v| v.trim().parse::<i32>().ok())
            .filter(|v| *v > 0);
        let temperature = std::env::var("JCODE_BEDROCK_TEMPERATURE")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| (0.0..=1.0).contains(v));
        let top_p = std::env::var("JCODE_BEDROCK_TOP_P")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| (0.0..=1.0).contains(v));
        let stop_sequences = std::env::var("JCODE_BEDROCK_STOP_SEQUENCES")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty());
        if max_tokens.is_none()
            && temperature.is_none()
            && top_p.is_none()
            && stop_sequences.is_none()
        {
            return None;
        }
        Some(
            InferenceConfiguration::builder()
                .set_max_tokens(max_tokens)
                .set_temperature(temperature)
                .set_top_p(top_p)
                .set_stop_sequences(stop_sequences)
                .build(),
        )
    }

    fn normalize_model_id(model: &str) -> String {
        let mut value = model.trim().to_string();
        if let Some((_, tail)) = value.rsplit_once('/') {
            value = tail.to_string();
        }
        for prefix in ["us.", "eu.", "apac.", "global."] {
            if let Some(stripped) = value.strip_prefix(prefix) {
                value = stripped.to_string();
                break;
            }
        }
        value
    }

    fn foundation_model_id_from_arn(arn: &str) -> Option<String> {
        arn.rsplit_once("foundation-model/")
            .map(|(_, model)| model.trim())
            .filter(|model| !model.is_empty())
            .map(str::to_string)
    }

    fn inference_profile_id_from_arn(arn: &str) -> Option<String> {
        arn.rsplit_once("inference-profile/")
            .map(|(_, profile)| profile.trim())
            .filter(|profile| !profile.is_empty())
            .map(str::to_string)
    }

    fn foundation_model_id_from_profile_id(profile_id: &str) -> Option<String> {
        let id = profile_id.trim();
        let id = Self::inference_profile_id_from_arn(id).unwrap_or_else(|| id.to_string());
        for prefix in ["us.", "eu.", "apac.", "global."] {
            if let Some(model) = id.strip_prefix(prefix)
                && !model.is_empty()
            {
                return Some(model.to_string());
            }
        }
        None
    }

    fn region_profile_prefix() -> Option<&'static str> {
        let region = Self::configured_region()?;
        if region.starts_with("us-") {
            Some("us.")
        } else if region.starts_with("eu-") {
            Some("eu.")
        } else if region.starts_with("ap-") {
            Some("apac.")
        } else {
            None
        }
    }

    fn inference_profile_priority(profile_id: &str) -> u8 {
        let id = profile_id.trim().to_ascii_lowercase();
        if let Some(prefix) = Self::region_profile_prefix()
            && id.starts_with(prefix)
        {
            return 0;
        }
        if id.starts_with("us.") || id.starts_with("eu.") || id.starts_with("apac.") {
            1
        } else if id.starts_with("global.") {
            2
        } else {
            3
        }
    }

    fn insert_preferred_profile_route(
        routes: &mut HashMap<String, String>,
        foundation_model: &str,
        profile_id: &str,
    ) {
        let foundation_model = foundation_model.trim();
        let profile_id = profile_id.trim();
        if foundation_model.is_empty() || profile_id.is_empty() {
            return;
        }
        let should_replace = routes
            .get(foundation_model)
            .map(|current| {
                Self::inference_profile_priority(profile_id)
                    < Self::inference_profile_priority(current)
            })
            .unwrap_or(true);
        if should_replace {
            routes.insert(foundation_model.to_string(), profile_id.to_string());
        }
    }

    fn merge_profile_routes_from_profile_ids(
        routes: &mut HashMap<String, String>,
        profiles: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        for profile in profiles {
            let profile = profile.as_ref().trim();
            let Some(foundation_model) = Self::foundation_model_id_from_profile_id(profile) else {
                continue;
            };
            let profile_id =
                Self::inference_profile_id_from_arn(profile).unwrap_or_else(|| profile.to_string());
            Self::insert_preferred_profile_route(routes, &foundation_model, &profile_id);
        }
    }

    fn profile_route_for_model(&self, model: &str) -> Option<String> {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }

        if let Ok(routes) = self.inference_profile_routes.read()
            && let Some(route) = routes.get(model).cloned()
        {
            return Some(route);
        }

        if let Ok(profiles) = self.fetched_inference_profiles.read() {
            let mut derived = HashMap::new();
            Self::merge_profile_routes_from_profile_ids(&mut derived, profiles.iter());
            if let Some(route) = derived.get(model).cloned() {
                return Some(route);
            }
        }

        None
    }

    pub fn is_bedrock_model_id(model: &str) -> bool {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return false;
        }
        if trimmed.starts_with("arn:aws:bedrock:") {
            return true;
        }

        let id = Self::normalize_model_id(trimmed).to_ascii_lowercase();
        id.starts_with("anthropic.")
            || id.starts_with("amazon.")
            || id.starts_with("cohere.")
            || id.starts_with("ai21.")
            || id.starts_with("meta.")
            || id.starts_with("mistral.")
            || id.starts_with("stability.")
            || id.starts_with("writer.")
            || id.starts_with("deepseek.")
            || id.starts_with("openai.")
            || id.starts_with("qwen.")
            || id.starts_with("moonshot.")
            || id.starts_with("moonshotai.")
            || id.starts_with("minimax.")
            || id.starts_with("zai.")
            || id.starts_with("google.")
            || id.starts_with("nvidia.")
    }

    fn model_info(model: &str) -> BedrockModelInfo {
        let id = Self::normalize_model_id(model).to_ascii_lowercase();
        if id.contains("claude-opus-4") || id.contains("claude-sonnet-4") {
            BedrockModelInfo {
                context_tokens: 200_000,
                max_output_tokens: 64_000,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: true,
                pricing: Some((3_000_000, 15_000_000)),
            }
        } else if id.contains("claude-3-7-sonnet") || id.contains("claude-3-5-sonnet") {
            BedrockModelInfo {
                context_tokens: 200_000,
                max_output_tokens: 8_192,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: id.contains("3-7"),
                pricing: Some((3_000_000, 15_000_000)),
            }
        } else if id.contains("claude-3-5-haiku") || id.contains("claude-3-haiku") {
            BedrockModelInfo {
                context_tokens: 200_000,
                max_output_tokens: 8_192,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                pricing: Some((800_000, 4_000_000)),
            }
        } else if id.contains("amazon.nova-pro") {
            BedrockModelInfo {
                context_tokens: 300_000,
                max_output_tokens: 5_120,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                pricing: Some((800_000, 3_200_000)),
            }
        } else if id.contains("amazon.nova-2-lite") || id.contains("amazon.nova-lite") {
            BedrockModelInfo {
                context_tokens: 300_000,
                max_output_tokens: 5_120,
                supports_tools: true,
                supports_vision: true,
                supports_reasoning: false,
                pricing: Some((60_000, 240_000)),
            }
        } else if id.contains("amazon.nova-micro") {
            BedrockModelInfo {
                context_tokens: 128_000,
                max_output_tokens: 5_120,
                supports_tools: true,
                supports_vision: false,
                supports_reasoning: false,
                pricing: Some((35_000, 140_000)),
            }
        } else if id.starts_with("deepseek.") {
            BedrockModelInfo {
                context_tokens: 128_000,
                max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: true,
                pricing: None,
            }
        } else if id.contains("llama3-1-405b") || id.starts_with("meta.") {
            BedrockModelInfo {
                context_tokens: 128_000,
                max_output_tokens: 4_096,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: false,
                pricing: Some((5_320_000, 16_000_000)),
            }
        } else if id.starts_with("mistral.") {
            BedrockModelInfo {
                context_tokens: 128_000,
                max_output_tokens: 8_192,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: false,
                pricing: Some((4_000_000, 12_000_000)),
            }
        } else if id.starts_with("openai.")
            || id.starts_with("qwen.")
            || id.starts_with("moonshot.")
            || id.starts_with("moonshotai.")
            || id.starts_with("minimax.")
            || id.starts_with("zai.")
            || id.starts_with("google.")
            || id.starts_with("nvidia.")
            || id.starts_with("writer.")
        {
            BedrockModelInfo {
                context_tokens: DEFAULT_CONTEXT_LIMIT,
                max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: id.contains("thinking")
                    || id.contains("reason")
                    || id.contains("gpt-oss"),
                pricing: None,
            }
        } else {
            BedrockModelInfo {
                context_tokens: DEFAULT_CONTEXT_LIMIT,
                max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
                supports_tools: false,
                supports_vision: false,
                supports_reasoning: false,
                pricing: None,
            }
        }
    }

    fn route_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
        let info = Self::model_info(model);
        info.pricing.map(|(input, output)| {
            RouteCheapnessEstimate::metered(
                RouteCostSource::Heuristic,
                RouteCostConfidence::Medium,
                input,
                output,
                None,
                Some("AWS Bedrock public on-demand pricing heuristic; verify for your region/account".to_string()),
            )
        })
    }

    fn known_models() -> Vec<&'static str> {
        vec![
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
            "anthropic.claude-sonnet-4-20250514-v1:0",
            "anthropic.claude-opus-4-20250514-v1:0",
            "amazon.nova-pro-v1:0",
            "amazon.nova-lite-v1:0",
            "amazon.nova-micro-v1:0",
            "meta.llama3-1-405b-instruct-v1:0",
            "mistral.mistral-large-2407-v1:0",
        ]
    }

    fn all_display_models(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut models = Vec::new();
        let inference_profile_routes = self
            .inference_profile_routes
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let should_hide_duplicate_foundation_model =
            |model: &str| inference_profile_routes.contains_key(model);
        for model in Self::known_models().into_iter().map(str::to_string) {
            if should_hide_duplicate_foundation_model(&model) {
                continue;
            }
            if seen.insert(model.clone()) {
                models.push(model);
            }
        }
        if let Ok(fetched) = self.fetched_models.read() {
            for model in fetched.iter() {
                if should_hide_duplicate_foundation_model(model) {
                    continue;
                }
                if seen.insert(model.clone()) {
                    models.push(model.clone());
                }
            }
        }
        if let Ok(profiles) = self.fetched_inference_profiles.read() {
            for profile in profiles.iter() {
                if seen.insert(profile.clone()) {
                    models.push(profile.clone());
                }
            }
        }
        models
    }

    async fn refresh_catalog(&self) -> Result<(Vec<String>, Vec<String>)> {
        let client = Self::control_client().await;
        let mut models = Vec::new();
        let mut profile_required_models = HashSet::new();
        let mut legacy_models = HashSet::new();
        let model_resp = client
            .list_foundation_models()
            .send()
            .await
            .map_err(|err| {
                anyhow::anyhow!(Self::classify_error_message(&Self::sdk_error_message(&err)))
            })?;
        for summary in model_resp.model_summaries() {
            let model_id = summary.model_id();
            if !model_id.is_empty() {
                models.push(model_id.to_string());
                let inference_types = summary.inference_types_supported();
                let supports_on_demand = inference_types
                    .iter()
                    .any(|kind| kind.as_str() == "ON_DEMAND");
                let supports_inference_profile = inference_types
                    .iter()
                    .any(|kind| kind.as_str() == "INFERENCE_PROFILE");
                if supports_inference_profile && !supports_on_demand {
                    profile_required_models.insert(model_id.to_string());
                }
                if summary
                    .model_lifecycle()
                    .map(|lifecycle| lifecycle.status().as_str() == "LEGACY")
                    .unwrap_or(false)
                {
                    legacy_models.insert(model_id.to_string());
                }
            }
        }
        models.sort();
        models.dedup();

        let mut profiles = Vec::new();
        let mut inference_profile_routes = HashMap::new();
        match client.list_inference_profiles().send().await {
            Ok(resp) => {
                for summary in resp.inference_profile_summaries() {
                    let id = summary.inference_profile_id();
                    if !id.is_empty() {
                        profiles.push(id.to_string());
                    }
                    let arn = summary.inference_profile_arn();
                    if !arn.is_empty() {
                        profiles.push(arn.to_string());
                    }
                    if summary.status().as_str() == "ACTIVE" && !id.is_empty() {
                        for model in summary.models() {
                            if let Some(model_arn) = model.model_arn()
                                && let Some(foundation_model) =
                                    Self::foundation_model_id_from_arn(model_arn)
                            {
                                Self::insert_preferred_profile_route(
                                    &mut inference_profile_routes,
                                    &foundation_model,
                                    id,
                                );
                            }
                        }
                    }
                }
                profiles.sort();
                profiles.dedup();
                Self::merge_profile_routes_from_profile_ids(
                    &mut inference_profile_routes,
                    profiles.iter(),
                );
            }
            Err(err) => {
                crate::logging::info(&format!(
                    "Bedrock inference profile discovery skipped: {}",
                    Self::classify_error_message(&Self::sdk_error_message(&err))
                ));
            }
        }

        if let Ok(mut guard) = self.fetched_models.write() {
            *guard = models.clone();
        }
        if let Ok(mut guard) = self.fetched_inference_profiles.write() {
            *guard = profiles.clone();
        }
        if let Ok(mut guard) = self.profile_required_models.write() {
            *guard = profile_required_models.clone();
        }
        if let Ok(mut guard) = self.inference_profile_routes.write() {
            *guard = inference_profile_routes.clone();
        }
        if let Ok(mut guard) = self.legacy_models.write() {
            *guard = legacy_models.clone();
        }
        Self::persist_catalog(
            &models,
            &profiles,
            &profile_required_models,
            &inference_profile_routes,
            &legacy_models,
        );
        Ok((models, profiles))
    }
}

impl Default for BedrockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    async fn complete(
        &self,
        messages: &[JMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Self::validate_credentials_if_requested().await?;
        let model = self.model();
        let info = Self::model_info(&model);
        let request_messages = Self::to_bedrock_messages(messages, info.supports_vision)?;
        let tool_config = if info.supports_tools {
            Self::tool_config(tools)
        } else {
            None
        };
        let inference_config = Self::inference_config();
        let system_blocks = if system.trim().is_empty() {
            None
        } else {
            Some(vec![SystemContentBlock::Text(system.to_string())])
        };
        let message_items = serde_json::to_value(messages)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let system_value = (!system.trim().is_empty()).then(|| Value::String(system.to_string()));
        let tools_value = if info.supports_tools && !tools.is_empty() {
            serde_json::to_value(tools).ok()
        } else {
            None
        };
        let payload = json!({
            "model": &model,
            "system": system_value.as_ref(),
            "messages": &message_items,
            "tools": tools_value.as_ref(),
            "supports_tools": info.supports_tools,
            "supports_vision": info.supports_vision,
            "inference_config_present": inference_config.is_some(),
        });
        super::fingerprint::log_provider_canonical_input(
            "bedrock",
            &model,
            "bedrock_converse_logical",
            &payload,
            &message_items,
            system_value.as_ref(),
            tools_value.as_ref(),
            Some(if info.supports_tools { tools.len() } else { 0 }),
            &[
                ("supports_tools", info.supports_tools.to_string()),
                ("supports_vision", info.supports_vision.to_string()),
                (
                    "inference_config_present",
                    inference_config.is_some().to_string(),
                ),
            ],
        );
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(64);
        tokio::spawn(async move {
            let client = Self::runtime_client().await;
            let mut req = client
                .converse_stream()
                .model_id(model.clone())
                .set_messages(Some(request_messages));
            if let Some(system_blocks) = system_blocks {
                req = req.set_system(Some(system_blocks));
            }
            if let Some(tool_config) = tool_config {
                req = req.tool_config(tool_config);
            }
            if let Some(inference_config) = inference_config {
                req = req.inference_config(inference_config);
            }
            let resp = match req.send().await {
                Ok(resp) => resp,
                Err(err) => {
                    let _ = tx
                        .send(Err(anyhow::anyhow!(Self::classify_error_message(
                            &Self::sdk_error_message(&err)
                        ))))
                        .await;
                    return;
                }
            };
            let mut stream = resp.stream;
            let mut current_tool: Option<(String, String, String)> = None;
            loop {
                match stream.recv().await {
                    Ok(Some(event)) => match event {
                        ConverseStreamOutput::ContentBlockStart(start) => {
                            if let Some(ContentBlockStart::ToolUse(tool)) = start.start {
                                let id = tool.tool_use_id().to_string();
                                let name = tool.name().to_string();
                                current_tool = Some((id.clone(), name.clone(), String::new()));
                                let _ = tx.send(Ok(StreamEvent::ToolUseStart { id, name })).await;
                            }
                        }
                        ConverseStreamOutput::ContentBlockDelta(delta) => {
                            if let Some(d) = delta.delta {
                                match d {
                                    ContentBlockDelta::Text(text) => {
                                        let _ = tx.send(Ok(StreamEvent::TextDelta(text))).await;
                                    }
                                    ContentBlockDelta::ToolUse(tool_delta) => {
                                        let input = tool_delta.input();
                                        if !input.is_empty() {
                                            if let Some((_, _, buf)) = current_tool.as_mut() {
                                                buf.push_str(input);
                                            }
                                            let _ = tx
                                                .send(Ok(StreamEvent::ToolInputDelta(
                                                    input.to_string(),
                                                )))
                                                .await;
                                        }
                                    }
                                    ContentBlockDelta::ReasoningContent(reasoning) => {
                                        if let ReasoningContentBlockDelta::Text(text) = reasoning {
                                            let _ =
                                                tx.send(Ok(StreamEvent::ThinkingDelta(text))).await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        ConverseStreamOutput::ContentBlockStop(_) => {
                            if current_tool.take().is_some() {
                                let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                            }
                        }
                        ConverseStreamOutput::MessageStop(stop) => {
                            let reason = Some(format!("{:?}", stop.stop_reason()));
                            let _ = tx
                                .send(Ok(StreamEvent::MessageEnd {
                                    stop_reason: reason,
                                }))
                                .await;
                        }
                        ConverseStreamOutput::Metadata(meta) => {
                            if let Some(usage) = meta.usage() {
                                let _ = tx
                                    .send(Ok(StreamEvent::TokenUsage {
                                        input_tokens: Some(usage.input_tokens() as u64),
                                        output_tokens: Some(usage.output_tokens() as u64),
                                        cache_read_input_tokens: None,
                                        cache_creation_input_tokens: None,
                                    }))
                                    .await;
                            }
                        }
                        _ => {}
                    },
                    Ok(None) => break,
                    Err(err) => {
                        let _ = tx
                            .send(Err(anyhow::anyhow!(Self::classify_error_message(
                                &Self::sdk_error_message(&err)
                            ))))
                            .await;
                        break;
                    }
                }
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx))
            as Pin<
                Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>,
            >)
    }

    fn name(&self) -> &str {
        "bedrock"
    }

    fn model(&self) -> String {
        self.model.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn supports_image_input(&self) -> bool {
        Self::model_info(&self.model()).supports_vision
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = model.trim();
        let model = self
            .profile_route_for_model(model)
            .unwrap_or_else(|| model.to_string());
        *self.model.write().unwrap_or_else(|p| p.into_inner()) = model;
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        Self::known_models()
    }

    fn available_models_display(&self) -> Vec<String> {
        self.all_display_models()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.all_display_models()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        let legacy_models = self
            .legacy_models
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let profile_required_models = self
            .profile_required_models
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        self.all_display_models()
            .into_iter()
            .map(|model| {
                let info = Self::model_info(&model);
                let is_legacy = legacy_models.contains(&model);
                let profile_foundation = Self::foundation_model_id_from_profile_id(&model);
                let missing_required_profile = profile_foundation.is_none()
                    && profile_required_models.contains(&model)
                    && self.profile_route_for_model(&model).is_none();
                let mut features = Vec::new();
                if info.supports_tools {
                    features.push("tools");
                } else {
                    features.push("no tools");
                }
                if info.supports_vision {
                    features.push("vision");
                }
                if info.supports_reasoning {
                    features.push("reasoning");
                }
                ModelRoute {
                    model: model.clone(),
                    provider: "AWS Bedrock".to_string(),
                    api_method: "bedrock".to_string(),
                    available: !is_legacy && !missing_required_profile,
                    detail: if is_legacy {
                        "legacy Bedrock model; choose an active model or inference profile"
                            .to_string()
                    } else if missing_required_profile {
                        "requires an inference profile; run /refresh-model-list or allow bedrock:ListInferenceProfiles"
                            .to_string()
                    } else {
                        let mut parts = Vec::new();
                        if let Some(foundation) = profile_foundation {
                            parts.push(format!("inference profile for {}", foundation));
                        }
                        parts.push(format!("context ~{} tokens", info.context_tokens));
                        parts.push(format!("max output ~{}", info.max_output_tokens));
                        parts.push(features.join(", "));
                        format!(
                            "ConverseStream · {}",
                            parts
                                .into_iter()
                                .filter(|part| !part.trim().is_empty())
                                .collect::<Vec<_>>()
                                .join(" · ")
                        )
                    },
                    cheapness: Self::route_pricing(&model),
                }
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        self.refresh_catalog().await.map(|_| ())
    }

    async fn refresh_model_catalog(&self) -> Result<ModelCatalogRefreshSummary> {
        let before_models = self.available_models_display();
        let before_routes = self.model_routes();
        self.refresh_catalog().await?;
        let after_models = self.available_models_display();
        let after_routes = self.model_routes();
        Ok(summarize_model_catalog_refresh(
            before_models,
            after_models,
            before_routes,
            after_routes,
        ))
    }

    fn context_window(&self) -> usize {
        Self::model_info(&self.model()).context_tokens
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn uses_jcode_compaction(&self) -> bool {
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: Arc::new(RwLock::new(self.model())),
            fetched_models: self.fetched_models.clone(),
            fetched_inference_profiles: self.fetched_inference_profiles.clone(),
            profile_required_models: self.profile_required_models.clone(),
            inference_profile_routes: self.inference_profile_routes.clone(),
            legacy_models: self.legacy_models.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            crate::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.as_ref() {
                crate::env::set_var(self.key, value);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn detects_env_credentials_requires_region_and_credential_hint() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let _removed = [
            "JCODE_BEDROCK_ENABLE",
            API_KEY_ENV,
            REGION_ENV,
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
            "AWS_PROFILE",
            "JCODE_BEDROCK_PROFILE",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SHARED_CREDENTIALS_FILE",
            "AWS_CONFIG_FILE",
        ]
        .map(EnvVarGuard::remove);
        crate::env::set_var(REGION_ENV, "us-east-1");
        assert!(!BedrockProvider::has_credentials());
        crate::env::set_var("AWS_PROFILE", "test");
        assert!(BedrockProvider::has_credentials());
    }

    #[test]
    fn explicit_enable_marks_configured_for_instance_metadata_credentials() {
        let _guard = crate::storage::lock_test_env();
        crate::env::set_var("JCODE_BEDROCK_ENABLE", "1");
        assert!(BedrockProvider::has_credentials());
        crate::env::remove_var("JCODE_BEDROCK_ENABLE");
    }

    #[test]
    fn detects_bedrock_login_env_file_credentials() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        for key in [
            "JCODE_BEDROCK_ENABLE",
            API_KEY_ENV,
            REGION_ENV,
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
            "AWS_PROFILE",
            "JCODE_BEDROCK_PROFILE",
            "AWS_ACCESS_KEY_ID",
        ] {
            crate::env::remove_var(key);
        }

        assert!(!BedrockProvider::has_credentials());
        crate::provider_catalog::save_env_value_to_env_file(
            API_KEY_ENV,
            ENV_FILE,
            Some("test-key"),
        )
        .unwrap();
        crate::env::remove_var(API_KEY_ENV);
        assert!(!BedrockProvider::has_credentials());

        crate::provider_catalog::save_env_value_to_env_file(
            REGION_ENV,
            ENV_FILE,
            Some("us-east-2"),
        )
        .unwrap();
        crate::env::remove_var(REGION_ENV);

        assert_eq!(
            BedrockProvider::configured_bearer_token().as_deref(),
            Some("test-key")
        );
        assert_eq!(
            BedrockProvider::configured_region().as_deref(),
            Some("us-east-2")
        );
        assert!(BedrockProvider::has_credentials());
    }

    #[test]
    fn switches_arbitrary_model_ids() {
        let p = BedrockProvider::new();
        p.set_model("us.anthropic.claude-3-5-sonnet-20241022-v2:0")
            .unwrap();
        assert_eq!(p.model(), "us.anthropic.claude-3-5-sonnet-20241022-v2:0");
    }

    #[test]
    fn maps_profile_required_foundation_model_to_inference_profile() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let p = BedrockProvider::new();
        p.profile_required_models
            .write()
            .unwrap()
            .insert("amazon.nova-2-lite-v1:0".to_string());
        p.inference_profile_routes.write().unwrap().insert(
            "amazon.nova-2-lite-v1:0".to_string(),
            "us.amazon.nova-2-lite-v1:0".to_string(),
        );

        p.set_model("amazon.nova-2-lite-v1:0").unwrap();

        assert_eq!(p.model(), "us.amazon.nova-2-lite-v1:0");
    }

    #[test]
    fn maps_foundation_model_from_stale_cached_profile_list() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let p = BedrockProvider::new();
        *p.fetched_inference_profiles.write().unwrap() = vec![
            "global.amazon.nova-2-lite-v1:0".to_string(),
            "us.amazon.nova-2-lite-v1:0".to_string(),
        ];

        p.set_model("amazon.nova-2-lite-v1:0").unwrap();

        assert_eq!(p.model(), "us.amazon.nova-2-lite-v1:0");
    }

    #[test]
    fn hides_profile_required_foundation_model_when_profile_route_exists() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let p = BedrockProvider::new();
        *p.fetched_models.write().unwrap() = vec!["amazon.nova-2-lite-v1:0".to_string()];
        *p.fetched_inference_profiles.write().unwrap() =
            vec!["us.amazon.nova-2-lite-v1:0".to_string()];
        p.profile_required_models
            .write()
            .unwrap()
            .insert("amazon.nova-2-lite-v1:0".to_string());
        p.inference_profile_routes.write().unwrap().insert(
            "amazon.nova-2-lite-v1:0".to_string(),
            "us.amazon.nova-2-lite-v1:0".to_string(),
        );

        let display = p.all_display_models();

        assert!(
            !display
                .iter()
                .any(|model| model == "amazon.nova-2-lite-v1:0")
        );
        assert!(
            display
                .iter()
                .any(|model| model == "us.amazon.nova-2-lite-v1:0")
        );
    }

    #[test]
    fn hides_foundation_model_when_profile_route_exists() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let p = BedrockProvider::new();
        *p.fetched_models.write().unwrap() = vec!["amazon.nova-2-lite-v1:0".to_string()];
        *p.fetched_inference_profiles.write().unwrap() =
            vec!["us.amazon.nova-2-lite-v1:0".to_string()];
        p.inference_profile_routes.write().unwrap().insert(
            "amazon.nova-2-lite-v1:0".to_string(),
            "us.amazon.nova-2-lite-v1:0".to_string(),
        );

        let display = p.all_display_models();

        assert!(
            !display
                .iter()
                .any(|model| model == "amazon.nova-2-lite-v1:0")
        );
        assert!(
            display
                .iter()
                .any(|model| model == "us.amazon.nova-2-lite-v1:0")
        );
    }

    #[test]
    fn profile_required_foundation_model_without_profile_route_is_disabled() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let p = BedrockProvider::new();
        *p.fetched_models.write().unwrap() = vec!["amazon.nova-2-lite-v1:0".to_string()];
        p.profile_required_models
            .write()
            .unwrap()
            .insert("amazon.nova-2-lite-v1:0".to_string());

        let route = p
            .model_routes()
            .into_iter()
            .find(|route| route.model == "amazon.nova-2-lite-v1:0")
            .expect("profile-required foundation model should be listed with a reason");

        assert!(!route.available);
        assert!(route.detail.contains("requires an inference profile"));
    }

    #[test]
    fn global_inference_profiles_use_foundation_capabilities_and_detail() {
        let p = BedrockProvider::new();
        *p.fetched_inference_profiles.write().unwrap() =
            vec!["global.amazon.nova-2-lite-v1:0".to_string()];

        let route = p
            .model_routes()
            .into_iter()
            .find(|route| route.model == "global.amazon.nova-2-lite-v1:0")
            .expect("global inference profile should be listed");

        assert!(route.available);
        assert!(
            route
                .detail
                .contains("inference profile for amazon.nova-2-lite-v1:0")
        );
        assert!(route.detail.contains("tools"));
        assert!(!route.detail.contains("no tools"));
    }

    #[test]
    fn ignores_persisted_bedrock_catalog_from_different_region() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        {
            let _region = EnvVarGuard::set(REGION_ENV, "us-east-1");
            BedrockProvider::persist_catalog(
                &["openai.gpt-oss-120b-1:0".to_string()],
                &[],
                &HashSet::new(),
                &HashMap::new(),
                &HashSet::new(),
            );
        }
        let _region = EnvVarGuard::set(REGION_ENV, "us-east-2");

        let p = BedrockProvider::new();

        assert!(p.fetched_models.read().unwrap().is_empty());
    }

    #[test]
    fn prefers_region_inference_profile_over_global_profile() {
        let _guard = crate::storage::lock_test_env();
        let _region = EnvVarGuard::set(REGION_ENV, "us-east-2");
        let mut routes = HashMap::new();

        BedrockProvider::insert_preferred_profile_route(
            &mut routes,
            "amazon.nova-2-lite-v1:0",
            "global.amazon.nova-2-lite-v1:0",
        );
        BedrockProvider::insert_preferred_profile_route(
            &mut routes,
            "amazon.nova-2-lite-v1:0",
            "us.amazon.nova-2-lite-v1:0",
        );

        assert_eq!(
            routes.get("amazon.nova-2-lite-v1:0").map(String::as_str),
            Some("us.amazon.nova-2-lite-v1:0")
        );
    }

    #[test]
    fn known_context_and_vision_capabilities() {
        let p = BedrockProvider::new();
        p.set_model("anthropic.claude-3-5-sonnet-20241022-v2:0")
            .unwrap();
        assert!(p.supports_image_input());
        assert_eq!(p.context_window(), 200_000);
        p.set_model("amazon.nova-micro-v1:0").unwrap();
        assert!(!p.supports_image_input());
        assert_eq!(p.context_window(), 128_000);
    }

    #[test]
    fn known_no_tool_models_do_not_advertise_tools() {
        assert!(!BedrockProvider::model_info("us.deepseek.r1-v1:0").supports_tools);
        assert!(!BedrockProvider::model_info("deepseek.v3.2").supports_tools);
        assert!(
            !BedrockProvider::model_info("mistral.mistral-large-3-675b-instruct").supports_tools
        );
        assert!(!BedrockProvider::model_info("openai.gpt-oss-120b-1:0").supports_tools);
        assert!(BedrockProvider::model_info("us.amazon.nova-2-lite-v1:0").supports_tools);
        assert!(BedrockProvider::model_info("us.anthropic.claude-sonnet-4-6").supports_tools);
    }

    #[test]
    fn error_classification_mentions_model_access() {
        let message = BedrockProvider::classify_error_message(
            "ValidationException: The provided model identifier is invalid",
        );
        assert!(message.contains("model"));
        assert!(message.contains("region"));
    }

    #[test]
    fn error_classification_mentions_legacy_models() {
        let message = BedrockProvider::classify_error_message(
            "Access denied. This Model is marked by provider as Legacy and you have not been actively using the model in the last 30 days",
        );
        assert!(message.contains("legacy"));
        assert!(message.contains("active"));
        assert!(!message.starts_with("AWS IAM denied"));
    }

    #[test]
    fn tool_use_streaming_error_is_not_classified_as_legacy_sdk_type_name() {
        let message = BedrockProvider::classify_error_message(
            "ValidationException: This model doesn't support tool use in streaming mode. extensions_1x: {hyper_util::client::legacy::connect::http::HttpInfo}",
        );
        assert!(message.contains("does not support tool use"));
        assert!(!message.starts_with("This Bedrock model is marked as legacy"));
    }

    #[test]
    fn expired_sso_error_is_concise_and_actionable() {
        let message = BedrockProvider::classify_error_message(
            "ServiceError(ServiceError { source: AccessDeniedException(AccessDeniedException { message: Some(\"Bearer Token has expired\") }) })",
        );
        assert_eq!(
            message,
            "AWS SSO/session credentials look expired. Run `aws sso login --profile <profile>` and retry."
        );
    }

    #[test]
    fn missing_credentials_error_omits_sdk_blob() {
        let message = BedrockProvider::classify_error_message(
            "CredentialsNotLoaded: could not load credentials from any provider; extensions_1x: noisy sdk internals",
        );
        assert!(message.contains("AWS credentials were not found"));
        assert!(!message.contains("extensions_1x"));
    }

    #[test]
    fn legacy_model_route_is_unavailable_with_reason() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let _xdg = EnvVarGuard::set("XDG_CONFIG_HOME", temp.path().as_os_str());
        let p = BedrockProvider::new();
        *p.fetched_models.write().unwrap() =
            vec!["anthropic.claude-3-haiku-20240307-v1:0".to_string()];
        p.legacy_models
            .write()
            .unwrap()
            .insert("anthropic.claude-3-haiku-20240307-v1:0".to_string());

        let route = p
            .model_routes()
            .into_iter()
            .find(|route| route.model == "anthropic.claude-3-haiku-20240307-v1:0")
            .expect("legacy route should be listed");

        assert!(!route.available);
        assert!(route.detail.contains("legacy"));
    }

    #[tokio::test]
    #[ignore = "requires AWS credentials and enabled Bedrock model access"]
    async fn bedrock_live_smoke_test() {
        if std::env::var("JCODE_BEDROCK_LIVE_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let provider = BedrockProvider::new();
        let output = provider
            .complete_simple("say bedrock ok and nothing else", "")
            .await
            .expect("live Bedrock completion");
        assert!(output.to_ascii_lowercase().contains("bedrock ok"));
    }
}
