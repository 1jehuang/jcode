use super::replay::RecordedSession;
use crate::config::Config;
use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use std::path::PathBuf;
use uuid::Uuid;

pub use super::sharing::*;

struct FileSystemShareStorage {
    base_path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl ShareStorage for FileSystemShareStorage {
    async fn store(&self, share: &ShareableSession) -> Result<ShareId> {
        let path = self.base_path.join(format!("{}.json", share.metadata.id.short_id));
        let content = serde_json::to_string_pretty(share)?;
        tokio::fs::write(path, content).await?;
        Ok(share.metadata.id.clone())
    }

    async fn load(&self, id: &ShareId) -> Result<ShareableSession> {
        let path = self.base_path.join(format!("{}.json", id.short_id));
        let content = tokio::fs::read_to_string(path).await?;
        serde_json::from_str(&content).context("Failed to parse shareable session")
    }

    async fn delete(&self, id: &ShareId) -> Result<()> {
        let path = self.base_path.join(format!("{}.json", id.short_id));
        tokio::fs::remove_file(path).await?;
        Ok(())
    }

    async fn search(&self, _query: &SearchQuery) -> Result<Vec<ShareMeta>> {
        let mut results = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&self.base_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        if let Ok(session) = serde_json::from_str::<ShareableSession>(&content) {
                            let meta = ShareMeta {
                                id: session.metadata.id,
                                title: session.metadata.title,
                                author_name: match &session.metadata.created_by {
                                    UserId::Anonymous => "Anonymous".to_string(),
                                    UserId::Registered { name, .. } => name.clone(),
                                },
                                created_at: session.metadata.created_at,
                                view_count: session.metadata.statistics.views,
                                duration_hint: None,
                                tags: session.metadata.tags,
                            };
                            results.push(meta);
                        }
                    }
                }
            }
        }
        Ok(results)
    }

    async fn list_user_shares(&self, _user: &UserId) -> Result<Vec<ShareMeta>> {
        let mut results = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&self.base_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        if let Ok(session) = serde_json::from_str::<ShareableSession>(&content) {
                            let meta = ShareMeta {
                                id: session.metadata.id,
                                title: session.metadata.title,
                                author_name: match &session.metadata.created_by {
                                    UserId::Anonymous => "Anonymous".to_string(),
                                    UserId::Registered { name, .. } => name.clone(),
                                },
                                created_at: session.metadata.created_at,
                                view_count: session.metadata.statistics.views,
                                duration_hint: None,
                                tags: session.metadata.tags,
                            };
                            results.push(meta);
                        }
                    }
                }
            }
        }
        Ok(results)
    }

    async fn increment_stat(&self, _id: &ShareId, _stat: StatType) -> Result<()> {
        Ok(())
    }
}

impl FileSystemShareStorage {
    fn new(base_path: std::path::PathBuf) -> Self {
        std::fs::create_dir_all(&base_path).ok();
        FileSystemShareStorage { base_path }
    }
}

pub struct SessionSharingManager {
    service: SessionSharingService,
    config: SharingConfig,
}

impl SessionSharingManager {
    pub fn new(_config: &Config) -> Self {
        let sharing_config = SharingConfig {
            base_url: "http://localhost:8080".to_string(),
            allow_anonymous: true,
            default_expiry: Some(Duration::days(7)),
            max_attachment_size_mb: 10,
            max_title_length: 200,
            rate_limit_per_hour: 100,
        };
        
        let data_dir = dirs::data_dir()
            .map(|d| d.join("carpai").join("shares"))
            .unwrap_or_else(|| PathBuf::from("./shares"));
        
        let storage = FileSystemShareStorage::new(data_dir);
        let service = SessionSharingService::new(Box::new(storage), sharing_config.clone());
        
        SessionSharingManager {
            service,
            config: sharing_config,
        }
    }

    pub async fn create_share_from_recorded(
        &self,
        session: RecordedSession,
        opts: ShareOptions,
    ) -> Result<ShareResult> {
        self.service.create_share(session, opts).await
    }

    pub async fn import_session_from_link(&self, link: &str) -> Result<ImportedSession> {
        let url = url::Url::parse(link)?;
        self.service.import_from_link(&url).await
    }

