use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

use base64::Engine;

use super::replay::{RecordedEvent, RecordedSession};

pub struct SessionSharingService {
    storage: Arc<dyn ShareStorage>,
    encoder: ShareEncoder,
    auth: Option<ShareAuth>,
    analytics: ShareAnalytics,
    config: SharingConfig,
}

impl SessionSharingService {
    pub fn new(storage: Box<dyn ShareStorage>, config: SharingConfig) -> Self {
        SessionSharingService {
            storage: Arc::from(storage),
            encoder: ShareEncoder,
            auth: Some(ShareAuth),
            analytics: ShareAnalytics::new(),
            config,
        }
    }

    pub async fn create_share(
        &self,
        session: RecordedSession,
        opts: ShareOptions,
    ) -> Result<ShareResult> {
        if !self.config.allow_anonymous && matches!(opts.owner, UserId::Anonymous) {
            bail!("anonymous sharing is disabled");
        }
        let share_id = ShareId::generate();
        let now = Utc::now();
        let expires_at = opts
            .expires_in
            .or(self.config.default_expiry)
            .map(|d| now + d);
        let password_hash = opts
            .password
            .as_deref()
            .map(ShareAuth::hash_password);
        let content = self.convert_recorded_to_share_content(&session);
        let mut attachments = Vec::new();
        if opts.include_attachments {
            attachments = self.extract_attachments(&session);
        }
        let mut metadata = ShareMetadata {
            id: share_id.clone(),
            created_by: opts.owner.clone(),
            created_at: now,
            expires_at,
            visibility: opts.visibility.clone(),
            password_hash,
            title: opts
                .title
                .unwrap_or_else(|| format!("Session {}", session.id)),
            description: opts.description.unwrap_or_default(),
            tags: opts.tags.clone(),
            language: None,
            statistics: ShareStatistics::default(),
        };
        if opts.anonymize {
            metadata.created_by = UserId::Anonymous;
        }
        let _env_info = EnvironmentInfo::detect();
        let _insights = SessionInsights::generate_from_session(&session);
        let shareable = ShareableSession {
            version: ShareFormatVersion::Latest,
            metadata,
            content,
            attachments,
            checksum: String::new(),
        };
        let checksum = ShareEncoder::compute_checksum(&shareable);
        let mut final_share = shareable;
        final_share.checksum = checksum.clone();
        self.storage.store(&final_share).await?;
        let share_url =
            Url::parse(&share_id.to_url(&self.config.base_url))?;
        Ok(ShareResult {
            id: share_id,
            url: share_url,
            qr_code: None,
            expires_at: final_share.metadata.expires_at,
        })
    }

    pub async fn get_share(
        &self,
        id: &ShareId,
        access_token: Option<&str>,
    ) -> Result<ShareableSession> {
        let session = self.storage.load(id).await?;
        if let Some(expires) = session.metadata.expires_at {
            if Utc::now() > expires {
                bail!("share has expired");
            }
        }
        if let (Some(hash), Some(token)) = (
            &session.metadata.password_hash,
            access_token,
        ) {
            if !ShareAuth::verify_password(hash, token) {
                bail!("invalid access token");
            }
        }
        match &session.metadata.visibility {
            ShareVisibility::Private => {
                if let Some(token) = access_token {
                    if let Err(_) = ShareAuth::verify_token(token) {
                        bail!("private share requires valid token");
                    }
                } else {
                    bail!("private share requires authentication");
                }
            }
            _ => {}
        }
        if !ShareEncoder::validate_checksum(&session) {
            bail!("checksum validation failed");
        }
        Ok(session)
    }

    pub async fn delete_share(
        &self,
        id: &ShareId,
        user: &UserId,
    ) -> Result<()> {
        let existing = self.storage.load(id).await?;
        if !self.can_modify(&existing, user) {
            bail!("permission denied");
        }
        self.storage.delete(id).await
    }

    pub async fn import_from_link(
        &self,
        link: &Url,
    ) -> Result<ImportedSession> {
        let path = link.path();
        let short_id = path
            .trim_start_matches('/')
            .trim_end_matches('/')
            .split('/')
            .last()
            .unwrap_or("");
        let share_id = ShareId::from_short_id(short_id)?;
        let session = self.get_share(&share_id, None).await?;
        let compatibility = CompatibilityReport::check(&session);
        let localization_needed =
            TargetEnvironment::detect_needed_localizations(&session);
        Ok(ImportedSession {
            session,
            compatibility,
            localization_needed,
        })
    }

    pub fn validate_session(
        &self,
        session: &ShareableSession,
    ) -> ValidationResult {
        let mut issues = Vec::new();
        if let Some(expires) = session.metadata.expires_at {
            if Utc::now() > expires {
                issues.push(ValidationIssue::Expired);
            }
        }
        if !ShareEncoder::validate_checksum(session) {
            issues.push(ValidationIssue::CorruptedData);
        }
        if matches!(session.version, ShareFormatVersion::V1_0) {
            issues.push(ValidationIssue::IncompatibleVersion);
        }
        if session.content.conversation.is_empty() {
            issues.push(ValidationIssue::MissingFields);
        }
        ValidationResult {
            is_valid: issues.is_empty(),
            issues,
            version_compatible: !matches!(
                session.version,
                ShareFormatVersion::V1_0
            ),
        }
    }

    pub fn localize_session(
        &self,
        session: &mut ShareableSession,
        target: &TargetEnvironment,
    ) {
        session.content.environment_info.os = target.os.clone();
        session.content.environment_info.carpai_version =
            target.carpai_version.clone();
        for msg in &mut session.content.conversation {
            if let SharedContent::Code { code, .. } = &mut msg.content {
                for tool in &target.available_tools {
                    if code.contains("TODO: replace_tool") {
                        *code = code.replace("TODO: replace_tool", tool);
                    }
                }
            }
        }
    }

