fn main() -> anyhow::Result<()> {
    let repo = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let p = jcode_build_support::publish_local_current_build(&repo)?;
    println!("published: {}", p.display());
    Ok(())
}
