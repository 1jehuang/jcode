fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_path = "../../proto";
    
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(
            &["../../proto/jcode.proto"],
            &[proto_path],
        )?;
    
    println!("cargo:rerun-if-changed=../../proto/jcode.proto");
    
    Ok(())
}
