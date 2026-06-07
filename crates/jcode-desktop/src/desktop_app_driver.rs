#![allow(dead_code)]

use crate::desktop_scene::DesktopScene;
use crate::session_launch;
use crate::workspace::KeyOutcome;
use serde::{Deserialize, Serialize};

pub(crate) const DESKTOP_UI_SNAPSHOT_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopUiSnapshot {
    pub(crate) version: u16,
    pub(crate) mode: String,
    pub(crate) title: String,
    pub(crate) live_session_id: Option<String>,
    pub(crate) surface: DesktopSurfaceSnapshot,
}

impl DesktopUiSnapshot {
    pub(crate) fn new(
        mode: impl Into<String>,
        title: String,
        live_session_id: Option<String>,
        surface: DesktopSurfaceSnapshot,
    ) -> Self {
        Self {
            version: DESKTOP_UI_SNAPSHOT_VERSION,
            mode: mode.into(),
            title,
            live_session_id,
            surface,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DesktopSurfaceSnapshot {
    SingleSession(DesktopSingleSessionSnapshot),
    Workspace(DesktopWorkspaceSnapshot),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopSingleSessionSnapshot {
    pub(crate) session_title: Option<String>,
    pub(crate) draft: String,
    pub(crate) draft_cursor: usize,
    pub(crate) body_scroll_millis: i32,
    pub(crate) detail_scroll: usize,
    pub(crate) show_help: bool,
    pub(crate) show_session_info: bool,
    pub(crate) pending_image_count: usize,
    pub(crate) model_picker_open: bool,
    pub(crate) session_switcher_open: bool,
    pub(crate) stdin_response_active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkspaceSnapshot {
    pub(crate) input_mode: String,
    pub(crate) focused_surface_id: u64,
    pub(crate) focused_session_id: Option<String>,
    pub(crate) zoomed: bool,
    pub(crate) detail_scroll: usize,
    pub(crate) draft: String,
    pub(crate) draft_cursor: usize,
    pub(crate) pending_image_count: usize,
    pub(crate) surfaces: Vec<DesktopWorkspaceSurfaceSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct DesktopWorkspaceSurfaceSnapshot {
    pub(crate) id: u64,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) session_id: Option<String>,
    pub(crate) lane: i32,
    pub(crate) column: i32,
    pub(crate) color_index: usize,
}

pub(crate) struct DesktopSceneBuildContext {
    pub(crate) scene: DesktopScene,
}

impl DesktopSceneBuildContext {
    pub(crate) fn new(scene: DesktopScene) -> Self {
        Self { scene }
    }
}

pub(crate) trait DesktopAppDriver {
    type KeyInput;
    type KeyOutcome;

    fn mode(&self) -> &'static str;
    fn status_title(&self) -> String;
    fn live_session_id(&self) -> Option<String>;
    fn has_background_work(&self) -> bool;
    fn has_frame_animation(&self) -> bool;
    fn handle_key_input(&mut self, key: Self::KeyInput) -> Self::KeyOutcome;
    fn apply_session_event(&mut self, event: session_launch::DesktopSessionEvent);
    fn build_scene(&self, context: DesktopSceneBuildContext) -> DesktopScene;
    fn snapshot(&self) -> DesktopUiSnapshot;
    fn restore_snapshot(
        &mut self,
        snapshot: DesktopUiSnapshot,
    ) -> Result<(), DesktopSnapshotRestoreError>;
}

pub(crate) struct DesktopAppRuntime<D: DesktopAppDriver> {
    driver: D,
}

impl<D: DesktopAppDriver> DesktopAppRuntime<D> {
    pub(crate) fn new(driver: D) -> Self {
        Self { driver }
    }

    pub(crate) fn driver(&self) -> &D {
        &self.driver
    }

    pub(crate) fn driver_mut(&mut self) -> &mut D {
        &mut self.driver
    }

    pub(crate) fn into_driver(self) -> D {
        self.driver
    }

    pub(crate) fn mode(&self) -> &'static str {
        self.driver.mode()
    }

    pub(crate) fn status_title(&self) -> String {
        self.driver.status_title()
    }

    pub(crate) fn live_session_id(&self) -> Option<String> {
        self.driver.live_session_id()
    }

    pub(crate) fn has_background_work(&self) -> bool {
        self.driver.has_background_work()
    }

    pub(crate) fn has_frame_animation(&self) -> bool {
        self.driver.has_frame_animation()
    }

    pub(crate) fn handle_key_input(&mut self, key: D::KeyInput) -> D::KeyOutcome {
        self.driver.handle_key_input(key)
    }

    pub(crate) fn apply_session_event(&mut self, event: session_launch::DesktopSessionEvent) {
        self.driver.apply_session_event(event);
    }

    pub(crate) fn build_scene(&self, scene: DesktopScene) -> DesktopScene {
        self.driver
            .build_scene(DesktopSceneBuildContext::new(scene))
    }

    pub(crate) fn snapshot(&self) -> DesktopUiSnapshot {
        self.driver.snapshot()
    }

    pub(crate) fn restore_snapshot(
        &mut self,
        snapshot: DesktopUiSnapshot,
    ) -> Result<(), DesktopSnapshotRestoreError> {
        self.driver.restore_snapshot(snapshot)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DesktopSnapshotRestoreError {
    UnsupportedVersion { version: u16 },
    UnsupportedMode { mode: String },
}

impl std::fmt::Display for DesktopSnapshotRestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported desktop snapshot version {version}")
            }
            Self::UnsupportedMode { mode } => {
                write!(formatter, "cannot restore desktop snapshot for mode {mode}")
            }
        }
    }
}

impl std::error::Error for DesktopSnapshotRestoreError {}

pub(crate) type DesktopKeyDriverOutcome = KeyOutcome;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeDriver {
        key_count: usize,
        restored: Option<DesktopUiSnapshot>,
    }

    impl DesktopAppDriver for FakeDriver {
        type KeyInput = char;
        type KeyOutcome = usize;

        fn mode(&self) -> &'static str {
            "fake"
        }

        fn status_title(&self) -> String {
            "Fake".to_string()
        }

        fn live_session_id(&self) -> Option<String> {
            Some("fake-session".to_string())
        }

        fn has_background_work(&self) -> bool {
            false
        }

        fn has_frame_animation(&self) -> bool {
            self.key_count > 0
        }

        fn handle_key_input(&mut self, _key: Self::KeyInput) -> Self::KeyOutcome {
            self.key_count += 1;
            self.key_count
        }

        fn apply_session_event(&mut self, _event: session_launch::DesktopSessionEvent) {}

        fn build_scene(&self, mut context: DesktopSceneBuildContext) -> DesktopScene {
            context.scene.metadata.title = Some(self.status_title());
            context.scene
        }

        fn snapshot(&self) -> DesktopUiSnapshot {
            fake_snapshot()
        }

        fn restore_snapshot(
            &mut self,
            snapshot: DesktopUiSnapshot,
        ) -> Result<(), DesktopSnapshotRestoreError> {
            self.restored = Some(snapshot);
            Ok(())
        }
    }

