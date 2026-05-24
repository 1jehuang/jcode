fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("../carpai-server/src/grpc/proto/agent.proto")?;
    tonic_build::compile_protos("../carpai-server/src/grpc/proto/session.proto")?;
    tonic_build::compile_protos("../carpai-server/src/grpc/proto/health.proto")?;
    Ok(())
}
