//! File hooks - 文件事件处理

use crate::hooks::{HookHandler, HookEvent};
use anyhow::Result;

// File Open Hook
pub struct FileOpenHook;

#[async_trait::async_trait]
impl HookHandler for FileOpenHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::FileOpened { path } = event {
            tracing::debug!("File opened: {}", path);
            // TODO: Add file to context, trigger LSP, etc.
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "file_open"
    }

    fn priority(&self) -> u32 {
        10
    }
}

// File Save Hook
pub struct FileSaveHook;

#[async_trait::async_trait]
impl HookHandler for FileSaveHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::FileSaved { path } = event {
            tracing::debug!("File saved: {}", path);
            // TODO: Auto-format, run linters, update index
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "file_save"
    }

    fn priority(&self) -> u32 {
        20
    }
}

// File Close Hook
pub struct FileCloseHook;

#[async_trait::async_trait]
impl HookHandler for FileCloseHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::FileClosed { path } = event {
            tracing::debug!("File closed: {}", path);
            // TODO: Cleanup resources, save state
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "file_close"
    }

    fn priority(&self) -> u32 {
        30
    }
}
