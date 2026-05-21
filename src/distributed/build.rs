fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile protobuf definitions for distributed cluster communication
    tonic_build::compile_protos("proto/distributed.proto")?;
    Ok(())
}
