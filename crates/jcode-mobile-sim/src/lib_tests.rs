#[cfg(test)]
mod tests {
    use crate::{AutomationRequest, request_status, run_server, send_request};
    use anyhow::{Result, anyhow};
    use jcode_mobile_core::ScenarioName;
    use serde_json::{Value, json};
    use std::path::Path;
    use tempfile::TempDir;

    #[cfg(unix)]
    async fn wait_for_socket(path: &Path) -> Result<()> {
        for _ in 0..100 {
            if path.exists() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Err(anyhow!("socket did not appear: {}", path.display()))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn automation_round_trip_over_socket() -> Result<()> {
        let dir = TempDir::new()?;
        let socket = dir.path().join("sim.sock");
        let server_socket = socket.clone();
        let server =
            tokio::spawn(async move { run_server(&server_socket, ScenarioName::Onboarding).await });
        wait_for_socket(&socket).await?;

        let status = request_status(&socket).await?;
        assert_eq!(status.screen, "onboarding");

        let _ = send_request(
            &socket,
            AutomationRequest {
                id: "set-host".to_string(),
                method: "dispatch".to_string(),
                params: json!({
                    "action": {
                        "type": "set_host",
                        "value": "devbox.tailnet.ts.net"
                    }
                }),
            },
        )
        .await?;

        let dispatch = send_request(
            &socket,
            AutomationRequest {
                id: "scenario".to_string(),
                method: "load_scenario".to_string(),
                params: json!({"scenario": "connected_chat"}),
            },
        )
        .await?;
        assert!(dispatch.ok);

        let tree = send_request(
            &socket,
            AutomationRequest {
                id: "tree".to_string(),
                method: "tree".to_string(),
                params: Value::Null,
            },
        )
        .await?;
        let tree_json = serde_json::to_string(&tree.result)?;
        assert!(tree_json.contains("chat.send"));

        let scene = send_request(
            &socket,
            AutomationRequest {
                id: "scene".to_string(),
                method: "scene".to_string(),
                params: Value::Null,
            },
        )
        .await?;
        assert!(scene.ok);
        assert_eq!(scene.result["schema_version"], 1);
        assert_eq!(scene.result["coordinate_space"], "logical_points_top_left");

        let preview_mesh = send_request(
            &socket,
            AutomationRequest {
                id: "preview-mesh".to_string(),
                method: "preview_mesh".to_string(),
                params: Value::Null,
            },
        )
        .await?;
        assert!(preview_mesh.ok);
        assert_eq!(preview_mesh.result["backend"], "wgpu-triangle-list-v1");
        assert!(
            preview_mesh.result["vertex_count"]
                .as_u64()
                .unwrap_or_default()
                > 500
        );

        let render = send_request(
            &socket,
            AutomationRequest {
                id: "render".to_string(),
                method: "render".to_string(),
                params: Value::Null,
            },
        )
        .await?;
        assert!(render.ok);
        assert!(
            render.result["output"]
                .as_str()
                .unwrap_or_default()
                .contains("chat.send [Button]")
        );

        let screenshot = send_request(
            &socket,
            AutomationRequest {
                id: "screenshot".to_string(),
                method: "screenshot".to_string(),
                params: Value::Null,
            },
        )
        .await?;
        assert!(screenshot.ok);
        assert!(
            screenshot.result["svg"]
                .as_str()
                .unwrap_or_default()
                .contains("chat.send")
        );

        let assert_screenshot = send_request(
            &socket,
            AutomationRequest {
                id: "assert-screenshot".to_string(),
                method: "assert_screenshot".to_string(),
                params: json!({"snapshot": screenshot.result}),
            },
        )
        .await?;
        assert!(assert_screenshot.ok);

        let assert_screen = send_request(
            &socket,
            AutomationRequest {
                id: "assert-screen".to_string(),
                method: "assert_screen".to_string(),
                params: json!({"screen": "chat"}),
            },
        )
        .await?;
        assert!(assert_screen.ok);

        let find_node = send_request(
            &socket,
            AutomationRequest {
                id: "find-node".to_string(),
                method: "find_node".to_string(),
                params: json!({"node_id": "chat.send"}),
            },
        )
        .await?;
        assert!(find_node.ok);

        let assert_node = send_request(
            &socket,
            AutomationRequest {
                id: "assert-node".to_string(),
                method: "assert_node".to_string(),
                params: json!({"node_id": "chat.send", "enabled": true, "role": "button"}),
            },
        )
        .await?;
        assert!(assert_node.ok);

        let assert_hit = send_request(
            &socket,
            AutomationRequest {
                id: "assert-hit".to_string(),
                method: "assert_hit".to_string(),
                params: json!({"x": 330, "y": 788, "node_id": "chat.send"}),
            },
        )
        .await?;
        assert!(assert_hit.ok);

        let assert_text = send_request(
            &socket,
            AutomationRequest {
                id: "assert-text".to_string(),
                method: "assert_text".to_string(),
                params: json!({"contains": "Connected to simulated jcode server."}),
            },
        )
        .await?;
        assert!(assert_text.ok);

        let assert_no_error = send_request(
            &socket,
            AutomationRequest {
                id: "assert-no-error".to_string(),
                method: "assert_no_error".to_string(),
                params: Value::Null,
            },
        )
        .await?;
        assert!(assert_no_error.ok);

        let wait = send_request(
            &socket,
            AutomationRequest {
                id: "wait".to_string(),
                method: "wait".to_string(),
                params: json!({"screen": "chat", "node_id": "chat.send", "timeout_ms": 50}),
            },
        )
        .await?;
        assert!(wait.ok);

        let scroll = send_request(
            &socket,
            AutomationRequest {
                id: "scroll".to_string(),
                method: "scroll".to_string(),
                params: json!({"node_id": "chat.messages", "delta_y": 120}),
            },
        )
        .await?;
        assert!(scroll.ok);

        let gesture = send_request(
            &socket,
            AutomationRequest {
                id: "gesture".to_string(),
                method: "gesture".to_string(),
                params: json!({"type": "swipe_up"}),
            },
        )
        .await?;
        assert!(gesture.ok);

        let type_text = send_request(
            &socket,
            AutomationRequest {
                id: "type-text".to_string(),
                method: "type_text".to_string(),
                params: json!({"node_id": "chat.draft", "text": "typed protocol"}),
            },
        )
        .await?;
        assert!(type_text.ok);

        let keypress = send_request(
            &socket,
            AutomationRequest {
                id: "keypress".to_string(),
                method: "keypress".to_string(),
                params: json!({"node_id": "chat.draft", "key": "Enter"}),
            },
        )
        .await?;
        assert!(keypress.ok);

        let assert_typed_response = send_request(
            &socket,
            AutomationRequest {
                id: "assert-typed-response".to_string(),
                method: "assert_text".to_string(),
                params: json!({"contains": "Simulated response to: typed protocol"}),
            },
        )
        .await?;
        assert!(assert_typed_response.ok);

        let set_draft = send_request(
            &socket,
            AutomationRequest {
                id: "set-draft".to_string(),
                method: "dispatch".to_string(),
                params: json!({"action": {"type": "set_draft", "value": "hello simulator"}}),
            },
        )
        .await?;
        assert!(set_draft.ok);

        let send_message = send_request(
            &socket,
            AutomationRequest {
                id: "send-message".to_string(),
                method: "dispatch".to_string(),
                params: json!({"action": {"type": "tap_node", "node_id": "chat.send"}}),
            },
        )
        .await?;
        assert!(send_message.ok);

        let assert_transition = send_request(
            &socket,
            AutomationRequest {
                id: "assert-transition".to_string(),
                method: "assert_transition".to_string(),
                params: json!({"type": "load_scenario", "contains": "connected_chat"}),
            },
        )
        .await?;
        assert!(assert_transition.ok);

        let assert_effect = send_request(
            &socket,
            AutomationRequest {
                id: "assert-effect".to_string(),
                method: "assert_effect".to_string(),
                params: json!({"type": "send_message", "contains": "hello simulator"}),
            },
        )
        .await?;
        assert!(assert_effect.ok);

        let replay = send_request(
            &socket,
            AutomationRequest {
                id: "replay".to_string(),
                method: "replay".to_string(),
                params: json!({"name": "automation-round-trip"}),
            },
        )
        .await?;
        assert!(replay.ok);
        assert_eq!(replay.result["name"], "automation-round-trip");
        let actions = replay.result["actions"].as_array().map_or(0, Vec::len);
        assert!(actions >= 3, "replay includes top-level actions");
        let assert_replay = send_request(
            &socket,
            AutomationRequest {
                id: "assert-replay".to_string(),
                method: "assert_replay".to_string(),
                params: json!({"trace": replay.result}),
            },
        )
        .await?;
        assert!(assert_replay.ok);

        let inject_fault = send_request(
            &socket,
            AutomationRequest {
                id: "inject-fault".to_string(),
                method: "inject_fault".to_string(),
                params: json!({"kind": "tool_failed"}),
            },
        )
        .await?;
        assert!(inject_fault.ok);

        let assert_fault_text = send_request(
            &socket,
            AutomationRequest {
                id: "assert-fault-text".to_string(),
                method: "assert_text".to_string(),
                params: json!({"contains": "Last simulated tool failed."}),
            },
        )
        .await?;
        assert!(assert_fault_text.ok);

        let _ = send_request(
            &socket,
            AutomationRequest {
                id: "shutdown".to_string(),
                method: "shutdown".to_string(),
                params: Value::Null,
            },
        )
        .await?;

        server.await??;
        Ok(())
    }
}