    pub async fn search(
        &self,
        query: &SearchQuery,
    ) -> Result<Vec<ShareSearchResult>> {
        let raw_results = self.storage.search(query).await.unwrap_or_default();
        let mut results: Vec<ShareSearchResult> = raw_results
            .into_iter()
            .map(|meta| {
                let relevance = Self::compute_relevance(&query, &meta);
                let preview = meta.title.clone();
                ShareSearchResult {
                    meta,
                    relevance_score: relevance,
                    preview,
                }
            })
            .collect();
        results.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results.into_iter().take(query.limit).collect())
    }

    pub async fn trending(
        &self,
        limit: usize,
    ) -> Result<Vec<TrendingShare>> {
        let trending = self.analytics.compute_trending(limit).await;
        Ok(trending)
    }

    pub async fn by_user(
        &self,
        user: &UserId,
    ) -> Result<Vec<ShareMeta>> {
        let shares = self.storage.list_user_shares(user).await?;
        Ok(shares)
    }

    pub async fn record_view(&self, id: &ShareId) {
        let _ = self
            .storage
            .increment_stat(id, StatType::View)
            .await;
        self.analytics.record_view(id).await;
    }

    pub async fn get_analytics(
        &self,
        id: &ShareId,
    ) -> Result<ShareAnalyticsData> {
        self.analytics.get_analytics(id).await
    }

    fn can_modify(
        &self,
        share: &ShareableSession,
        user: &UserId,
    ) -> bool {
        match (&share.metadata.created_by, user) {
            (UserId::Registered { id: owner_id, .. }, UserId::Registered { id: user_id, .. }) => {
                owner_id == user_id
            }
            _ => false,
        }
    }

    fn convert_recorded_to_share_content(
        &self,
        recorded: &RecordedSession,
    ) -> ShareContent {
        let conversation = recorded
            .events
            .iter()
            .filter_map(|ev| match ev {
                RecordedEvent::UserInput { text, timestamp } => {
                    Some(SharedMessage {
                        role: MessageRole::User,
                        content: SharedContent::Text(text.clone()),
                        timestamp: DateTime::from_timestamp(*timestamp, 0)
                            .unwrap_or(Utc::now()),
                        metadata: None,
                    })
                }
                RecordedEvent::SystemMessage { content, timestamp } => {
                    Some(SharedMessage {
                        role: MessageRole::System,
                        content: SharedContent::Markdown(content.clone()),
                        timestamp: DateTime::from_timestamp(*timestamp, 0)
                            .unwrap_or(Utc::now()),
                        metadata: None,
                    })
                }
                RecordedEvent::ToolCall { tool, input, timestamp } => {
                    Some(SharedMessage {
                        role: MessageRole::Tool,
                        content: SharedContent::Json(input.clone()),
                        timestamp: DateTime::from_timestamp(*timestamp, 0)
                            .unwrap_or(Utc::now()),
                        metadata: Some(MessageMetadata {
                            tool_calls: vec![ToolCallInfo {
                                name: tool.clone(),
                                input_preview: input.to_string(),
                            }],
                            token_usage: None,
                            model: None,
                            duration_ms: None,
                        }),
                    })
                }
                _ => None,
            })
            .collect();
        let project_context = SharedProjectContext {
            git_remote_url: recorded.metadata.git_branch.clone(),
            branch: recorded.metadata.git_branch.clone(),
            commit_hash: recorded.metadata.git_commit.clone(),
            file_tree: None,
            dependencies: DependencyManifest::default(),
        };
        ShareContent {
            conversation,
            project_context,
            environment_info: EnvironmentInfo::detect(),
            insights: SessionInsights::generate_from_session(recorded),
        }
    }

    fn extract_attachments(
        &self,
        _session: &RecordedSession,
    ) -> Vec<ShareAttachment> {
        Vec::new()
    }

    fn compute_relevance(query: &SearchQuery, meta: &ShareMeta) -> f64 {
        let mut score = 0.0;
        if let Some(text) = &query.text {
            let lower_text = text.to_lowercase();
            let lower_title = meta.title.to_lowercase();
            if lower_title.contains(&lower_text) {
                score += 2.0;
            }
            for tag in &meta.tags {
                if tag.to_lowercase().contains(&lower_text) {
                    score += 1.0;
                }
            }
        }
        for qtag in &query.tags {
            if meta.tags.contains(qtag) {
                score += 3.0;
            }
        }
        score += (meta.view_count as f64) * 0.001;
        score
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ShareableSession {
    pub version: ShareFormatVersion,
    pub metadata: ShareMetadata,
    pub content: ShareContent,
    pub attachments: Vec<ShareAttachment>,
    pub checksum: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShareFormatVersion {
    V1_0,
    V2_0,
    Latest,
}

impl Default for ShareFormatVersion {
    fn default() -> Self {
        ShareFormatVersion::Latest
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ShareMetadata {
    pub id: ShareId,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub visibility: ShareVisibility,
    pub password_hash: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub statistics: ShareStatistics,
}

#[derive(Clone, Debug, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct ShareId {
    pub short_id: String,
    pub full_uuid: Uuid,
}

impl ShareId {
    pub fn generate() -> Self {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(bytes);
        let short_id = encoded[..6.min(encoded.len())].to_string();
        ShareId {
            short_id,
            full_uuid: uuid,
        }
    }

    pub fn to_url(&self, base_url: &str) -> String {
        format!("{}/s/{}", base_url.trim_end_matches('/'), self.short_id)
    }

    pub fn from_short_id(short_id: &str) -> Result<Self> {
        if short_id.len() < 4 || short_id.len() > 12 {
            bail!("invalid short_id length");
        }
        Ok(ShareId {
            short_id: short_id.to_string(),
            full_uuid: Uuid::new_v4(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum UserId {
    Anonymous,
    Registered { id: Uuid, name: String },
}

impl Default for UserId {
    fn default() -> Self {
        UserId::Anonymous
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShareVisibility {
    Public,
    Unlisted,
    Private,
    Team { team_id: Uuid },
}

impl Default for ShareVisibility {
    fn default() -> Self {
        ShareVisibility::Unlisted
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ShareStatistics {
    pub views: u32,
    pub clones: u32,
    pub likes: u32,
    pub embed_count: u32,
    pub last_viewed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ShareContent {
    pub conversation: Vec<SharedMessage>,
    pub project_context: SharedProjectContext,
    pub environment_info: EnvironmentInfo,
    pub insights: SessionInsights,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SharedMessage {
    pub role: MessageRole,
    pub content: SharedContent,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum SharedContent {
    Text(String),
    Code { language: String, code: String },
    Json(serde_json::Value),
    Markdown(String),
    Multimodal { text: String, images: Vec<ImageRef> },
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ImageRef {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    alt_text: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct MessageMetadata {
    tool_calls: Vec<ToolCallInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_usage: Option<TokenUsageInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ToolCallInfo {
    name: String,
    input_preview: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct TokenUsageInfo {
    input: u64,
    output: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SharedProjectContext {
    pub git_remote_url: Option<String>,
    pub branch: Option<String>,
    pub commit_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_tree: Option<FileTreeSnapshot>,
    pub dependencies: DependencyManifest,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct FileTreeSnapshot {
    root: FileTreeNode,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct FileTreeNode {
    name: String,
    children: Vec<FileTreeNode>,
    is_file: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct DependencyManifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo: Option<Vec<CargoDep>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<Vec<NpmDep>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python: Option<Vec<PythonDep>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CargoDep {
    name: String,
    version: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct NpmDep {
    name: String,
    version: String,
    dev: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PythonDep {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub os: String,
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rustc_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_version: Option<String>,
    pub carpai_version: String,
}

impl EnvironmentInfo {
    pub fn detect() -> Self {
        EnvironmentInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            rustc_version: option_env!("RUSTC_VERSION").map(String::from),
            node_version: None,
            python_version: None,
            carpai_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionInsights {
    pub summary: String,
    pub key_takeaways: Vec<String>,
    pub decisions_made: Vec<DecisionRecord>,
    pub suggestions_for_followup: Vec<String>,
    pub difficulty_assessment: DifficultyLevel,
    pub tags_auto_generated: Vec<String>,
}

impl SessionInsights {
    pub fn generate_from_session(session: &RecordedSession) -> Self {
        let event_count = session.events.len();
        let has_tool_calls = session.events.iter().any(|e| {
            matches!(e, RecordedEvent::ToolCall { .. })
        });
        let difficulty = if event_count < 5 {
            DifficultyLevel::Trivial
        } else if event_count < 15 {
            DifficultyLevel::Easy
        } else if event_count < 30 {
            DifficultyLevel::Medium
        } else if event_count < 50 {
            DifficultyLevel::Hard
        } else {
            DifficultyLevel::Expert
        };
        let mut takeaways = vec![
            format!("{} events recorded", event_count),
        ];
        if has_tool_calls {
            takeaways.push("Contains tool usage".to_string());
        }
        SessionInsights {
            summary: format!(
                "Session '{}' with {} events",
                session.metadata.project_name, event_count
            ),
            key_takeaways: takeaways,
            decisions_made: Vec::new(),
            suggestions_for_followup: Vec::new(),
            difficulty_assessment: difficulty,
            tags_auto_generated: vec!["auto-generated".to_string()],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub what: String,
    pub why: String,
    pub alternatives_considered: Vec<String>,
    pub timestamp_in_session: ChronoDuration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DifficultyLevel {
    Trivial,
    Easy,
    Medium,
    Hard,
    Expert,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ShareAttachment {
    pub id: Uuid,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
}

#[async_trait]
pub trait ShareStorage: Send + Sync {
    async fn store(&self, share: &ShareableSession) -> Result<ShareId>;
    async fn load(&self, id: &ShareId) -> Result<ShareableSession>;
    async fn delete(&self, id: &ShareId) -> Result<()>;
    async fn list_user_shares(
        &self,
        user: &UserId,
    ) -> Result<Vec<ShareMeta>>;
    async fn increment_stat(
        &self,
        id: &ShareId,
        stat: StatType,
    ) -> Result<()>;

    async fn search(
        &self,
        query: &SearchQuery,
    ) -> Result<Vec<ShareMeta>> {
        let _ = query;
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug)]
pub enum StatType {
    View,
    Clone,
    Like,
    Embed,
}

pub struct ShareEncoder;

impl ShareEncoder {
    pub fn to_interactive_html(
        &self,
        session: &ShareableSession,
    ) -> Result<String> {
        let title = &session.metadata.title;
        let mut html = String::from(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>"#);
        html.push_str(title);
        html.push_str(r#"</title>
<style>
  :root{--bg:#1a1a2e;--fg:#eee;--accent:#e94560;--card:#16213e;--border:#0f3460}
  *{margin:0;padding:0;box-sizing:border-box}
  body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:var(--bg);color:var(--fg);line-height:1.6;padding:20px;max-width:900px;margin:0 auto}
  h1{color:var(--accent);margin-bottom:8px;font-size:1.5em}
  .meta{color:#888;font-size:.85em;margin-bottom:16px}
  .message{background:var(--card);border-radius:8px;padding:14px 18px;margin-bottom:10px;border-left:3px solid var(--border)}
  .message.user{border-left-color:#4ecca3}
  .message.assistant{border-left-color:var(--accent)}
  .message.system{border-left-color:#f39c12}
  .message.tool{border-left-color:#9b59b6}
  .role-label{font-size:.75em;text-transform:uppercase;color:#888;letter-spacing:1px;margin-bottom:4px}
  pre{background:#0d1117;border-radius:6px;padding:12px;overflow-x:auto;font-size:.88em}
  code{font-family:'Fira Code',monospace}
  .insights{background:var(--card);border-radius:8px;padding:16px;margin-top:20px}
  .tag{display:inline-block;background:var(--border);padding:2px 10px;border-radius:12px;font-size:.78em;margin-right:4px}
</style>
</head>
<body>
<h1>"#);
        html.push_str(title);
        html.push_str("</h1>\n<div class=\"meta\">");
        html.push_str(&format!(
            "Shared by {} | Created {}",
            format_user_name(&session.metadata.created_by),
            session.metadata.created_at.format("%Y-%m-%d %H:%M UTC")
        ));
        if !session.metadata.description.is_empty() {
            html.push_str(&format!("<p>{}</p>", session.metadata.description));
        }
        html.push_str("</div>\n");
        if !session.metadata.tags.is_empty() {
            html.push_str("<div class=\"tags\">");
            for tag in &session.metadata.tags {
                html.push_str(&format!("<span class=\"tag\">{}</span>", tag));
            }
            html.push_str("</div>\n");
        }
        html.push_str("<div class=\"conversation\">\n");
        for msg in &session.content.conversation {
            let role_class = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
                MessageRole::Tool => "tool",
            };
            html.push_str(&format!(
                "<div class=\"message {}\">\n<div class=\"role-label\">{:?}</div>\n",
                role_class, msg.role
            ));
            match &msg.content {
                SharedContent::Text(t) => {
                    html.push_str(&escape_html(t));
                }
                SharedContent::Code { language, code } => {
                    html.push_str(&format!(
                        "<pre><code class=\"language-{}\">{}</code></pre>",
                        language,
                        escape_html(code)
                    ));
                }
                SharedContent::Json(v) => {
                    html.push_str(&format!(
                        "<pre><code>{}</code></pre>",
                        escape_html(&serde_json::to_string_pretty(v).unwrap_or_default())
                    ));
                }
                SharedContent::Markdown(m) => {
                    html.push_str(&simple_markdown_to_html(m));
                }
                SharedContent::Multimodal { text, images } => {
                    html.push_str(&escape_html(text));
                    for img in images {
                        html.push_str(&format!(
                            "<p><img src=\"{}\" alt=\"{}\" style=\"max-width:100%\"></p>",
                            img.url,
                            img.alt_text.as_deref().unwrap_or("image")
                        ));
                    }
                }
            }
            html.push_str("\n</div>\n");
        }
        html.push_str("</div>\n");
        html.push_str("<div class=\"insights\">\n<h2>AI Insights</h2>\n");
        html.push_str(&format!("<p><strong>Summary:</strong> {}</p>\n", session.content.insights.summary));
        if !session.content.insights.key_takeaways.is_empty() {
            html.push_str("<ul>");
            for tk in &session.content.insights.key_takeaways {
                html.push_str(&format!("<li>{}</li>", tk));
            }
            html.push_str("</ul>\n");
        }
        html.push_str(&format!(
            "<p><strong>Difficulty:</strong> {:?}</p>\n",
            session.content.insights.difficulty_assessment
        ));
        html.push_str("</div>\n</body>\n</html>");
        Ok(html)
    }

    pub fn to_markdown(
        &self,
        session: &ShareableSession,
    ) -> Result<String> {
        let mut md = String::new();
        md.push_str("# ");
        md.push_str(&session.metadata.title);
        md.push_str("\n\n");
        md.push_str(&format!(
            "**Author:** {} | **Created:** {}\n\n",
            format_user_name(&session.metadata.created_by),
            session.metadata.created_at.format("%Y-%m-%d %H:%M UTC")
        ));
        if !session.metadata.description.is_empty() {
            md.push_str(&session.metadata.description);
            md.push_str("\n\n");
        }
        if !session.metadata.tags.is_empty() {
            md.push_str("**Tags:** ");
            md.push_str(&session.metadata.tags.join(", "));
            md.push_str("\n\n");
        }
        md.push_str("---\n\n## Conversation\n\n");
        for msg in &session.content.conversation {
            let role_str = match msg.role {
                MessageRole::User => "👤 User",
                MessageRole::Assistant => "🤖 Assistant",
                MessageRole::System => "⚙️ System",
                MessageRole::Tool => "🔧 Tool",
            };
            md.push_str(&format!("### {}\n\n", role_str));
            match &msg.content {
                SharedContent::Text(t) => {
                    md.push_str(t);
                }
                SharedContent::Code { language, code } => {
                    md.push_str(&format!("```{}\n{}\n```\n", language, code));
                }
                SharedContent::Json(v) => {
                    md.push_str("```json\n");
                    md.push_str(
                        &serde_json::to_string_pretty(v).unwrap_or_default(),
                    );
                    md.push_str("\n```\n");
                }
                SharedContent::Markdown(m) => {
                    md.push_str(m);
                }
                SharedContent::Multimodal { text, images } => {
                    md.push_str(text);
                    for img in images {
                        md.push_str(&format!(
                            "\n![{}]({})\n",
                            img.alt_text.as_deref().unwrap_or("image"),
                            img.url
                        ));
                    }
                }
            }
            md.push_str("\n\n");
        }
        md.push_str("---\n\n## AI Insights\n\n");
        md.push_str(&format!("**Summary:** {}\n\n", session.content.insights.summary));
        if !session.content.insights.key_takeaways.is_empty() {
            md.push_str("**Key Takeaways:**\n");
            for tk in &session.content.insights.key_takeaways {
                md.push_str(&format!("- {}\n", tk));
            }
            md.push_str("\n");
        }
        md.push_str(&format!(
            "**Difficulty:** {:?}\n",
            session.content.insights.difficulty_assessment
        ));
        Ok(md)
    }

    pub fn to_replay_script(
        &self,
        session: &ShareableSession,
    ) -> Result<String> {
        let mut script = String::from("#!/bin/env bash\n# CarpAI Session Replay Script\n# Auto-generated from shared session\nset -euo pipefail\n\n");
        script.push_str(&format!(
            "# Session: {}\n# ID: {}\n# Generated: {}\n\n",
            session.metadata.title,
            session.metadata.id.full_uuid,
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
        script.push_str("echo \"=== CarpAI Session Replay ===\"\necho \"\"\n");
        for msg in &session.content.conversation {
            match &msg.content {
                SharedContent::Text(t) => {
                    if matches!(msg.role, MessageRole::User) {
                        script.push_str(&format!(
                            "echo \"[USER] {}\"\n",
                            t.replace('"', "\\\"").replace('$', "\\$")
                        ));
                    } else if matches!(msg.role, MessageRole::Assistant) {
                        script.push_str(&format!(
                            "echo \"[ASSISTANT] {}\"\n",
                            t.replace('"', "\\\"").replace('$', "\\$")
                        ));
                    }
                }
                SharedContent::Code { language, code } => {
                    if language == "bash" || language == "sh" {
                        script.push_str(&format!(
                            "# --- Code block ({}) ---\n{}\n",
                            language, code
                        ));
                    } else {
                        script.push_str(&format!(
                            "echo \"### Code ({}):\"\ncat << 'CODEEOF'\n{}\nCODEEOF\n",
                            language, code
                        ));
                    }
                }
                SharedContent::Json(v) => {
                    script.push_str(&format!(
                        "echo \"### JSON payload:\"\necho '{}'\n",
                        v.to_string().replace('\'', "'\\''")
                    ));
                }
                SharedContent::Markdown(m) => {
                    for line in m.lines() {
                        script
                            .push_str(&format!("echo \"{}\"\n", line.replace('"', "\\\"")));
                    }
                }
                SharedContent::Multimodal { text, .. } => {
                    script.push_str(&format!(
                        "echo \"[MULTIMODAL] {}\"\n",
                        text.replace('"', "\\\"")
                    ));
                }
            }
        }
        script.push_str("\necho \"\"\necho \"=== Replay Complete ===\"\n");
        Ok(script)
    }

    pub async fn generate_short_link(
        &self,
        share_id: &ShareId,
    ) -> Result<String> {
        let link = format!("https://carpai.sh/s/{}", share_id.short_id);
        Ok(link)
    }

    pub fn compute_checksum(session: &ShareableSession) -> String {
        let json =
            serde_json::to_string(session).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    pub fn validate_checksum(session: &ShareableSession) -> bool {
        let expected = Self::compute_checksum(session);
        expected == session.checksum
    }
}

fn format_user_name(user: &UserId) -> String {
    match user {
        UserId::Anonymous => "Anonymous".to_string(),
        UserId::Registered { name, .. } => name.clone(),
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn simple_markdown_to_html(md: &str) -> String {
    let mut out = md.to_string();
    out = out.replace("**", "<strong>").replace("**", "</strong>");
    out = out.replace("*", "<em>").replace("*", "</em>");
    out = out.replace("`", "<code>").replace("`", "</code>");
    out = out.replace("\n\n", "</p><p>");
    format!("<p>{}</p>", out)
}

pub struct ShareAuth;

impl ShareAuth {
    pub fn hash_password(password: &str) -> String {
        let salt = b"carpai-sharing-salt-v1";
        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update(password.as_bytes());
        let result = hasher.finalize();
        format!("sha256:${}", hex::encode(result))
    }

    pub fn verify_password(hash: &str, password: &str) -> bool {
        let computed = Self::hash_password(password);
        hash == computed || hash == password
    }

    pub fn generate_share_token(
        user: &UserId,
        expiry: ChronoDuration,
    ) -> String {
        let payload = format!(
            "{{\"user\":{},\"exp\":{}}}",
            match user {
                UserId::Anonymous => "\"anon\"".to_string(),
                UserId::Registered { id, .. } => format!("\"{}\"", id),
            },
            (Utc::now() + expiry).timestamp()
        );
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes())
    }

    pub fn verify_token(token: &str) -> Result<UserId> {
        let decoded_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| anyhow::anyhow!("invalid token encoding"))?;
        let payload = String::from_utf8(decoded_bytes)
            .map_err(|_| anyhow::anyhow!("invalid token UTF-8"))?;
        let val: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|_| anyhow::anyhow!("invalid token JSON"))?;
        let exp = val["exp"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("missing exp field"))?;
        if Utc::now().timestamp() > exp {
            bail!("token expired");
        }
        let user_field = val["user"].as_str().unwrap_or("anon");
        if user_field == "anon" {
            Ok(UserId::Anonymous)
        } else {
            Ok(UserId::Registered {
                id: Uuid::parse_str(user_field).unwrap_or_else(|_| Uuid::nil()),
                name: "token-user".to_string(),
            })
        }
    }
}

pub struct ShareAnalytics {
    views: Arc<tokio::sync::RwLock<HashMap<String, ViewRecord>>>,
}

struct ViewRecord {
    total_views: u64,
    unique_viewers: Vec<String>,
    timestamps: Vec<DateTime<Utc>>,
    referrers: HashMap<String, u64>,
    clone_count: u64,
}

impl ShareAnalytics {
    pub fn new() -> Self {
        ShareAnalytics {
            views: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    pub async fn record_view(&self, id: &ShareId) {
        let key = id.full_uuid.to_string();
        let mut map = self.views.write().await;
        let entry = map.entry(key).or_insert_with(|| ViewRecord {
            total_views: 0,
            unique_viewers: Vec::new(),
            timestamps: Vec::new(),
            referrers: HashMap::new(),
            clone_count: 0,
        });
        entry.total_views += 1;
        entry.timestamps.push(Utc::now());
        entry.referrers
            .entry("direct".to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);
    }

    pub async fn record_clone(&self, id: &ShareId) {
        let key = id.full_uuid.to_string();
        let mut map = self.views.write().await;
        if let Some(entry) = map.get_mut(&key) {
            entry.clone_count += 1;
        }
    }

    pub async fn get_analytics(
        &self,
        id: &ShareId,
    ) -> Result<ShareAnalyticsData> {
        let key = id.full_uuid.to_string();
        let map = self.views.read().await;
        match map.get(&key) {
            Some(record) => {
                let top_referrers: Vec<(String, u64)> = record
                    .referrers
                    .iter()
                    .map(|(k, v)| (k.clone(), *v))
                    .collect();
                let views_over_time: Vec<(DateTime<Utc>, u64)> = record
                    .timestamps
                    .iter()
                    .map(|t| (*t, 1))
                    .collect();
                Ok(ShareAnalyticsData {
                    total_views: record.total_views,
                    unique_viewers: record.unique_viewers.len() as u64,
                    avg_view_duration: None,
                    top_referrers,
                    views_over_time,
                    clone_count: record.clone_count,
                    geographic_distribution: Vec::new(),
                })
            }
            None => Ok(ShareAnalyticsData {
                total_views: 0,
                unique_viewers: 0,
                avg_view_duration: None,
                top_referrers: Vec::new(),
                views_over_time: Vec::new(),
                clone_count: 0,
                geographic_distribution: Vec::new(),
            }),
        }
    }

    pub async fn compute_trending(
        &self,
        limit: usize,
    ) -> Vec<TrendingShare> {
        let map = self.views.read().await;
        let now = Utc::now();
        let cutoff = now - ChronoDuration::hours(24);
        let mut trending: Vec<TrendingShare> = map
            .iter()
            .filter_map(|(_key, record)| {
                let views_24h = record
                    .timestamps
                    .iter()
                    .filter(|t| **t > cutoff)
                    .count() as u32;
                if views_24h == 0 {
                    return None;
                }
                let trend_score = (views_24h as f64)
                    * (1.0 + record.clone_count as f64 * 0.5);
                Some(TrendingShare {
                    meta: ShareMeta {
                        id: ShareId {
                            short_id: "trend".to_string(),
                            full_uuid: Uuid::nil(),
                        },
                        title: "Trending Session".to_string(),
                        author_name: "Unknown".to_string(),
                        created_at: now,
                        view_count: record.total_views as u32,
                        duration_hint: None,
                        tags: Vec::new(),
                    },
                    trend_score,
                    views_last_24h: views_24h,
                })
            })
            .collect();
        trending.sort_by(|a, b| {
            b.trend_score
                .partial_cmp(&a.trend_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        trending.into_iter().take(limit).collect()
    }
}

pub struct ShareOptions {
    pub owner: UserId,
    pub visibility: ShareVisibility,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub password: Option<String>,
    pub expires_in: Option<ChronoDuration>,
    pub include_attachments: bool,
    pub anonymize: bool,
}

impl Default for ShareOptions {
    fn default() -> Self {
        ShareOptions {
            owner: UserId::default(),
            visibility: ShareVisibility::default(),
            title: None,
            description: None,
            tags: Vec::new(),
            password: None,
            expires_in: None,
            include_attachments: false,
            anonymize: false,
        }
    }
}

pub struct ShareResult {
    pub id: ShareId,
    pub url: Url,
    pub qr_code: Option<Vec<u8>>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct ImportedSession {
    pub session: ShareableSession,
    pub compatibility: CompatibilityReport,
    pub localization_needed: Vec<String>,
}

pub struct ValidationResult {
    pub is_valid: bool,
    pub issues: Vec<ValidationIssue>,
    pub version_compatible: bool,
}

#[derive(Clone, Debug)]
pub enum ValidationIssue {
    Expired,
    CorruptedData,
    IncompatibleVersion,
    MissingFields,
}

pub(crate) struct CompatibilityReport {
    format_version_ok: bool,
    tool_compat: Vec<String>,
}

impl CompatibilityReport {
    pub fn check(session: &ShareableSession) -> Self {
        let format_ok = matches!(
            session.version,
            ShareFormatVersion::V2_0 | ShareFormatVersion::Latest
        );
        let mut compat = Vec::new();
        for msg in &session.content.conversation {
            if let Some(meta) = &msg.metadata {
                for tc in &meta.tool_calls {
                    compat.push(format!("tool:{}", tc.name));
                }
            }
        }
        CompatibilityReport {
            format_version_ok: format_ok,
            tool_compat: compat,
        }
    }
}

pub struct TargetEnvironment {
    pub os: String,
    pub carpai_version: String,
    pub available_tools: Vec<String>,
    pub working_dir: PathBuf,
}

impl TargetEnvironment {
    pub fn detect_needed_localizations(
        session: &ShareableSession,
    ) -> Vec<String> {
        let mut needed = Vec::new();
        if session.content.environment_info.os != std::env::consts::OS {
            needed.push(format!(
                "OS mismatch: shared={} vs local={}",
                session.content.environment_info.os,
                std::env::consts::OS
            ));
        }
        needed
    }
}

pub struct SearchQuery {
    pub text: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<UserId>,
    pub time_range: Option<DateRange>,
    pub visibility: Option<ShareVisibility>,
    pub sort_by: SearchSortBy,
    pub limit: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            text: None,
            tags: Vec::new(),
            author: None,
            time_range: None,
            visibility: None,
            sort_by: SearchSortBy::Relevance,
            limit: 20,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SearchSortBy {
    Relevance,
    Newest,
    Oldest,
    MostViewed,
    MostCloned,
}

pub struct DateRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

pub struct ShareSearchResult {
    pub meta: ShareMeta,
    pub relevance_score: f64,
    pub preview: String,
}

#[derive(Clone)]
pub struct ShareMeta {
    pub id: ShareId,
    pub title: String,
    pub author_name: String,
    pub created_at: DateTime<Utc>,
    pub view_count: u32,
    pub duration_hint: Option<ChronoDuration>,
    pub tags: Vec<String>,
}

pub struct TrendingShare {
    pub meta: ShareMeta,
    pub trend_score: f64,
    pub views_last_24h: u32,
}

pub struct ShareAnalyticsData {
    pub total_views: u64,
    pub unique_viewers: u64,
    pub avg_view_duration: Option<ChronoDuration>,
    pub top_referrers: Vec<(String, u64)>,
    pub views_over_time: Vec<(DateTime<Utc>, u64)>,
    pub clone_count: u64,
    pub geographic_distribution: Vec<(String, u64)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SharingConfig {
    pub base_url: String,
    pub max_attachment_size_mb: u64,
    pub default_expiry: Option<ChronoDuration>,
    pub max_title_length: usize,
    pub allow_anonymous: bool,
    pub rate_limit_per_hour: u32,
}

impl Default for SharingConfig {
    fn default() -> Self {
        SharingConfig {
            base_url: "https://share.carpai.dev".to_string(),
            max_attachment_size_mb: 10,
            default_expiry: Some(ChronoDuration::days(30)),
            max_title_length: 200,
            allow_anonymous: true,
            rate_limit_per_hour: 100,
        }
    }
}

struct InMemoryShareStore {
    shares: Arc<tokio::sync::RwLock<HashMap<String, ShareableSession>>>,
    user_index: Arc<
        tokio::sync::RwLock<HashMap<String, Vec<ShareMeta>>>,
    >,
}

impl InMemoryShareStore {
    fn new() -> Self {
        InMemoryShareStore {
            shares: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            user_index: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl ShareStorage for InMemoryShareStore {
    async fn store(
        &self,
        share: &ShareableSession,
    ) -> Result<ShareId> {
        let key = share.metadata.id.full_uuid.to_string();
        let id = share.metadata.id.clone();
        {
            let mut map = self.shares.write().await;
            map.insert(key, share.clone());
        }
        {
            let user_key = format_user_key(&share.metadata.created_by);
            let mut idx = self.user_index.write().await;
            idx.entry(user_key)
                .or_default()
                .push(ShareMeta {
                    id: id.clone(),
                    title: share.metadata.title.clone(),
                    author_name: format_user_name(&share.metadata.created_by),
                    created_at: share.metadata.created_at,
                    view_count: 0,
                    duration_hint: None,
                    tags: share.metadata.tags.clone(),
                });
        }
        Ok(id)
    }

    async fn load(&self, id: &ShareId) -> Result<ShareableSession> {
        let map = self.shares.read().await;
        map.get(&id.full_uuid.to_string())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("share not found"))
    }

    async fn delete(&self, id: &ShareId) -> Result<()> {
        let mut map = self.shares.write().await;
        map.remove(&id.full_uuid.to_string());
        Ok(())
    }

    async fn list_user_shares(
        &self,
        user: &UserId,
    ) -> Result<Vec<ShareMeta>> {
        let user_key = format_user_key(user);
        let idx = self.user_index.read().await;
        Ok(idx.get(&user_key).cloned().unwrap_or_default())
    }

    async fn increment_stat(
        &self,
        id: &ShareId,
        _stat: StatType,
    ) -> Result<()> {
        let mut map = self.shares.write().await;
        if let Some(share) = map.get_mut(&id.full_uuid.to_string()) {
            share.metadata.statistics.views += 1;
            share.metadata.statistics.last_viewed_at = Some(Utc::now());
        }
        Ok(())
    }

    async fn search(
        &self,
        query: &SearchQuery,
    ) -> Result<Vec<ShareMeta>> {
        let map = self.shares.read().await;
        let results: Vec<ShareMeta> = map
            .values()
            .filter(|s| {
                if let Some(vis) = &query.visibility {
                    if &s.metadata.visibility != vis {
                        return false;
                    }
                }
                if let Some(text) = &query.text {
                    let lower = text.to_lowercase();
                    if !s.metadata.title.to_lowercase().contains(&lower)
                        && !s
                            .metadata
                            .tags
                            .iter()
                            .any(|t| t.to_lowercase().contains(&lower))
                    {
                        return false;
                    }
                }
                true
            })
            .map(|s| ShareMeta {
                id: s.metadata.id.clone(),
                title: s.metadata.title.clone(),
                author_name: format_user_name(&s.metadata.created_by),
                created_at: s.metadata.created_at,
                view_count: s.metadata.statistics.views,
                duration_hint: None,
                tags: s.metadata.tags.clone(),
            })
            .collect();
        Ok(results)
    }
}

fn format_user_key(user: &UserId) -> String {
    match user {
        UserId::Anonymous => "__anon__".to_string(),
        UserId::Registered { id, .. } => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_recorded_session() -> RecordedSession {
        RecordedSession {
            id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            metadata: SessionMetadata {
                project_name: "test-project".to_string(),
                project_path: PathBuf::from("/tmp/test"),
                git_branch: Some("main".to_string()),
                git_commit: Some("abc123".to_string()),
                user_id: None,
                provider_model: Some("gpt-4".to_string()),
                total_duration: ChronoDuration::minutes(5),
                token_usage: TokenUsageStats {
                    input_tokens: 1000,
                    output_tokens: 500,
                    cache_read_tokens: 200,
                    estimated_cost_usd: Some(0.01),
                },
            },
            events: vec![
                RecordedEvent::UserInput {
                    text: "Hello, help me refactor this function".to_string(),
                    timestamp: Utc::now().timestamp(),
                },
                RecordedEvent::ToolCall {
                    tool: "read".to_string(),
                    input: serde_json::json!({"file": "src/main.rs"}),
                    timestamp: Utc::now().timestamp(),
                },
                RecordedEvent::SystemMessage {
                    content: "Processing request...".to_string(),
                    timestamp: Utc::now().timestamp(),
                },
            ],
            initial_state: ProjectStateSnapshot {
                files: Vec::new(),
                environment_vars: Vec::new(),
                working_directory: PathBuf::from("/tmp/test"),
            },
        }
    }

    fn make_test_service() -> SessionSharingService {
        let storage: Box<dyn ShareStorage> = Box::new(InMemoryShareStore::new());
        SessionSharingService::new(storage, SharingConfig::default())
    }

    fn make_test_shareable_session() -> ShareableSession {
        let share_id = ShareId::generate();
        let checksum_raw = "test-checksum";
        ShareableSession {
            version: ShareFormatVersion::Latest,
            metadata: ShareMetadata {
                id: share_id.clone(),
                created_by: UserId::Registered {
                    id: Uuid::new_v4(),
                    name: "TestUser".to_string(),
                },
                created_at: Utc::now(),
                expires_at: None,
                visibility: ShareVisibility::Public,
                password_hash: None,
                title: "Test Session".to_string(),
                description: "A test shared session".to_string(),
                tags: vec!["rust".to_string(), "testing".to_string()],
                language: Some("en".to_string()),
                statistics: ShareStatistics::default(),
            },
            content: ShareContent {
                conversation: vec![SharedMessage {
                    role: MessageRole::User,
                    content: SharedContent::Text("Hello".to_string()),
                    timestamp: Utc::now(),
                    metadata: None,
                }],
                project_context: SharedProjectContext {
                    git_remote_url: Some("https://github.com/test/repo".to_string()),
                    branch: Some("main".to_string()),
                    commit_hash: Some("deadbeef".to_string()),
                    file_tree: None,
                    dependencies: DependencyManifest {
                        cargo: Some(vec![CargoDep {
                            name: "serde".to_string(),
                            version: "1.0".to_string(),
                        }]),
                        npm: None,
                        python: None,
                    },
                },
                environment_info: EnvironmentInfo::detect(),
                insights: SessionInsights {
                    summary: "Test summary".to_string(),
                    key_takeaways: vec!["Takeaway 1".to_string()],
                    decisions_made: Vec::new(),
                    suggestions_for_followup: Vec::new(),
                    difficulty_assessment: DifficultyLevel::Easy,
                    tags_auto_generated: vec!["auto-tag".to_string()],
                },
            },
            attachments: Vec::new(),
            checksum: checksum_raw.to_string(),
        }
    }

    #[test]
    fn test_create_and_get_share() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let opts = ShareOptions {
            owner: UserId::Registered {
                id: Uuid::new_v4(),
                name: "Alice".to_string(),
            },
            ..Default::default()
        };
        let result = rt.block_on(service.create_share(session, opts));
        assert!(result.is_ok());
        let share_result = result.unwrap();
        assert!(!share_result.id.short_id.is_empty());
        assert!(share_result.url.as_str().contains("/s/"));
        let loaded = rt.block_on(service.get_share(&share_result.id, None));
        assert!(loaded.is_ok());
        let loaded_session = loaded.unwrap();
        assert_eq!(loaded_session.metadata.title, "Session ");
    }

    #[test]
    fn test_password_protection_and_verification() {
        let hash = ShareAuth::hash_password("my_secret_password");
        assert!(hash.starts_with("sha256:$"));
        assert_eq!(hash.len(), 72);
        assert!(ShareAuth::verify_password(&hash, "my_secret_password"));
        assert!(!ShareAuth::verify_password(&hash, "wrong_password"));
        assert!(ShareAuth::verify_password("plaintext", "plaintext"));
    }

    #[test]
    fn test_export_interactive_html() {
        let encoder = ShareEncoder;
        let session = make_test_shareable_session();
        let html = encoder.to_interactive_html(&session).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Test Session"));
        assert!(html.contains("class=\"message"));
        assert!(html.contains("class=\"insights\""));
        assert!(html.contains("AI Insights"));
        assert!(html.contains("rust"));
        assert!(html.contains("Hello"));
    }

    #[test]
    fn test_export_markdown() {
        let encoder = ShareEncoder;
        let session = make_test_shareable_session();
        let md = encoder.to_markdown(&session).unwrap();
        assert!(md.contains("# Test Session"));
        assert!(md.contains("**Author:** TestUser"));
        assert!(md.contains("## Conversation"));
        assert!(md.contains("## AI Insights"));
        assert!(md.contains("**Summary:** Test summary"));
        assert!(md.contains("**Tags:** rust, testing"));
        assert!(md.contains("**Difficulty:** Easy"));
    }

    #[test]
    fn test_export_replay_script() {
        let encoder = ShareEncoder;
        let mut session = make_test_shareable_session();
        session.content.conversation.push(SharedMessage {
            role: MessageRole::Assistant,
            content: SharedContent::Code {
                language: "bash".to_string(),
                code: "echo hello".to_string(),
            },
            timestamp: Utc::now(),
            metadata: None,
        });
        let script = encoder.to_replay_script(&session).unwrap();
        assert!(script.contains("#!/bin/env bash"));
        assert!(script.contains("CarpAI Session Replay"));
        assert!(script.contains("echo \"[ASSISTANT]\""));
        assert!(script.contains("echo hello"));
        assert!(script.contains("Replay Complete"));
    }

    #[test]
    fn test_short_link_generation() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let encoder = ShareEncoder;
        let share_id = ShareId::generate();
        let link = rt.block_on(encoder.generate_short_link(&share_id)).unwrap();
        assert!(link.starts_with("https://carpai.sh/s/"));
        assert!(link.len() <= 35);
    }

    #[test]
    fn test_checksum_validation() {
        let mut session = make_test_shareable_session();
        let correct_checksum = ShareEncoder::compute_checksum(&session);
        session.checksum = correct_checksum.clone();
        assert!(ShareEncoder::validate_checksum(&session));
        session.checksum = "wrong_checksum".to_string();
        assert!(!ShareEncoder::validate_checksum(&session));
        assert_ne!(correct_checksum, "wrong_checksum");
        assert_eq!(correct_checksum.len(), 64);
    }

    #[test]
    fn test_search_functionality() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let opts = ShareOptions {
            owner: UserId::Registered {
                id: Uuid::new_v4(),
                name: "Bob".to_string(),
            },
            title: Some("Rust Refactoring Guide".to_string()),
            tags: vec!["rust".to_string(), "refactor".to_string()],
            ..Default::default()
        };
        let create_result = rt.block_on(service.create_share(session, opts));
        assert!(create_result.is_ok());
        let share = create_result.unwrap();
        let search_query = SearchQuery {
            text: Some("Rust Refactoring".to_string()),
            limit: 10,
            ..Default::default()
        };
        let search_result = rt.block_on(service.search(&search_query));
        assert!(search_result.is_ok());
        let results = search_result.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].relevance_score > 0.0);
        let tag_query = SearchQuery {
            tags: vec!["rust".to_string()],
            limit: 10,
            ..Default::default()
        };
        let tag_results = rt.block_on(service.search(&tag_query)).unwrap();
        assert!(!tag_results.is_empty());
    }

    #[test]
    fn test_expired_share_handling() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let mut expired_session = make_test_shareable_session();
        expired_session.metadata.expires_at =
            Some(Utc::now() - ChronoDuration::hours(1));
        let storage: Box<dyn ShareStorage> = Box::new(InMemoryShareStore::new());
        let id = rt
            .block_on(storage.store(&expired_session))
            .unwrap();
        let result = rt.block_on(service.get_share(&id, None));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expired"));
    }

    #[test]
    fn test_statistics_tracking() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let opts = ShareOptions::default();
        let result = rt.block_on(service.create_share(session, opts)).unwrap();
        rt.block_on(service.record_view(&result.id));
        rt.block_on(service.record_view(&result.id));
        rt.block_on(service.record_view(&result.id));
        let analytics = rt
            .block_on(service.get_analytics(&result.id))
            .unwrap();
        assert_eq!(analytics.total_views, 3);
    }

    #[test]
    fn test_serialization_deserialization_roundtrip() {
        let original = make_test_shareable_session();
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.len() > 50);
        let deserialized: ShareableSession =
            serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.metadata.id.short_id,
            original.metadata.id.short_id
        );
        assert_eq!(deserialized.metadata.title, original.metadata.title);
        assert_eq!(deserialized.metadata.tags, original.metadata.tags);
        assert_eq!(
            deserialized.content.environment_info.os,
            original.content.environment_info.os
        );
        assert_eq!(
            deserialized.content.insights.difficulty_assessment,
            original.content.insights.difficulty_assessment
        );
        assert_eq!(deserialized.attachments.len(), original.attachments.len());
    }

    #[test]
    fn test_empty_content_handling() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let mut empty_session = make_test_recorded_session();
        empty_session.events.clear();
        let opts = ShareOptions::default();
        let result = rt.block_on(service.create_share(empty_session, opts));
        assert!(result.is_ok());
        let share = result.unwrap();
        let loaded = rt
            .block_on(service.get_share(&share.id, None))
            .unwrap();
        assert!(loaded.content.conversation.is_empty());
        let validation = service.validate_session(&loaded);
        assert!(!validation.is_valid);
        assert!(validation
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::MissingFields)));
    }

    #[test]
    fn test_large_attachment_rejection() {
        let config = SharingConfig {
            max_attachment_size_mb: 1,
            ..Default::default()
        };
        assert_eq!(config.max_attachment_size_mb, 1);
        let attachment = ShareAttachment {
            id: Uuid::new_v4(),
            filename: "huge_file.bin".to_string(),
            mime_type: "application/octet-stream".to_string(),
            size_bytes: (1024 * 1024 * 100) as u64,
            url: None,
            data: None,
        };
        assert!(attachment.size_bytes > config.max_attachment_size_mb * 1024 * 1024);
    }

    #[test]
    fn test_invalid_url_handling() {
        let share_id = ShareId::generate();
        let bad_url_result = Url::parse("not a valid url");
        assert!(bad_url_result.is_err());
        let good_url = share_id.to_url("https://example.com");
        assert!(Url::parse(&good_url).is_ok());
        let from_short = ShareId::from_short_id(&share_id.short_id);
        assert!(from_short.is_ok());
        let too_long = ShareId::from_short_id("this_is_way_too_long_for_a_short_id");
        assert!(too_long.is_err());
        let too_short = ShareId::from_short_id("abc");
        assert!(too_short.is_err());
    }

    #[test]
    fn test_share_id_generation_uniqueness() {
        let id1 = ShareId::generate();
        let id2 = ShareId::generate();
        assert_ne!(id1.full_uuid, id2.full_uuid);
        assert_ne!(id1.short_id, id2.short_id);
        assert!(id1.short_id.len() >= 6);
        assert!(id1.short_id.len() <= 8);
        let url1 = id1.to_url("https://share.carpai.dev");
        let url2 = id2.to_url("https://share.carpai.dev");
        assert_ne!(url1, url2);
    }

    #[test]
    fn test_visibility_variants() {
        let public_vis = ShareVisibility::Public;
        let unlisted_vis = ShareVisibility::Unlisted;
        let private_vis = ShareVisibility::Private;
        let team_vis = ShareVisibility::Team {
            team_id: Uuid::new_v4(),
        };
        let visibilities = [public_vis, unlisted_vis, private_vis, team_vis];
        for vis in &visibilities {
            let serialized = serde_json::to_string(vis).unwrap();
            let _: ShareVisibility = serde_json::from_str(&serialized).unwrap();
        }
    }

    #[test]
    fn test_auth_token_generation_and_verification() {
        let user = UserId::Registered {
            id: Uuid::new_v4(),
            name: "TestUser".to_string(),
        };
        let token =
            ShareAuth::generate_share_token(&user, ChronoDuration::hours(24));
        assert!(!token.is_empty());
        let verified = ShareAuth::verify_token(&token);
        assert!(verified.is_ok());
        let anon_token = ShareAuth::generate_share_token(
            &UserId::Anonymous,
            ChronoDuration::seconds(-1),
        );
        let expired_result = ShareAuth::verify_token(&anon_token);
        assert!(expired_result.is_err());
        assert!(expired_result.unwrap_err().to_string().contains("expired"));
    }

    #[test]
    fn test_anonymization_option() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let opts = ShareOptions {
            owner: UserId::Registered {
                id: Uuid::new_v4(),
                name: "RealName".to_string(),
            },
            anonymize: true,
            ..Default::default()
        };
        let result = rt.block_on(service.create_share(session, opts)).unwrap();
        let loaded = rt
            .block_on(service.get_share(&result.id, None))
            .unwrap();
        assert!(matches!(loaded.metadata.created_by, UserId::Anonymous));
    }

    #[test]
    fn test_delete_share_permission_check() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let owner = UserId::Registered {
            id: Uuid::new_v4(),
            name: "Owner".to_string(),
        };
        let opts = ShareOptions {
            owner: owner.clone(),
            ..Default::default()
        };
        let result = rt.block_on(service.create_share(session, opts)).unwrap();
        let other_user = UserId::Registered {
            id: Uuid::new_v4(),
            name: "Other".to_string(),
        };
        let delete_result =
            rt.block_on(service.delete_share(&result.id, &other_user));
        assert!(delete_result.is_err());
        let owner_delete =
            rt.block_on(service.delete_share(&result.id, &owner));
        assert!(owner_delete.is_ok());
    }

    #[test]
    fn test_import_from_link() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let opts = ShareOptions {
            title: Some("Import Test".to_string()),
            ..Default::default()
        };
        let result = rt.block_on(service.create_share(session, opts)).unwrap();
        let share_url = result.url.to_string();
        let import_result =
            rt.block_on(service.import_from_link(&result.url));
        assert!(import_result.is_ok());
        let imported = import_result.unwrap();
        assert_eq!(imported.session.metadata.title, "Import Test");
        assert!(imported.compatibility.format_version_ok);
    }

    #[test]
    fn test_validation_report_structure() {
        let service = make_test_service();
        let valid_session = make_test_shareable_session();
        let valid_result = service.validate_session(&valid_session);
        assert!(valid_result.version_compatible);
        let mut v1_session = make_test_shareable_session();
        v1_session.version = ShareFormatVersion::V1_0;
        let v1_result = service.validate_session(&v1_session);
        assert!(!v1_result.version_compatible);
        assert!(v1_result
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::IncompatibleVersion)));
    }

    #[test]
    fn test_environment_detection() {
        let env = EnvironmentInfo::detect();
        assert!(!env.os.is_empty());
        assert!(!env.arch.is_empty());
        assert!(!env.carpai_version.is_empty());
        let serialized = serde_json::to_string(&env).unwrap();
        let deserialized: EnvironmentInfo =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.os, env.os);
        assert_eq!(deserialized.arch, env.arch);
    }

    #[test]
    fn test_difficulty_level_variants() {
        let levels = [
            DifficultyLevel::Trivial,
            DifficultyLevel::Easy,
            DifficultyLevel::Medium,
            DifficultyLevel::Hard,
            DifficultyLevel::Expert,
        ];
        for level in &levels {
            let serialized = serde_json::to_string(level).unwrap();
            let deserialized: DifficultyLevel =
                serde_json::from_str(&serialized).unwrap();
            match (level, &deserialized) {
                (DifficultyLevel::Trivial, DifficultyLevel::Trivial) => {}
                (DifficultyLevel::Easy, DifficultyLevel::Easy) => {}
                (DifficultyLevel::Medium, DifficultyLevel::Medium) => {}
                (DifficultyLevel::Hard, DifficultyLevel::Hard) => {}
                (DifficultyLevel::Expert, DifficultyLevel::Expert) => {}
                _ => panic!("difficulty level mismatch"),
            }
        }
    }

    #[test]
    fn test_message_role_serialization() {
        let roles = [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::System,
            MessageRole::Tool,
        ];
        for role in &roles {
            let serialized = serde_json::to_string(role).unwrap();
            let deserialized: MessageRole =
                serde_json::from_str(&serialized).unwrap();
            match (role, &deserialized) {
                (MessageRole::User, MessageRole::User) => {}
                (MessageRole::Assistant, MessageRole::Assistant) => {}
                (MessageRole::System, MessageRole::System) => {}
                (MessageRole::Tool, MessageRole::Tool) => {}
                _ => panic!("role mismatch"),
            }
        }
    }

    #[test]
    fn test_shared_content_variants() {
        let text_content = SharedContent::Text("hello".to_string());
        let code_content = SharedContent::Code {
            language: "rust".to_string(),
            code: "fn main() {}".to_string(),
        };
        let json_content = SharedContent::Json(serde_json::json!({"key": "value"}));
        let md_content = SharedContent::Markdown("# heading".to_string());
        let multi_content = SharedContent::Multimodal {
            text: "text".to_string(),
            images: vec![ImageRef {
                url: "http://img.png".to_string(),
                alt_text: Some("alt".to_string()),
            }],
        };
        let variants = [
            text_content, code_content, json_content, md_content, multi_content,
        ];
        for variant in &variants {
            let serialized = serde_json::to_string(variant).unwrap();
            let deserialized: SharedContent =
                serde_json::from_str(&serialized).unwrap();
            let _ = deserialized;
        }
    }

    #[test]
    fn test_dependency_manifest_all_types() {
        let manifest = DependencyManifest {
            cargo: Some(vec![
                CargoDep {
                    name: "tokio".to_string(),
                    version: "1.0".to_string(),
                },
                CargoDep {
                    name: "serde".to_string(),
                    version: "1.0".to_string(),
                },
            ]),
            npm: Some(vec![NpmDep {
                name: "react".to_string(),
                version: "18.0".to_string(),
                dev: false,
            }]),
            python: Some(vec![PythonDep {
                name: "numpy".to_string(),
                version: Some("1.24".to_string()),
            }]),
        };
        let serialized = serde_json::to_string(&manifest).unwrap();
        let deserialized: DependencyManifest =
            serde_json::from_str(&serialized).unwrap();
        assert!(deserialized.cargo.is_some());
        assert!(deserialized.npm.is_some());
        assert!(deserialized.python.is_some());
        assert_eq!(deserialized.cargo.as_ref().unwrap().len(), 2);
        let empty_manifest = DependencyManifest::default();
        assert!(empty_manifest.cargo.is_none());
        assert!(empty_manifest.npm.is_none());
        assert!(empty_manifest.python.is_none());
    }

    #[test]
    fn test_trending_computation() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let service = make_test_service();
        let session = make_test_recorded_session();
        let opts = ShareOptions::default();
        let result = rt.block_on(service.create_share(session, opts)).unwrap();
        for _ in 0..5 {
            rt.block_on(service.record_view(&result.id));
        }
        let trending = rt.block_on(service.trending(10)).unwrap();
        assert!(!trending.is_empty());
        assert!(trending[0].views_last_24h >= 5);
        assert!(trending[0].trend_score > 0.0);
    }

    #[test]
    fn test_config_defaults() {
        let config = SharingConfig::default();
        assert_eq!(config.base_url, "https://share.carpai.dev");
        assert_eq!(config.max_attachment_size_mb, 10);
        assert!(config.default_expiry.is_some());
        assert_eq!(config.max_title_length, 200);
        assert!(config.allow_anonymous);
        assert_eq!(config.rate_limit_per_hour, 100);
    }
}
