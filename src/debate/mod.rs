//! Debate multi-perspectiva coordinator
//!
//! Arquitectura: El Coordinator recibe "requests de tools" de las perspectivas,
//! las ejecuta con un ToolContext valido, y distribuye los resultados a todas
//! las perspectivas para comentario.
//!
//! Flujo:
//!   Perspectiva -> [tool_request] -> Coordinator -> [execute_tool] -> Registry
//!   Registry -> [ToolOutput] -> Coordinator -> [broadcast] -> Todas perspectivas

mod coordinator;
mod perspective_agent;
mod types;

pub use coordinator::*;
pub use perspective_agent::*;
pub use types::*;
