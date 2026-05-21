//! UI/UX命令模块

pub mod theme;
pub mod vim;
pub mod plan;
pub mod effort;
pub mod fast;
pub mod passes;

pub use theme::ThemeCommand;
pub use vim::VimCommand;
pub use plan::PlanCommand;
pub use effort::EffortCommand;
pub use fast::FastCommand;
pub use passes::PassesCommand;
