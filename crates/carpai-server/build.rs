fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile protobuf files and generate Rust code
    tonic_build::compile_protos("src/grpc/proto/agent.proto")?;
    tonic_build::compile_protos("src/grpc/proto/session.proto")?;
    tonic_build::compile_protos("src/grpc/proto/tool.proto")?;
    tonic_build::compile_protos("src/grpc/proto/health.proto")?;

    Ok(())
}
