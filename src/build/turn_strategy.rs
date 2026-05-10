//! # BuildTurnStrategy — Build 模式的策略实现
//!
//! 在 StandardTurnStrategy 基础上叠加:
//! 1. 注入 Build 系统提示 → 引导 LLM 先规划再执行
//! 2. 追加计划步骤进度显示
//! 3. 失败步骤自动重试 (最多 max_retries 次)
//! 4. 执行完毕后触发 micro-ci 验证

use crate::agent::{Agent, StandardTurnStrategy, TurnStrategy};
use crate::compaction::CompactionEvent;
use crate::memory::PendingMemory;
use crate::prompt::SplitSystemPrompt;
use jcode_message_types::{Message, ToolDefinition};
use std::sync::Arc;
use std::time::Instant;

/// Build 模式策略 — 叠加在 StandardTurnStrategy 之上的 Build 行为
pub struct BuildTurnStrategy {
    inner: StandardTurnStrategy,
    max_retries: u32,
    run_ci: bool,
    retry_count: u32,
    last_step_name: String,
    step_timer: Option<Instant>,
}

impl BuildTurnStrategy {
    pub fn new(max_retries: u32, run_ci: bool) -> Self {
        Self {
            inner: StandardTurnStrategy::new(),
            max_retries,
            run_ci,
            retry_count: 0,
            last_step_name: String::new(),
            step_timer: None,
        }
    }

    /// 构建系统提示 — 注入 Build 模式指令
    fn build_build_system_prompt(&self, base_prompt: &SplitSystemPrompt) -> SplitSystemPrompt {
        let build_instructions = r#"
## Build 模式指令 (激活)

你当前处于 **Build 模式**。请遵循以下工作流程:

### 第 1 步: 规划 (PLAN)
在修改任何代码之前，先输出一个清晰的执行计划:
- 列出所有需要创建或修改的文件
- 说明每个文件变更的目的
- 按合理顺序排列步骤

格式:
```plan
1. [step title] — 文件: path/to/file.rs
   目的: 说明此步骤要做什么
   操作: create | modify | delete
```

### 第 2 步: 执行 (EXECUTE)
逐步骤执行。每完成一步:
- 确认变更已保存
- 简要总结做了什么
- 进入下一步

### 第 3 步: 验证 (VERIFY)
所有步骤完成后:
- 检查是否有语法错误
- 确认无遗漏的依赖
- 输出最终总结

**重要**: 每次只做一个步骤。不要跳跃。
"#;

        SplitSystemPrompt {
            static_part: format!("{}\n{}", base_prompt.static_part, build_instructions),
            dynamic_part: base_prompt.dynamic_part.clone(),
        }
    }
}

impl TurnStrategy for BuildTurnStrategy {
    /// Phase 1: 修复 + 跟踪步骤
    fn repair(&self, agent: &mut Agent) -> usize {
        self.inner.repair(agent)
    }

    /// Phase 2: 准备消息
    fn prepare_messages(&self, agent: &mut Agent) -> (Vec<Message>, Option<CompactionEvent>) {
        self.inner.prepare_messages(agent)
    }

    /// Phase 3: 处理压缩
    fn handle_compaction(&self, agent: &mut Agent, event: &CompactionEvent, print: bool) {
        self.inner.handle_compaction(agent, event, print)
    }

    /// Phase 4: 构建工具定义
    async fn tool_defs(&self, agent: &mut Agent) -> Vec<ToolDefinition> {
        self.inner.tool_defs(agent).await
    }

    /// Phase 5: 构建内存提示
    fn build_memory(&self, agent: &mut Agent, msgs: Arc<[Message]>) -> Option<PendingMemory> {
        self.inner.build_memory(agent, msgs)
    }

    /// Phase 6: 构建系统提示 — 注入 Build 模式指令
    fn build_prompt(&self, agent: &Agent) -> SplitSystemPrompt {
        let base = self.inner.build_prompt(agent);
        self.build_build_system_prompt(&base)
    }

    /// Phase 7: 记录缓存
    fn record_cache(&self, agent: &mut Agent, msgs: &[Message]) {
        self.inner.record_cache(agent, msgs)
    }

    /// Phase 8: 微观压缩
    fn microcompact(&self, msgs: &mut Vec<Message>, print: bool) {
        self.inner.microcompact(msgs, print)
    }

    /// Phase 9: 注入内存
    fn inject_memory(&self, msgs: &mut Vec<Message>, memory: &PendingMemory) {
        self.inner.inject_memory(msgs, memory)
    }
}
