//! Metadata — 会话元数据管理
//!
//! ## 核心能力
//! - 会话元数据存储
//! - 自定义属性支持
//! - 元数据查询

use crate::types::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

/// 会话元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: SessionId,
    pub created_at: String,
    pub updated_at: String,
    pub attributes: HashMap<String, serde_json::Value>,
}

/// 元数据管理器
pub struct MetadataManager {
    metadata: std::sync::RwLock<HashMap<SessionId, SessionMetadata>>,
}

impl MetadataManager {
    /// 创建新的元数据管理器
    pub fn new() -> Self {
        Self {
            metadata: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// 创建会话元数据
    pub fn create_metadata(&self, session_id: &SessionId) -> SessionMetadata {
        let meta = SessionMetadata {
            session_id: session_id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            attributes: HashMap::new(),
        };

        self.metadata.write().unwrap_or_else(|e| e.into_inner()).insert(session_id.clone(), meta.clone());
        
        info!("Created metadata for session {}", session_id);
        meta
    }

    /// 获取会话元数据
    pub fn get_metadata(&self, session_id: &SessionId) -> Option<SessionMetadata> {
        self.metadata.read().unwrap_or_else(|e| e.into_inner()).get(session_id).cloned()
    }

    /// 设置属性
    pub fn set_attribute(
        &self,
        session_id: &SessionId,
        key: &str,
        value: serde_json::Value,
    ) -> anyhow::Result<()> {
        let mut meta = self.metadata.read().unwrap_or_else(|e| e.into_inner());
        
        if let Some(m) = meta.get_mut(session_id) {
            m.attributes.insert(key.to_string(), value);
            m.updated_at = chrono::Utc::now().to_rfc3339();
            
            debug!(
                session = %session_id,
                attribute = %key,
                "Set attribute"
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found: {}", session_id))
        }
    }

    /// 获取属性
    pub fn get_attribute(
        &self,
        session_id: &SessionId,
        key: &str,
    ) -> Option<serde_json::Value> {
        self.metadata
            .read()
            .unwrap()
            .get(session_id)
            .and_then(|m| m.attributes.get(key).cloned())
    }

    /// 删除属性
    pub fn remove_attribute(
        &self,
        session_id: &SessionId,
        key: &str,
    ) -> anyhow::Result<()> {
        let mut meta = self.metadata.read().unwrap_or_else(|e| e.into_inner());
        
        if let Some(m) = meta.get_mut(session_id) {
            m.attributes.remove(key);
            m.updated_at = chrono::Utc::now().to_rfc3339();
            
            debug!(
                session = %session_id,
                attribute = %key,
                "Removed attribute"
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found: {}", session_id))
        }
    }

    /// 列出所有元数据键
    pub fn list_attributes(&self, session_id: &SessionId) -> Vec<String> {
        self.metadata
            .read()
            .unwrap()
            .get(session_id)
            .map(|m| m.attributes.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 删除会话元数据
    pub fn delete_metadata(&self, session_id: &SessionId) {
        self.metadata.write().unwrap_or_else(|e| e.into_inner()).remove(session_id);
        info!("Deleted metadata for session {}", session_id);
    }
}
