//! Permission Protocol - 权限请求/响应协议实现
//!
//! 移植自 Claude Code SDK Control Protocol:
//! - `src/entrypoints/sdk/controlTypes.ts`
//! - `src/remote/RemoteSessionManager.ts` 权限流

use crate::types::*;
use std::collections::HashMap;
use uuid::Uuid;

// ============================================================================
// 权限请求管理器
// ============================================================================

/// 权限请求管理器
/// 
/// 管理 CCR -> Client 的权限请求生命周期:
/// ```
/// CCR 发送 ControlRequest -> 存入 pending -> 等待用户响应 -> 发送 ControlResponse
/// ```
pub struct PermissionRequestManager {
    /// 挂起的权限请求 (request_id -> request)
    pending_requests: HashMap<Uuid, SdkControlPermissionRequest>,
    
    /// 已处理的请求 (用于去重和历史记录)
    resolved_requests: HashMap<Uuid, SdkControlResponseBody>,
    
    /// 最大挂起请求数
    max_pending: usize,
}

impl PermissionRequestManager {
    /// 创建新的权限管理器
    pub fn new() -> Self {
        Self {
            pending_requests: HashMap::new(),
            resolved_requests: HashMap::new(),
            max_pending: 10,
        }
    }

    /// 接收新的权限请求
    pub fn receive_request(&mut self, request: SdkControlRequest) -> Option<Uuid> {
        match request {
            SdkControlRequest::Request { request_id, request } => {
                if self.pending_requests.len() >= self.max_pending {
                    warn!("Permission request queue full, dropping {:?}", request_id);
                    return None;
                }

                let info = SdkControlPermissionRequest {
                    request,
                    request_id,
                };
                
                let id = info.request_id;
                self.pending_requests.insert(id, info);
                
                tracing::debug!(
                    "Received permission request {}: subtype={}",
                    id,
                    info.request.subtype
                );
                
                Some(id)
            }
            
            SdkControlRequest::CancelRequest { request_id } => {
                // 取消挂起的请求
                if self.pending_requests.remove(&request_id).is_some() {
                    tracing::info!("Cancelled permission request {}", request_id);
                    
                    // 记录为已拒绝
                    self.resolved_requests.insert(
                        request_id,
                        SdkControlResponseBody {
                            subtype: "cancelled".to_string(),
                            request_id,
                            response: PermissionBehavior::Deny {
                                message: "Request cancelled by server".to_string(),
                            },
                        },
                    );
                }
                None
            }
        }
    }

    /// 获取所有挂起的请求信息 (用于 UI 展示)
    pub fn get_pending_info_list(&self) -> Vec<PermissionRequestInfo> {
        self.pending_requests
            .values()
            .map(|req| {
                let tool_name = req.request.extra.get("tool_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                
                let file_path = req.request.extra.get("file_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                PermissionRequestInfo {
                    tool_name,
                    file_path,
                    request_id: req.request_id,
                    requested_at: chrono::Utc::now(),
                    timed_out: false,
                }
            })
            .collect()
    }

    /// 响应权限请求 (允许)
    pub fn allow_request(
        &mut self,
        request_id: Uuid,
        updated_input: Option<HashMap<String, serde_json::Value>>,
    ) -> bool {
        self.respond_to_request(
            request_id,
            SdkControlResponseBody {
                subtype: "success".to_string(),
                request_id,
                response: PermissionBehavior::Allow { updated_input },
            },
        )
    }

    /// 响应权限请求 (拒绝)
    pub fn deny_request(&mut self, request_id: Uuid, message: &str) -> bool {
        self.respond_to_request(
            request_id,
            SdkControlResponseBody {
                subtype: "denied".to_string(),
                request_id,
                response: PermissionBehavior::Deny {
                    message: message.to_string(),
                },
            },
        )
    }

    /// 内部: 响应请求并清理挂起状态
    fn respond_to_request(
        &mut self,
        request_id: Uuid,
        response: SdkControlResponseBody,
    ) -> bool {
        // 移除挂起状态
        if self.pending_requests.remove(&request_id).is_none() {
            warn!("Attempted to respond to unknown permission request: {}", request_id);
            return false;
        }

        // 记录响应
        self.resolved_requests.insert(request_id, response);

        tracing::debug!(
            "Responded to permission request {}",
            request_id
        );

        true
    }

    /// 检查是否有挂起的请求
    pub fn has_pending_requests(&self) -> bool {
        !self.pending_requests.is_empty()
    }

    /// 获取挂起请求数量
    pub fn pending_count(&self) -> usize {
        self.pending_requests.len()
    }
}

impl Default for PermissionRequestManager {
    fn default() -> Self {
        Self::new()
    }
}
