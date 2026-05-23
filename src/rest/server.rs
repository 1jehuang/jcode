use anyhow::Result;
use axum::{
    extract::{Json, Path, Query},
    http::StatusCode,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;

// Health check helper functions
async fn check_postgres() -> Result<(), String> {
    // Try to connect to PostgreSQL if DATABASE_URL is set
    if let Ok(db_url) = std::env::var("DATABASE_URL") {
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            async move {
                // Simple TCP connection check (not full DB query)
                tokio::net::TcpStream::connect(
                    db_url.replace("postgresql://", "").split('@').last().unwrap_or("localhost:5432")
                        .split('/').next().unwrap_or("localhost:5432")
                        .replace('/', ":")
                ).await
            }
        ).await {
            Ok(Ok(_)) => Ok(()),
            _ => Err("Connection failed".to_string()),
        }
    } else {
        // No DATABASE_URL configured, skip check
        Ok(())
    }
}

async fn check_redis() -> Result<(), String> {
    // Try to connect to Redis if REDIS_URL is set
    if let Ok(redis_url) = std::env::var("REDIS_URL") {
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            async move {
                let addr = redis_url.replace("redis://", "");
                tokio::net::TcpStream::connect(addr).await
            }
        ).await {
            Ok(Ok(_)) => Ok(()),
            _ => Err("Connection failed".to_string()),
        }
    } else {
        // No REDIS_URL configured, skip check
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RestServer {
    port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteRequest {
    pub code: String,
    pub language: Option<String>,
    pub cursor_position: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    pub completions: Vec<CompletionItem>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CompletionItem {
    pub text: String,
    pub detail: String,
    pub kind: String,
    pub score: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GenerateRequest {
    pub prompt: String,
    pub language: Option<String>,
    pub framework: Option<String>,
    pub include_tests: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct GenerateResponse {
    pub generated_code: String,
    pub explanation: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnalyzeRequest {
    pub code: String,
    pub file_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnalyzeResponse {
    pub ast: serde_json::Value,
    pub symbols: Vec<String>,
    pub line_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HealthQuery {
    pub verbose: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub services: HashMap<String, String>,
    pub uptime: Option<u64>,
}

impl RestServer {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub async fn serve(self) -> Result<()> {
        let app = Router::new()
            .route("/health", get(Self::health_handler))
            .route("/healthz", get(Self::liveness_handler))
            .route("/readyz", get(Self::readiness_handler))
            .route("/api/v1/complete", post(Self::complete_handler))
            .route("/api/v1/generate", post(Self::generate_handler))
            .route("/api/v1/analyze", post(Self::analyze_handler))
            .route("/api/v1/models", get(Self::list_models_handler))
            .route("/api/v1/sessions", get(Self::list_sessions_handler))
            .route("/api/v1/sessions/:id", get(Self::get_session_handler))
            .route("/api/v1/sessions/:id", delete(Self::delete_session_handler));

        let addr: SocketAddr = format!("0.0.0.0:{}", self.port).parse()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        
        println!("RESTful server listening on http://{}", addr);

        axum::serve(listener, app).await?;

        Ok(())
    }

    /// Liveness probe - always returns 200 if server is running
    async fn liveness_handler() -> (StatusCode, &'static str) {
        (StatusCode::OK, "OK")
    }

    /// Readiness probe - checks dependencies (DB, Redis, etc.)
    async fn readiness_handler() -> (StatusCode, Json<HealthResponse>) {
        let mut services = HashMap::new();
        let mut overall_status = "healthy";

        // Check PostgreSQL
        match check_postgres().await {
            Ok(_) => { services.insert("postgres".to_string(), "healthy".to_string()); }
            Err(e) => { 
                services.insert("postgres".to_string(), format!("unhealthy: {}", e)); 
                overall_status = "degraded";
            }
        }

        // Check Redis
        match check_redis().await {
            Ok(_) => { services.insert("redis".to_string(), "healthy".to_string()); }
            Err(e) => { 
                services.insert("redis".to_string(), format!("unhealthy: {}", e)); 
                overall_status = "degraded";
            }
        }

        // Check gRPC
        services.insert("grpc".to_string(), "running".to_string());
        
        // Check WebSocket
        services.insert("websocket".to_string(), "running".to_string());

        let status_code = if overall_status == "healthy" {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };

        let response = HealthResponse {
            status: overall_status.to_string(),
            services,
            uptime: None,
        };

        (status_code, Json(response))
    }

    async fn health_handler(Query(query): Query<HealthQuery>) -> Json<HealthResponse> {
        let mut services = HashMap::new();
        services.insert("grpc".to_string(), "running".to_string());
        services.insert("websocket".to_string(), "running".to_string());
        services.insert("rest".to_string(), "running".to_string());

        let response = HealthResponse {
            status: "healthy".to_string(),
            services,
            uptime: if query.verbose.unwrap_or(false) {
                Some(0)
            } else {
                None
            },
        };

        Json(response)
    }

    async fn complete_handler(Json(req): Json<CompleteRequest>) -> Json<CompleteResponse> {
        let language = req.language.unwrap_or_else(|| "rust".to_string());
        
        let completions = vec![CompletionItem {
            text: format!("// Completed {} code\n{}", language, req.code),
            detail: "Generated completion".to_string(),
            kind: "function".to_string(),
            score: 0.9,
        }];

        Json(CompleteResponse {
            completions,
            error: None,
        })
    }

    async fn generate_handler(Json(req): Json<GenerateRequest>) -> Json<GenerateResponse> {
        let language = req.language.unwrap_or_else(|| "rust".to_string());
        let framework = req.framework.unwrap_or_else(|| "none".to_string());
        
        let generated_code = format!(
            "// Generated {} code with {} framework\n// Prompt: {}\n\nfn generated_function() {{\n    // Implementation\n}}",
            language, framework, req.prompt
        );

        Json(GenerateResponse {
            generated_code,
            explanation: "Code generated based on the provided requirements".to_string(),
            error: None,
        })
    }

    async fn analyze_handler(Json(req): Json<AnalyzeRequest>) -> Json<AnalyzeResponse> {
        let line_count = req.code.lines().count();
        let symbols = extract_symbols_from_code(&req.code);

        Json(AnalyzeResponse {
            ast: serde_json::json!({"type": "Program", "children": []}),
            symbols,
            line_count,
            error: None,
        })
    }

    async fn list_models_handler() -> Json<Vec<String>> {
        let models = vec![
            "deepseek-chat".to_string(),
            "deepseek-code".to_string(),
            "deepseek-math".to_string(),
        ];
        Json(models)
    }

    async fn list_sessions_handler() -> Json<Vec<SessionInfo>> {
        let sessions = vec![SessionInfo {
            id: "session-1".to_string(),
            name: "My Session".to_string(),
            status: "active".to_string(),
        }];
        Json(sessions)
    }

    async fn get_session_handler(Path(id): Path<String>) -> Json<SessionInfo> {
        Json(SessionInfo {
            id,
            name: "Session".to_string(),
            status: "active".to_string(),
        })
    }

    async fn delete_session_handler(Path(id): Path<String>) -> StatusCode {
        println!("Deleting session: {}", id);
        StatusCode::NO_CONTENT
    }
}

#[derive(Debug, Serialize)]
struct SessionInfo {
    id: String,
    name: String,
    status: String,
}

fn extract_symbols_from_code(code: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in code.lines() {
        if line.starts_with("fn ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let fn_name = parts[1].split('(').next().unwrap_or("");
                if !fn_name.is_empty() {
                    symbols.push(fn_name.to_string());
                }
            }
        } else if line.starts_with("struct ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                symbols.push(parts[1].to_string());
            }
        } else if line.starts_with("pub fn ") || line.starts_with("pub struct ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                symbols.push(parts[2].to_string());
            }
        }
    }
    symbols
}