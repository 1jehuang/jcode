//! Public widget snapshots remain constructible by downstream TUI clients.
use jcode_tui::tui::info_widget::{
    CloudMachineStats, CloudProbeState, CloudStatusInfo, InfoWidgetData, WidgetKind,
};

#[test]
fn downstream_can_construct_cloud_widget_data() {
    let now = std::time::SystemTime::now();
    let data = InfoWidgetData {
        cloud_info: Some(CloudStatusInfo {
            host: "cloud".into(),
            remote_dir: "/workspace".into(),
            local_dir: None,
            local_is_source: false,
            started_at: now,
            last_success: None,
            probe_duration_ms: None,
            state: CloudProbeState::Loading,
            machine: Some(CloudMachineStats::default()),
            last_transfer_at: None,
        }),
        cloud_connection: Some("Connecting".into()),
        cloud_working: false,
        ..Default::default()
    };
    assert!(data.has_data_for(WidgetKind::CloudVm));
}
