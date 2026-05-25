//! Video export functionality stub
//!
//! Placeholder for video export features (to be implemented).

use anyhow::Result;

pub async fn export_swarm_video(
    _panes: &[Vec<String>],
    _speed: f32,
    _output_path: &std::path::Path,
    _cols: u16,
    _rows: u16,
) -> Result<()> {
    tracing::warn!("Video export (swarm) not yet implemented");
    Ok(())
}

pub async fn export_video(
    _session: &str,
    _timeline: &[u8],
    _speed: f32,
    _output_path: &std::path::Path,
    _cols: u16,
    _rows: u16,
    _fps: u8,
    _centered_override: Option<bool>,
) -> Result<()> {
    tracing::warn!("Video export not yet implemented");
    Ok(())
}
