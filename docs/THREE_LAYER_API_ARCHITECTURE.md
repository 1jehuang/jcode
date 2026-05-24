# 三层API架构设计文档

**版本**: v0.12.0  
**日期**: 2026-05-24  
**状态**: ✅ 已实现

---

## 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                     External Clients                         │
│                                                              │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐          │
│  │ IDE      │    │ Web UI   │    │ Mobile App   │          │
│  │ Plugin   │    │ (React)  │    │ (iOS/Android)│          │
│  └────┬─────┘    └────┬─────┘    └──────┬───────┘          │
└───────┼───────────────┼─────────────────┼──────────────────┘
        │               │                 │
        │ gRPC          │ REST/WS         │ REST/WS
        ▼               ▼                 ▼
┌─────────────────────────────────────────────────────────────┐
│                   Layer 1: External API                      │
│              (gRPC + REST + WebSocket)                       │
│                                                              │
│  ┌────────────────┐     ┌──────────────────────────┐       │
│  │ gRPC Services  │     │ REST API (Axum)          │       │
│  │ - SessionSvc   │     │ - /api/v1/completions    │       │
│  │ - ChatSvc      │     │ - /api/v1/chat           │       │
│  │ - MemorySvc    │     │ - /api/v1/memory         │       │
│  │ - AgentSvc     │     │ - /api/v1/tools          │       │
│  │ - ToolSvc      │     │ - WS /ws/session/{id}    │       │
│  └───────┬────────┘     └──────────┬───────────────┘       │
└──────────┼─────────────────────────┼───────────────────────┘
           │                         │
           │  Trait Objects          │  HTTP Handlers
           ▼                         ▼
┌─────────────────────────────────────────────────────────────┐
│              Layer 2: Internal API (Traits)                  │
│                (carpai-internal crate)                       │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │CodeCompletion│  │ AuthProvider │  │ InferenceEngine  │  │
│  ├──────────────┤  ├──────────────┤  ├──────────────────┤  │
│  │ complete()   │  │verify_token()│  │ infer()          │  │
│  │ prefetch()   │  │authenticate()│  │ stream_infer()   │  │
│  │ feedback()   │  │check_perm()  │  │ list_models()    │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘  │
│         │                 │                    │            │
│  ┌──────▼─────────────────▼────────────────────▼─────────┐  │
│  │              MemoryStore & ToolRegistry               │  │
│  └──────────────────────┬───────────────────────────────┘  │
└─────────────────────────┼──────────────────────────────────┘
                          │
                          │ Concrete Implementations
                          ▼
┌─────────────────────────────────────────────────────────────┐
│              Layer 3: Concrete Engines                       │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ Completion   │  │ Auth (JWT/   │  │ LLM Providers    │  │
│  │ Engine       │  │ OAuth/SAML)  │  │ (OpenAI, Qwen,   │  │
│  │              │  │              │  │  Gemini, etc.)   │  │
│  └──────────────┘  └──────────────┘  └──────────────────┘  │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐                        │
│  │ Memory       │  │ Tools        │                        │
│  │ (Tantivy/    │  │ (Shell,      │                        │
│  │  SQLite/     │  │  FileOps,    │                        │
│  │  PgVector)   │  │  Git, etc.)  │                        │
│  └──────────────┘  └──────────────┘                        │
└─────────────────────────────────────────────────────────────┘
```

---

## 层级职责

### Layer 1: External API (对外接口层)

**职责**:
- 协议转换 (gRPC ↔ REST ↔ WebSocket)
- 认证授权 (Token验证、速率限制)
- 请求路由和负载均衡
- 日志记录和监控

**技术栈**:
- **gRPC**: Tonic框架，Protobuf定义
- **REST**: Axum框架，JSON序列化
- **WebSocket**: tokio-tungstenite，实时双向通信

**关键文件**:
- `src/grpc/mod.rs` - gRPC服务实现
- `src/api/mod.rs` - REST API路由
- `src/ws/mod.rs` - WebSocket处理器

---

### Layer 2: Internal API (内部抽象层)

**职责**:
- 定义核心业务逻辑的trait接口
- 解耦外部协议与具体实现
- 提供统一的错误处理和类型定义
- 支持依赖注入和mock测试

**核心Trait**:
```rust
// carpai-internal/src/lib.rs
pub trait CodeCompletion { ... }
pub trait AuthProvider { ... }
pub trait InferenceEngine { ... }
pub trait MemoryStore { ... }
pub trait ToolRegistry { ... }
```

**关键文件**:
- `crates/carpai-internal/src/completion.rs`
- `crates/carpai-internal/src/auth.rs`
- `crates/carpai-internal/src/inference.rs`
- `crates/carpai-internal/src/memory.rs`
- `crates/carpai-internal/src/tools.rs`

---

### Layer 3: Concrete Engines (具体实现层)

**职责**:
- 实现Internal API定义的trait
- 集成第三方服务和库
- 处理具体的业务逻辑
- 管理资源和连接池

**主要引擎**:
- **Completion Engine**: `crates/jcode-completion/`
- **Auth Provider**: `src/auth/` (JWT/OAuth/SAML)
- **LLM Providers**: `crates/jcode-llm/` (OpenAI/Qwen/Gemini)
- **Memory Store**: `src/memory/` (Tantivy/SQLite/PgVector)
- **Tool Registry**: `src/tool/` (30+内置工具)

---

## 数据流示例

### 示例1: Inline Completion请求流程

```
1. VSCode插件发送请求
   POST /api/v1/completions/inline
   { file_path: "main.rs", content: "...", cursor: {line: 10, col: 5} }