    pub async fn get_shared_session(&self, id: &str) -> Result<ShareableSession> {
        let share_id = ShareId::from_short_id(id)?;
        self.service.get_share(&share_id, None).await
    }

    pub async fn delete_shared_session(&self, id: &str, user: Option<&UserId>) -> Result<()> {
        let share_id = ShareId::from_short_id(id)?;
        let user_id = user.cloned().unwrap_or_default();
        self.service.delete_share(&share_id, &user_id).await
    }

    pub async fn search_shared_sessions(&self, query: &str) -> Result<Vec<ShareSearchResult>> {
        let mut search_query = SearchQuery::default();
        search_query.text = Some(query.to_string());
        self.service.search(&search_query).await
    }

    pub async fn get_trending_sessions(&self, limit: usize) -> Result<Vec<TrendingShare>> {
        self.service.trending(limit).await
    }

    pub async fn get_user_shares(&self, user: &UserId) -> Result<Vec<ShareMeta>> {
        self.service.by_user(user).await
    }

    pub fn get_config(&self) -> &SharingConfig {
        &self.config
    }
}

pub async fn create_shared_session(
    manager: &SessionSharingManager,
    title: Option<&str>,
    description: Option<&str>,
    tags: Vec<String>,
    expires_in_days: Option<i64>,
    visibility: ShareVisibility,
    password: Option<&str>,
    include_attachments: bool,
    anonymize: bool,
) -> Result<ShareResult> {
    let expires_in = expires_in_days.map(|d| Duration::days(d));
    
    let recorded = create_test_recorded_session();
    
    let opts = ShareOptions {
        owner: UserId::Anonymous,
        title: title.map(str::to_string),
        description: description.map(str::to_string),
        tags,
        expires_in,
        visibility,
        password: password.map(str::to_string),
        include_attachments,
        anonymize,
    };

    manager.create_share_from_recorded(recorded, opts).await
}

fn create_test_recorded_session() -> RecordedSession {
    use super::replay::{ProjectStateSnapshot, SessionMetadata, TokenUsageStats};
    
    RecordedSession {
        id: Uuid::new_v4(),
        recorded_at: Utc::now(),
        metadata: SessionMetadata {
            project_name: "Test Project".to_string(),
            project_path: PathBuf::from("."),
            git_branch: None,
            git_commit: None,
            user_id: None,
            provider_model: None,
            total_duration: Duration::seconds(0),
            token_usage: TokenUsageStats {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                estimated_cost_usd: None,
            },
        },
        events: Vec::new(),
        initial_state: ProjectStateSnapshot {
            files: Vec::new(),
            environment_vars: Vec::new(),
            working_directory: PathBuf::from("."),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn test_create_share_manager() {
        let config = Config::default();
        let manager = SessionSharingManager::new(&config);
        assert!(!manager.get_config().base_url.is_empty());
    }

    #[tokio::test]
    async fn test_share_id_generation() {
        let id = ShareId::generate();
        assert!(!id.short_id.is_empty());
        assert_eq!(id.short_id.len(), 6);
    }

    #[tokio::test]
    async fn test_share_id_to_url() {
        let id = ShareId::generate();
        let url = id.to_url("http://example.com");
        assert!(url.starts_with("http://example.com/s/"));
        assert!(url.ends_with(&id.short_id));
    }

    #[tokio::test]
    async fn test_share_options_defaults() {
        let opts = ShareOptions::default();
        assert!(matches!(opts.owner, UserId::Anonymous));
        assert!(opts.title.is_none());
        assert!(opts.description.is_none());
        assert!(opts.tags.is_empty());
        assert!(opts.expires_in.is_none());
        assert!(matches!(opts.visibility, ShareVisibility::Unlisted));
        assert!(opts.password.is_none());
        assert!(!opts.include_attachments);
        assert!(!opts.anonymize);
    }

    #[tokio::test]
    async fn test_share_visibility_default() {
        let visibility = ShareVisibility::default();
        assert!(matches!(visibility, ShareVisibility::Unlisted));
    }
}