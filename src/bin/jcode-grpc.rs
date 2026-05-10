
use std::net::SocketAddr;
use jcode::grpc::GrpcServerBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 简单启动，先不使用 tracing
    let addr: SocketAddr = "0.0.0.0:50051".parse()?;
    println!("Starting jcode gRPC server on {}", addr);

    let builder = GrpcServerBuilder::new();
    builder.serve(addr).await?;

    Ok(())
}
