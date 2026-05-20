fn main() {
    println!("cargo:rustc-check-cfg=cfg(mmdr_size_api_available)");
    println!("cargo:rerun-if-env-changed=JCODE_MMDR_SIZE_API_AVAILABLE");
    if std::env::var_os("JCODE_MMDR_SIZE_API_AVAILABLE").is_some() {
        println!("cargo:rustc-cfg=mmdr_size_api_available");
    }
}
