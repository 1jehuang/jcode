use std::net::SocketAddr;
use jcode::{
    grpc::GrpcServerBuilder,
    ws::WebSocketServer,
    rest::RestServer,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let grpc_port: u16 = 50051;
    let ws_port: u16 = 8080;
    let rest_port: u16 = 8081;

    println!("🚀 Starting jcode Multi-Protocol Server");
    println!("====================================");
    println!("gRPC:      0.0.0.0:{}", grpc_port);
    println!("WebSocket: 0.0.0.0:{}", ws_port);
    println!("REST:      0.0.0.0:{}", rest_port);
    println!("====================================");

    let grpc_addr: SocketAddr = format!("0.0.0.0:{}", grpc_port).parse()?;
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