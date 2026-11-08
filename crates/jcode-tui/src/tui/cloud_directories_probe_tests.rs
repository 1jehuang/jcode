//! Synthetic-only regression harness for the embedded directory probe.

#[test]
fn cloud_directories_probe_metadata_regressions() {
    let dir = tempfile::tempdir().expect("synthetic probe directory");
    let probe = dir.path().join("cloud_directories_probe.py");
    let tests = dir.path().join("cloud_directories_probe_test.py");
    std::fs::write(&probe, include_str!("cloud_directories_probe.py"))
        .expect("write synthetic probe copy");
    std::fs::write(&tests, include_str!("cloud_directories_probe_test.py"))
        .expect("write synthetic probe tests");
    let output = std::process::Command::new("python3")
        .arg(&tests)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 is required for directory probe regression tests");
    assert!(
        output.status.success(),
        "directory probe regressions failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
