//! Agent Loop - Core ReAct loop engine
//!
//! TODO: Implement full agent loop logic
//! Currently providing stub types for compilation
//! Core types (LoopEvent, TerminalState, AgentLoopConfig) are defined in types.rs

use std::marker::PhantomData;

use crate::types::{AgentLoopConfig, LoopEvent, TerminalState};

/// The core agent loop
pub struct AgentLoop<T> {
    _marker: PhantomData<T>,
}

impl<T> AgentLoop<T> {
    pub fn new(_config: AgentLoopConfig) -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}
