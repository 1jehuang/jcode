fn main() {
    println!("cargo:rustc-check-cfg=cfg(mmdr_size_api_available)");
    // The pinned mermaid-rs-renderer tag (v0.3.0+) ships the render size
    // metadata API, so the size-API path is available by default. Set
    // KCODE_MMDR_SIZE_API_DISABLE=1 to force the legacy SVG-retarget path
    // (e.g. when testing against an older renderer via a Cargo patch).
    println!("cargo:rerun-if-env-changed=KCODE_MMDR_SIZE_API_DISABLE");
    // Legacy opt-in env var kept for compatibility with old build scripts.
    println!("cargo:rerun-if-env-changed=KCODE_MMDR_SIZE_API_AVAILABLE");
    if std::env::var_os("KCODE_MMDR_SIZE_API_DISABLE").is_none() {
        println!("cargo:rustc-cfg=mmdr_size_api_available");
    }
}
