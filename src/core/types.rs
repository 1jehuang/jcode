//! Common type definitions for CarpAI

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// User role in the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UserRole {
    Admin,
    Developer,
    Viewer,
}

/// Organization/tenant information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// User information with tenant context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub org_id: String,
    pub email: String,
    pub role: UserRole,
    pub display_name: Option<String>,
}

/// Session context passed through request lifecycle
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub session_id: String,
    pub user_id: String,
    pub tenant_id: String,
    pub permissions: Vec<String>,
}

/// API response wrapper with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ApiErrorDetail>,
    pub trace_id: String,
}

/// Detailed error information for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
    pub field: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            trace_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn error(code: String, message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(ApiErrorDetail {
                code,
                message,
                field: None,
            }),
            trace_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}
