//! Explicitly opt-in production smoke probe. Every event is marked CI and uses
//! a disposable installation ID, so it never enters user concurrency reports.
//! Run only after deploying the concurrency-aware telemetry worker.
use jcode_telemetry_core::begin_concurrency_session;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() != Some("--emit-ci-telemetry") {
        return Err("pass --emit-ci-telemetry to send isolated CI probe events".into());
    }
    let home = tempfile::tempdir()?;
    // This is a single-threaded probe until the first guard starts delivery.
    unsafe {
        std::env::set_var("JCODE_HOME", home.path());
        std::env::set_var("JCODE_CI", "1");
        std::env::set_var("JCODE_NO_TELEMETRY", "0");
        std::env::set_var("DO_NOT_TRACK", "0");
    }
    let mut first = begin_concurrency_session("concurrency-probe-root-a", None);
    let mut second = begin_concurrency_session("concurrency-probe-root-b", None);
    let mut child =
        begin_concurrency_session("concurrency-probe-child", Some("concurrency-probe-root-a"));
    child.finish();
    second.finish();
    first.finish();
    let mut fresh = begin_concurrency_session("concurrency-probe-after-close", None);
    fresh.finish();
    println!(
        "installation_id={}",
        std::fs::read_to_string(home.path().join("telemetry_id"))?.trim()
    );
    println!("Expected four end peaks: root-a=3, root-b=3, child=3, after-close=1.");
    println!(
        "Verify remote D1 delivery and is_ci=1. Successful local execution alone is not delivery proof."
    );
    // Allow background starts to finish too. Ends use bounded lifecycle delivery.
    std::thread::sleep(std::time::Duration::from_secs(3));
    Ok(())
}
