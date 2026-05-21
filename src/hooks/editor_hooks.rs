//! Editor hooks - 编辑器事件处理

use crate::hooks::{HookHandler, HookEvent};
use anyhow::Result;

// Cursor Move Hook
pub struct CursorMoveHook;

#[async_trait::async_trait]
impl HookHandler for CursorMoveHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::CursorMoved { line, col } = event {
            tracing::trace!("Cursor moved to {}:{}", line, col);
            // TODO: Update status bar, trigger hover info
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "cursor_move"
    }

    fn priority(&self) -> u32 {
        50
    }
}

// Selection Change Hook
pub struct SelectionChangeHook;

#[async_trait::async_trait]
impl HookHandler for SelectionChangeHook {
    async fn handle(&self, event: &HookEvent) -> Result<()> {
        if let HookEvent::SelectionChanged { start, end } = event {
            tracing::trace!("Selection changed: {:?} to {:?}", start, end);
            // TODO: Show selection info, enable actions
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "selection_change"
    }

    fn priority(&self) -> u32 {
        60
    }
}
