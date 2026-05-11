pub mod server;
pub mod web_ide;
pub mod protocol;
pub mod session;
pub mod handlers;

// 注意: lsp_bridge, terminal, file_system, collaboration 这些模块
// 目前未实现（作为占位符），如果需要可以后续添加

pub use server::{WebSocketServer, WsRequest, WsResponse};
pub use web_ide::{WebIdeWebSocketServer, WebSocketConfig};
