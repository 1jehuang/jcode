// ════════════════════════════════════════════════════════════════
// KAIROS 文件传输通道 — Agent ↔ 用户之间的文件传输
//
// 使用场景:
//   - Agent 生成/修改了大文件 (如生成的代码、图片), 需要传给用户
//   - 用户有本地文件需要让 Agent 处理
//   - 跨会话的文件共享
//
// 实现方式:
//   1. 内存缓冲区 (小文件 <10MB)
//   2. 临时文件系统路径
//   3. 可选: Base64 编码内嵌传输
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 文件传输方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    /// 用户 -> Agent (上传)
    UserToAgent,
    /// Agent -> 用户 (下载)
    AgentToUser,
}

/// 文件传输记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransfer {
    pub id: String,
    
    /// 文件名
    pub file_name: String,
    
    /// MIME 类型
    pub mime_type: String,
    
    /// 文件大小 (bytes)
    pub size: u64,
    
    /// 传输方向
    pub direction: TransferDirection,
    
    /// 文件内容 (内存模式) 或文件路径 (磁盘模式)
    content: FileContent,
    
    /// 创建时间
    pub created_at: chrono::DateTime<chrono::Utc>,
    
    /// 是否已被接收方确认
    pub acknowledged: bool,
    
    /// 额外元数据
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileContent {
    /// 小文件直接存储在内存中
    InMemory { data: Vec<u8> },
    /// 大文件存储在临时目录
    OnDisk { path: std::path::PathBuf },
    /// 通过 URL 引用 (外部存储)
    UrlReference { url: String },
}

impl FileContent {
    pub fn size(&self) -> u64 {
        match self {
            Self::InMemory { data } => data.len() as u64,
            Self::OnDisk { path } => std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            Self::UrlReference { .. } => 0,
        }
    }
}

/// 文件传输管理器
pub struct KairosFileTransfer {
    transfers: Arc<RwLock<Vec<FileTransfer>>>,
    max_memory_size: u64, // 超过此大小使用磁盘存储
    
    // 临时文件目录
    temp_dir: Arc<std::path::PathBuf>,
}

impl Default for KairosFileTransfer {
    fn default() -> Self { Self::new() }
}

impl KairosFileTransfer {
    pub fn new() -> Self {
        let temp_dir = std::env::temp_dir().join("kairos");
        
        Self {
            transfers: Arc::new(RwLock::new(Vec::new())),
            max_memory_size: 10 * 1024 * 1024, // 10MB
            temp_dir: Arc::new(temp_dir),
        }
    }

    /// 发送文件给用户 (Agent -> User)
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let transfer = kairos.send_to_user(
    ///     "generated_code.rs",
    ///     b"use serde::{Serialize};...",
    ///     "application/rust",
    /// ).await?;
    /// println!("Download URL: {}", transfer.download_url());
    /// ```
    pub async fn send_to_user(
        &self,
        file_name: &str,
        data: &[u8],
        mime_type: &str,
        metadata: Option<HashMap<String, String>>,
    ) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let size = data.len() as u64;

        let content = if size <= self.max_memory_size {
            FileContent::InMemory { data: data.to_vec() }
        } else {
            self.write_to_disk(&id, file_name, data).await?
        };

        let transfer = FileTransfer {
            id: id.clone(),
            file_name: file_name.to_string(),
            mime_type: mime_type.to_string(),
            size,
            direction: TransferDirection::AgentToUser,
            content,
            created_at: chrono::Utc::now(),
            acknowledged: false,
            metadata: metadata.unwrap_or_default(),
        };

        self.transfers.write().await.push(transfer);

        tracing::info!(
            file = %file_name,
            size = size,
            id = %id,
            "File sent to user via Kairos"
        );

        Ok(id)
    }

    /// 从用户接收文件 (User -> Agent)
    pub async fn receive_from_user(
        &self,
        file_id: &str,
    ) -> Result<Vec<u8>, String> {
        let transfers = self.transfers.read().await;
        let transfer = transfers.iter()
            .find(|t| t.id == file_id && t.direction == TransferDirection::UserToAgent)
            .ok_or(format!("No pending file with ID '{}'", file_id))?;

        // 先克隆数据再释放锁
        let result = match &transfer.content {
            FileContent::InMemory { data } => {
                Ok(data.clone())
            }
            FileContent::OnDisk { path } => {
                tokio::fs::read(path).await
                    .map_err(|e| format!("Failed to read file: {}", e))
            }
            FileContent::UrlReference { url } => {
                reqwest::Client::new()
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| format!("Failed to download from {}: {}", url, e))?
                    .bytes()
                    .await
                    .map_err(|e| format!("Failed to read response body: {}", e))
                    .map(|b| b.to_vec())
            }
        };

        // 释放锁后再执行副作用操作
        drop(transfers);
        if result.is_ok() {
            self.acknowledge(file_id).await;
        }

        result
    }

    /// 确认文件已收到
    pub async fn acknowledge(&self, file_id: &str) {
        let mut transfers = self.transfers.write().await;
        if let Some(t) = transfers.iter_mut().find(|t| t.id == file_id) {
            t.acknowledged = true;
        }
    }

    /// 列出待接收的文件
    pub async fn list_pending_for_agent(&self) -> Vec<FileTransfer> {
        let transfers = self.transfers.read().await;
        transfers.iter()
            .filter(|t| t.direction == TransferDirection::UserToAgent && !t.acknowledged)
            .cloned()
            .collect()
    }

    /// 列出发送给用户的文件
    pub async fn list_sent_to_user(&self) -> Vec<FileTransfer> {
        let transfers = self.transfers.read().await;
        transfers.iter()
            .filter(|t| t.direction == TransferDirection::AgentToUser)
            .cloned()
            .collect()
    }

    /// 清理已确认的旧传输记录
    pub async fn cleanup_acked(&self, older_than_secs: u64) -> usize {
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(older_than_secs as i64);
        let mut transfers = self.transfers.write().await;
        let before = transfers.len();
        transfers.retain(|t| !t.acknowledged || t.created_at > cutoff);
        before - transfers.len()
    }

    /// 写入临时磁盘文件
    async fn write_to_disk(
        &self,
        id: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<FileContent, String> {
        tokio::fs::create_dir_all(&*self.temp_dir).await
            .map_err(|e| format!("Failed to create temp dir: {}", e))?;

        let safe_name = sanitize_filename(file_name);
        let path = self.temp_dir.join(format!("{}_{}", id, safe_name));

        tokio::fs::write(&path, data).await
            .map_err(|e| format!("Failed to write temp file: {}", e))?;

        Ok(FileContent::OnDisk { path })
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
}
