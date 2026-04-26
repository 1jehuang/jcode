use super::*;
use crate::transport::WriteHalf;
use anyhow::{Result, anyhow};

fn setup_runtime_dir() -> Result<(tempfile::TempDir, Option<std::ffi::OsString>)> {
    let runtime = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    Ok((runtime, prev_runtime))
}

fn restore_runtime_dir(prev_runtime: Option<std::ffi::OsString>) {
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

fn test_writer() -> Result<(Arc<Mutex<WriteHalf>>, crate::transport::Stream)> {
    let (stream_a, stream_b) = crate::transport::stream_pair().map_err(|e| anyhow!(e))?;
    let (_reader, writer_half) = stream_a.into_split();
    Ok((Arc::new(Mutex::new(writer_half)), stream_b))
}

include!("resume/multiple_live_attach.rs");
include!("resume/busy_existing_attach.rs");
include!("resume/reconnect_takeover_with_history.rs");
include!("resume/attach_without_local_history.rs");
include!("resume/different_client_attach.rs");
include!("resume/live_events_before_history.rs");
include!("resume/same_client_takeover.rs");
