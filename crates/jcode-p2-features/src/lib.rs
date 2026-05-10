// jcode-p2-features
// ════════════════════════════════════════════════════════════════
// P2 锦上添花功能集 — 10 项增强功能
//
// 模块列表:
//
//   1. repl.rs           — REPL 虚拟机 (安全代码执行沙箱)
//   2. notebook.rs       — Jupyter .ipynb 编辑器
//   3. workflow.rs       — 自定义工作流脚本引擎
//   4. mermaid.rs        — Mermaid 图表终端渲染
//   5. usage_overlay.rs  — Token/费用实时覆盖层
//   6. config_wizard.rs  — 交互式配置向导
//   7. powershell.rs    — Windows PowerShell 集成
//   8. kairos_file.rs    — KAIROS 文件传输通道
//   9. brief_mode.rs     — Brief 简要输出模式
//   10. notification.rs  — 多渠道通知系统
// ════════════════════════════════════════════════════════════════

pub mod repl;
pub mod notebook;
pub mod workflow;
pub mod mermaid;
pub mod usage_overlay;
pub mod config_wizard;
pub mod powershell;
pub mod kairos_file;
pub mod brief_mode;
pub mod notification;

// 重导出核心类型
pub use repl::{ReplExecutor, ReplResult, ReplLanguage, ReplConfig};
pub use notebook::{NotebookEditor, NotebookCell, CellType, Notebook};
pub use workflow::{WorkflowEngine, WorkflowDefinition, WorkflowStep, WorkflowResult, WorkflowContext};
pub use mermaid::{MermaidRenderer, MermaidDiagram, DiagramType};
pub use usage_overlay::{UsageOverlay, UsageStats, TokenUsage};
pub use config_wizard::{ConfigWizard, WizardStep, WizardResult};
pub use powershell::PowerShellBridge;
pub use kairos_file::KairosFileTransfer;
pub use brief_mode::{BriefFormatter, BriefOutput};
pub use notification::{NotificationDispatcher, NotificationMessage};
pub use notification::NotificationLevel;

/// 所有 P2 功能的统一初始化入口
pub struct P2FeatureSet {
    pub repl: ReplExecutor,
    pub notebook: NotebookEditor,
    pub workflow: WorkflowEngine,
    pub mermaid: MermaidRenderer,
    pub usage: UsageOverlay,
    pub config: ConfigWizard,
    pub notifications: NotificationDispatcher,
}

impl Default for P2FeatureSet {
    fn default() -> Self {
        Self {
            repl: ReplExecutor::new(ReplConfig::default()),
            notebook: NotebookEditor::new(Notebook::default()),
            workflow: WorkflowEngine::new(),
            mermaid: MermaidRenderer::new(),
            usage: UsageOverlay::new(),
            config: ConfigWizard::new(),
            notifications: NotificationDispatcher::new(),
        }
    }
}
