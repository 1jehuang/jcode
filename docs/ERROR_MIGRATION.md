# thiserror 迁移指南

## 目标

逐步将 `anyhow::Result` 替换为细粒度 `thiserror` 类型，提升错误诊断能力。

## 迁移步骤

### 步骤 1：添加错误模块

已完成：`src/error_types.rs` 包含 6 个错误枚举

### 步骤 2：在目标模块中导入

```rust
// 改为
use crate::error_types::ProviderError;

// 替换函数签名
async fn call_api(&self) -> Result<Response, ProviderError> {
```

### 步骤 3：添加 From 实现

```rust
// 在 error_types.rs 中添加
impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        ProviderError::ApiCallFailed {
            provider: "anthropic".into(),
            status: e.status().map(|s| s.as_u16()).unwrap_or(0),
            message: e.to_string(),
        }
    }
}
```

### 步骤 4：迁移优先级

| 优先级 | 模块 | 说明 |
|--------|------|------|
| P0 | `src/provider/` | API 调用频繁，错误类型最需要区分的 |
| P1 | `src/tool/*.rs` | 工具执行错误需要区分超时/拒绝/失败 |
| P2 | `src/config.rs` | 配置加载错误简单，迁移成本低 |

### 模式示例

```rust
// before (anyhow)
async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
    let resp = client.post(url).json(&body).send().await?;
    let text = resp.text().await?;
    Ok(text)
}

// after (thiserror)
async fn complete(&self, prompt: &str) -> Result<String, ProviderError> {
    let resp = client.post(url).json(&body).send().await
        .map_err(|e| ProviderError::ApiCallFailed {
            provider: "anthropic".into(),
            status: e.status().map(|s| s.as_u16()).unwrap_or(0),
            message: e.to_string(),
        })?;
    let text = resp.text().await
        .map_err(|e| ProviderError::StreamError(e.to_string()))?;
    Ok(text)
}
```