    fn fake_snapshot() -> DesktopUiSnapshot {
        DesktopUiSnapshot::new(
            "fake",
            "Fake".to_string(),
            Some("fake-session".to_string()),
            DesktopSurfaceSnapshot::SingleSession(DesktopSingleSessionSnapshot {
                session_title: None,
                draft: String::new(),
                draft_cursor: 0,
                body_scroll_millis: 0,
                detail_scroll: 0,
                show_help: false,
                show_session_info: false,
                pending_image_count: 0,
                model_picker_open: false,
                session_switcher_open: false,
                stdin_response_active: false,
            }),
        )
    }

    #[test]
    fn app_runtime_delegates_driver_operations() {
        let mut runtime = DesktopAppRuntime::new(FakeDriver::default());

        assert_eq!(runtime.mode(), "fake");
        assert_eq!(runtime.status_title(), "Fake");
        assert_eq!(runtime.live_session_id(), Some("fake-session".to_string()));
        assert!(!runtime.has_background_work());
        assert!(!runtime.has_frame_animation());
        assert_eq!(runtime.handle_key_input('x'), 1);
        assert!(runtime.has_frame_animation());

        let scene = runtime.build_scene(DesktopScene::default());
        assert_eq!(scene.metadata.title, Some("Fake".to_string()));

        let snapshot = runtime.snapshot();
        runtime
            .restore_snapshot(snapshot.clone())
            .expect("restore snapshot");
        assert_eq!(runtime.driver().restored, Some(snapshot));
        assert_eq!(runtime.into_driver().key_count, 1);
    }

    #[test]
    fn ui_snapshot_round_trips_single_session_surface() {
        let snapshot = DesktopUiSnapshot::new(
            "single_session",
            "Jcode".to_string(),
            Some("session-1".to_string()),
            DesktopSurfaceSnapshot::SingleSession(DesktopSingleSessionSnapshot {
                session_title: Some("active session".to_string()),
                draft: "hello".to_string(),
                draft_cursor: 5,
                body_scroll_millis: 1500,
                detail_scroll: 3,
                show_help: true,
                show_session_info: false,
                pending_image_count: 2,
                model_picker_open: true,
                session_switcher_open: false,
                stdin_response_active: true,
            }),
        );

        let encoded = serde_json::to_string(&snapshot).expect("serialize snapshot");
        let decoded: DesktopUiSnapshot =
            serde_json::from_str(&encoded).expect("deserialize snapshot");

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn ui_snapshot_round_trips_workspace_surface() {
        let snapshot = DesktopUiSnapshot::new(
            "workspace",
            "Workspace".to_string(),
            None,
            DesktopSurfaceSnapshot::Workspace(DesktopWorkspaceSnapshot {
                input_mode: "Normal".to_string(),
                focused_surface_id: 42,
                focused_session_id: Some("session-2".to_string()),
                zoomed: true,
                detail_scroll: 7,
                draft: "workspace draft".to_string(),
                draft_cursor: 9,
                pending_image_count: 1,
                surfaces: vec![DesktopWorkspaceSurfaceSnapshot {
                    id: 42,
                    kind: "Session".to_string(),
                    title: "worker".to_string(),
                    session_id: Some("session-2".to_string()),
                    lane: 1,
                    column: 2,
                    color_index: 3,
                }],
            }),
        );

        let encoded = serde_json::to_string(&snapshot).expect("serialize snapshot");
        let decoded: DesktopUiSnapshot =
            serde_json::from_str(&encoded).expect("deserialize snapshot");

        assert_eq!(decoded, snapshot);
    }
}
