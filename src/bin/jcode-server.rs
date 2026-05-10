use std::net::SocketAddr;
use jcode::{
    grpc::GrpcServerBuilder,
    ws::WebSocketServer,
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

    println!("🚀 Starting jcode Multi-Protocol Server");
    println!("====================================");
    println!("gRPC:      {}:{}", bind_addr, grpc_port);
    println!("WebSocket: {}:{}", bind_addr, ws_port);
    println!("REST:      {}:{}", bind_addr, rest_port);
    println!("====================================");

    let grpc_addr: SocketAddr = format!("{}:{}", bind_addr, grpc_port)
        .parse()
        .map_err(|e| format!("Invalid gRPC bind address: {}", e))?;
    let grpc_builder = GrpcServerBuilder::new();

    let ws_server = WebSocketServer::new(ws_port);
    let rest_server = RestServer::new(rest_port);

    tokio::spawn(async move {
        if let Err(e) = grpc_builder.serve(grpc_addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    tokio::spawn(async move {
        if let Err(e) = ws_server.serve().await {
            eprintln!("WebSocket server error: {}", e);
        }
    });

    rest_server.serve().await?;

    Ok(())
}
