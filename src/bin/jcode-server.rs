use std::net::SocketAddr;
use jcode::{
    grpc::GrpcServerBuilder,
    ws::{WebIdeWebSocketServer, WebSocketConfig},
    rest::RestServer,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 从环境变量读取端口配置，带合理的默认值
    let grpc_port: u16 = std::env::var("JCODE_GRPC_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50051);
    let ws_port: u16 = std::env::var("JCODE_WS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);
    let rest_port: u16 = std::env::var("JCODE_REST_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8081);

    // 绑定地址可配置，默认 0.0.0.0
    let bind_addr = std::env::var("JCODE_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    // Web IDE 功能开关（通过环境变量配置）
    let enable_lsp: bool = std::env::var("JCODE_ENABLE_LSP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true);
    
    let enable_terminal: bool = std::env::var("JCODE_ENABLE_TERMINAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true);
    
    let enable_collaboration: bool = std::env::var("JCODE_ENABLE_COLLABORATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(true);

    println!("🚀 Starting JCode Multi-Protocol Server");
    println!("=====================================");
    println!("gRPC:      {}:{}", bind_addr, grpc_port);
    println!("WebSocket: {}:{}", bind_addr, ws_port);
    println!("REST:      {}:{}", bind_addr, rest_port);
    println!();
    println!("🌐 Web IDE Features:");
    if enable_lsp {
        println!("   ✅ LSP Integration (code completion, diagnostics)");
    }
    if enable_terminal {
        println!("   ✅ Terminal Sessions (shell access)");
    }
    if enable_collaboration {
        println!("   ✅ Real-time Collaboration Editing");
    }
    println!("=====================================");

    let grpc_addr: SocketAddr = format!("{}:{}", bind_addr, grpc_port)
        .parse()
        .map_err(|e| format!("Invalid gRPC bind address: {}", e))?;
    let grpc_builder = GrpcServerBuilder::new();

    // 使用新的 Web IDE WebSocket 服务器
    let web_ide_config = WebSocketConfig {
        port: ws_port,
        enable_lsp,
        enable_terminal,
        enable_collaboration,
        ..Default::default()
    };
    let ws_server = WebIdeWebSocketServer::new(web_ide_config);
    
    let rest_server = RestServer::new(rest_port);

    tokio::spawn(async move {
        if let Err(e) = grpc_builder.serve(grpc_addr).await {
            eprintln!("❌ gRPC server error: {}", e);
        }
    });

    tokio::spawn(async move {
        if let Err(e) = ws_server.serve().await {
            eprintln!("❌ WebSocket server error: {}", e);
        }
    });

    rest_server.serve().await?;

    Ok(())
}
