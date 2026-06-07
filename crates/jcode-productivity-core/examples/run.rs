fn main() -> anyhow::Result<()> {
    let t = std::time::Instant::now();
    let out = jcode_productivity_core::generate()?;
    eprintln!("--- generate() took {:.2}s ---", t.elapsed().as_secs_f64());
    eprintln!("PNG: {} ({} bytes)", out.png_path.display(), out.png.len());
    eprintln!(
        "scanned={} parse_errors={} cache_hits={} scan_secs={:.2}",
        out.report.scanned_files,
        out.report.parse_errors,
        out.report.cache_hits,
        out.report.scan_secs
    );
    println!("{}", out.markdown);
    Ok(())
}
