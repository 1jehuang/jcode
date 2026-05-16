//! NLP (自然语言处理) 能力模块
//!
//! 提供代码分析与自然语言处理能力，包括意图识别、实体提取、
//! 代码骨架生成、架构文档生成、迁移计划生成等功能。

pub mod types;
pub mod engine;
pub mod skeletons;
pub mod docs;
pub mod helpers;

pub use types::*;
pub use engine::*;
pub use skeletons::*;
pub use docs::*;
pub use helpers::*;
