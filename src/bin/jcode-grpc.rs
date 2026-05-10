
use std::net::SocketAddr;
use jcode::grpc::GrpcServerBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 从环境变量读取端口配置，默认 50051
    let grpc_port: u16 = std::env::var("JCODE_GRPC_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50051);

    // 绑定地址可配置，默认 0.0.0.0
    let bind_addr = std::env::var("JCODE_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    let addr: SocketAddr = format!("{}:{}", bind_addr, grpc_port)
        .parse()
        .map_err(|e| format!("Invalid gRPC bind address: {}", e))?;

    println!("Starting jcode gRPC server on {}:{}", bind_addr, grpc_port);

    let builder = GrpcServerBuilder::new();
    builder.serve(addr).await?;

    Ok(())
}