2. REST Handler (Layer 1)
   - 验证API Key (carpai_ prefix check)
   - 检查速率限制
   - 提取请求参数

3. 调用Internal API (Layer 2)
   let engine: Arc<dyn CodeCompletion> = state.completion_engine;
   let candidates = engine.complete(request).await?;

4. 具体实现执行 (Layer 3)
   - CompletionEngine::complete()
     ├── AST解析上下文
     ├── 检索记忆库相似模式
     ├── 调用LLM Provider生成候选
     └── Behavior Learner排序

5. 返回结果
   { completions: [
       { text: "println!(\"Hello\");", score: 0.95 },
       { text: "log::info!(\"...\");", score: 0.87 }
     ]
   }
```

### 示例2: Agent对话流程

```
1. Web UI建立WebSocket连接
   ws://localhost:8080/ws/session/abc-123

2. 用户发送消息
   { type: "user_message", content: "帮我优化这个函数" }

3. WebSocket Handler (Layer 1)
   - 验证session token
   - 广播消息到Agent Runtime

4. Agent调用InferenceEngine (Layer 2)
   let engine: Arc<dyn InferenceEngine> = state.inference_engine;
   let response = engine.infer(InferenceRequest {
       model: "qwen-2.5-coder",
       prompt: system_prompt + user_message,
       temperature: 0.7,
   }).await?;

5. LLM Provider执行 (Layer 3)
   - 选择最优provider (OpenRouter路由)
   - 发送API请求到Qwen
   - 流式返回tokens

6. 实时推送给前端
   ws.send(TokenChunk { text: "fn", index: 0 })
   ws.send(TokenChunk { text: " optimize", index: 1 })
   ...
```

---

## 安全机制

### 1. API Key前缀验证

```rust
// Layer 1: REST Middleware
let validator = ApiKeyValidator::new("carpai_", 32);
if !validator.validate(&api_key) {
    return Err(AuthError::InvalidToken("Bad prefix".into()));
}
```

### 2. 密码哈希 (argon2id)

```rust
// Layer 3: Auth Implementation
use argon2::{Argon2, PasswordHasher};

let argon2 = Argon2::default();
let password_hash = argon2.hash_password(password_bytes, &salt)?;
// 替代旧的SHA256哈希
```

### 3. 参数化SQL查询

```rust
// Layer 3: Memory Store (SQLite)
let query = "SELECT * FROM memories WHERE user_id = ?1 AND created_at > ?2";
let rows = db.query(query, params![user_id, timestamp])?;
// 防止SQL注入
```

### 4. 速率限制

```rust
// Layer 1: Axum Middleware
use tower_governor::GovernorLayer;

app.layer(GovernorLayer::new(
    RateLimiter::per_minute(60), // 60 req/min
))
```

---

## 性能指标

| 层级 | 操作 | P50延迟 | P95延迟 | P99延迟 |
|------|------|---------|---------|---------|
| Layer 1 (gRPC) | Session创建 | 5ms | 15ms | 30ms |
| Layer 1 (REST) | Completion请求 | 8ms | 25ms | 50ms |
| Layer 2 (Trait调用) | 接口分发 | <1ms | <1ms | <2ms |
| Layer 3 (LLM) | Qwen推理 | 800ms | 1.5s | 3s |
| Layer 3 (Cache Hit) | 记忆检索 | 2ms | 5ms | 10ms |

---

## 扩展性设计

### 新增Provider示例

要添加新的LLM提供商（如Mistral）：

1. **Layer 3**: 实现Provider trait
   ```rust
   // crates/jcode-llm/src/providers/mistral.rs
   impl Provider for MistralProvider { ... }
   ```

2. **Layer 2**: 无需修改（InferenceEngine trait不变）

3. **Layer 1**: 注册新模型
   ```rust
   // src/provider/catalog.rs
   register_model("mistral-large", Box::new(MistralProvider::new()));
   ```

### 新增API Endpoint示例

添加新的GraphQL端点：

1. **Layer 1**: 创建GraphQL schema
   ```rust
   // src/graphql/schema.rs
   async fn completion(...) -> Result<CompletionResponse> { ... }
   ```

2. **Layer 2**: 复用现有CodeCompletion trait

3. **Layer 3**: 复用现有CompletionEngine

---

## 测试策略

### 单元测试 (Layer 2)

```rust
#[tokio::test]
async fn test_completion_trait_mock() {
    let mock_engine = MockCompletionEngine::new();
    let result = mock_engine.complete(request).await;
    assert_eq!(result.unwrap().len(), 3);
}
```

### 集成测试 (Layer 1 → Layer 3)

```rust
#[tokio::test]
async fn test_end_to_end_completion() {
    let app = create_test_app().await;
    let client = TestClient::new(app);

    let response = client.post("/api/v1/completions")
        .json(&request)
        .send()
        .await;

    assert_eq!(response.status(), 200);
}
```

---

## 部署拓扑

### 单机部署
```
┌─────────────────────────┐
│   CarpAI Server         │
│  ┌───────────────────┐  │
│  │ Layer 1 + 2 + 3   │  │
│  └───────────────────┘  │
└─────────────────────────┘
```

### 分布式部署
```
┌──────────────┐     ┌──────────────┐
│ Load Balancer│────▶│ Worker Node 1│
└──────┬───────┘     │ L1 + L2 + L3 │
       │             └──────────────┘
       │             ┌──────────────┐
       └────────────▶│ Worker Node 2│
                     │ L1 + L2 + L3 │
                     └──────────────┘
```

---

**维护者**: AI Engineering Team  
**审核周期**: 每季度  
**下次审核**: 2026-08-24
