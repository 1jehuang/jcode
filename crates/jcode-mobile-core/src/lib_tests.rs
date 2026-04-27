#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_flow_reaches_connected_chat() {
        let mut store = SimulatorStore::default();
        store.dispatch(SimulatorAction::SetHost {
            value: "devbox.tailnet.ts.net".to_string(),
        });
        store.dispatch(SimulatorAction::SetPairCode {
            value: "123456".to_string(),
        });
        let report = store.dispatch(SimulatorAction::TapNode {
            node_id: "pair.submit".to_string(),
        });

        assert!(!report.transitions.is_empty());
        assert_eq!(store.state().connection_state, ConnectionState::Connected);
        assert_eq!(store.state().screen, Screen::Chat);
    }

    #[test]
    fn sending_message_creates_assistant_reply() {
        let mut store =
            SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        store.dispatch(SimulatorAction::SetDraft {
            value: "hello simulator".to_string(),
        });
        store.dispatch(SimulatorAction::TapNode {
            node_id: "chat.send".to_string(),
        });

        let last = store.state().messages.last();
        assert!(last.is_some(), "assistant reply present");
        let Some(last) = last else {
            return;
        };
        assert_eq!(last.role, MessageRole::Assistant);
        assert!(last.text.contains("hello simulator"));
        assert!(!store.state().is_processing);
    }

    #[test]
    fn semantic_tree_reflects_current_screen() {
        let store = SimulatorStore::default();
        let tree = store.semantic_tree();
        assert_eq!(tree.screen, Screen::Onboarding);
        assert!(
            tree.root
                .children
                .iter()
                .any(|node| node.id == "pair.submit")
        );
    }

    #[test]
    fn semantic_tree_exposes_agent_metadata() {
        let store = SimulatorStore::default();
        let tree = store.semantic_tree();

        let pair_submit = tree
            .root
            .children
            .iter()
            .find(|node| node.id == "pair.submit");
        assert!(pair_submit.is_some(), "pair submit node");
        let Some(pair_submit) = pair_submit else {
            return;
        };
        assert_eq!(
            pair_submit.accessibility_label.as_deref(),
            Some("Pair & Connect")
        );
        assert!(pair_submit.supported_actions.contains(&UiNodeAction::Tap));

        let pair_host = tree
            .root
            .children
            .iter()
            .find(|node| node.id == "pair.host");
        assert!(pair_host.is_some(), "pair host node");
        let Some(pair_host) = pair_host else {
            return;
        };
        assert!(pair_host.supported_actions.contains(&UiNodeAction::SetText));
        assert!(
            pair_host
                .supported_actions
                .contains(&UiNodeAction::TypeText)
        );
    }

    #[test]
    fn all_scenarios_parse_round_trip() {
        for scenario in ScenarioName::ALL {
            assert_eq!(ScenarioName::parse(scenario.as_str()), Some(*scenario));
        }
    }

    #[test]
    fn scenario_fixtures_cover_error_processing_and_offline_states() {
        let invalid = SimulatorState::for_scenario(ScenarioName::PairingInvalidCode);
        assert!(
            invalid
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("Invalid")
        );

        let streaming = SimulatorState::for_scenario(ScenarioName::ChatStreaming);
        assert!(streaming.is_processing);
        assert_eq!(streaming.screen, Screen::Chat);

        let offline = SimulatorState::for_scenario(ScenarioName::OfflineQueuedMessage);
        assert_eq!(offline.connection_state, ConnectionState::Disconnected);
        assert!(offline.draft_message.contains("Queued"));
    }

    #[test]
    fn fake_backend_rejects_invalid_pairing_code() {
        let mut store =
            SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        store.dispatch(SimulatorAction::SetPairCode {
            value: "000000".to_string(),
        });
        store.dispatch(SimulatorAction::TapNode {
            node_id: "pair.submit".to_string(),
        });

        assert_eq!(
            store.state().connection_state,
            ConnectionState::Disconnected
        );
        assert!(
            store
                .state()
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("Invalid")
        );
    }

    #[test]
    fn fake_backend_reports_unreachable_host() {
        let mut store =
            SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        store.dispatch(SimulatorAction::SetHost {
            value: "offline.tailnet.ts.net".to_string(),
        });
        store.dispatch(SimulatorAction::TapNode {
            node_id: "pair.submit".to_string(),
        });

        assert_eq!(
            store.state().connection_state,
            ConnectionState::Disconnected
        );
        assert!(
            store
                .state()
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("unreachable")
        );
    }

    #[test]
    fn replay_trace_records_and_replays_deterministically() -> anyhow::Result<()> {
        let actions = vec![
            SimulatorAction::TapNode {
                node_id: "pair.submit".to_string(),
            },
            SimulatorAction::SetDraft {
                value: "hello replay".to_string(),
            },
            SimulatorAction::TapNode {
                node_id: "chat.send".to_string(),
            },
        ];
        let trace = ReplayTrace::record(
            "pairing-ready-chat-send",
            SimulatorState::for_scenario(ScenarioName::PairingReady),
            actions,
        );
        trace.assert_replays()?;
        assert_eq!(trace.actions.len(), 3);
        assert_eq!(trace.transitions.len(), 7);
        assert_eq!(trace.effects.len(), 2);
        assert_eq!(trace.final_state.screen, Screen::Chat);
        assert!(
            trace
                .final_state
                .messages
                .iter()
                .any(|message| message.text.contains("hello replay"))
        );
        Ok(())
    }

    #[test]
    fn golden_replay_trace_matches_core_behavior() -> anyhow::Result<()> {
        let golden = include_str!("../tests/golden/pairing_ready_chat_send.json");
        let trace: ReplayTrace = serde_json::from_str(golden)?;
        trace.assert_replays()?;
        Ok(())
    }

    #[test]
    fn layout_bounds_support_hit_testing() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        let tree = store.semantic_tree();
        let submit = tree
            .root
            .children
            .iter()
            .find(|node| node.id == "pair.submit");
        assert!(submit.is_some(), "pair.submit node");
        let Some(submit) = submit else {
            return;
        };
        assert!(submit.bounds.is_some(), "pair.submit bounds");
        let Some(bounds) = submit.bounds else {
            return;
        };
        let (x, y) = bounds.center();
        assert_eq!(
            hit_test(&tree, x, y).map(|node| node.id.as_str()),
            Some("pair.submit")
        );
        assert_eq!(
            hit_test_actionable(&tree, x, y, UiNodeAction::Tap).map(|node| node.id.as_str()),
            Some("pair.submit")
        );
    }

    #[test]
    fn chat_layout_hit_tests_send_button() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let tree = store.semantic_tree();
        assert_eq!(
            hit_test_actionable(&tree, 330, 788, UiNodeAction::Tap).map(|node| node.id.as_str()),
            Some("chat.send")
        );
    }

    #[test]
    fn screenshot_snapshot_is_deterministic_svg_with_layout() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let tree = store.semantic_tree();
        let first = screenshot_snapshot(&tree);
        let second = screenshot_snapshot(&tree);

        assert_eq!(first, second);
        assert_eq!(first.width, DEFAULT_VIEWPORT_WIDTH);
        assert_eq!(first.height, DEFAULT_VIEWPORT_HEIGHT);
        assert!(first.hash.starts_with("fnv1a64:"));
        assert!(first.svg.contains("data-node=\"chat.send\""));
        assert_eq!(
            first.scene.as_ref().map(|scene| scene.schema_version),
            Some(1)
        );
        assert!(first.layout.root.bounds.is_some());
    }

    #[test]
    fn visual_scene_is_rust_owned_backend_contract() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let scene = store.visual_scene();

        assert_eq!(scene.schema_version, VISUAL_SCENE_SCHEMA_VERSION);
        assert_eq!(scene.coordinate_space, "logical_points_top_left");
        assert_eq!(scene.viewport.width, DEFAULT_VIEWPORT_WIDTH);
        assert_eq!(scene.viewport.height, DEFAULT_VIEWPORT_HEIGHT);
        assert!(scene.layers.iter().any(|layer| layer.id == "background"));
        assert!(scene.layers.iter().any(|layer| layer.id == "chrome"));
        assert!(scene.layers.iter().any(|layer| layer.id == "content"));

        let content = scene.layers.iter().find(|layer| layer.id == "content");
        assert!(content.is_some(), "content layer");
        let Some(content) = content else {
            return;
        };
        assert!(content.primitives.iter().any(|primitive| matches!(
            primitive,
            VisualPrimitive::Rect(rect)
                if rect.semantic_node_id.as_deref() == Some("chat.send")
                    && rect.bounds.x == DEFAULT_VIEWPORT_WIDTH - 110
        )));
        assert!(content.primitives.iter().any(|primitive| matches!(
            primitive,
            VisualPrimitive::Text(text)
                if text.semantic_node_id.as_deref() == Some("message.0")
                    && text.text.contains("summarize")
        )));
    }

    #[test]
    fn svg_backend_renders_from_visual_scene() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::PairingReady));
        let scene = store.visual_scene();
        let svg = render_scene_svg(&scene);

        assert!(svg.contains("data-layer=\"background\""));
        assert!(svg.contains("data-layer=\"chrome\""));
        assert!(svg.contains("data-layer=\"content\""));
        assert!(svg.contains("data-primitive=\"pair.submit.rect\""));
        assert!(svg.contains("data-node=\"pair.submit\""));
        assert!(svg.contains("Pair &amp; Connect"));
    }

    #[test]
    fn screenshot_diff_reports_mismatch() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let mut expected = screenshot_snapshot(&store.semantic_tree());
        let actual = expected.clone();
        expected.svg.push_str("<!-- changed -->");
        expected.hash = "fnv1a64:changed".to_string();

        let diff = diff_screenshots(&expected, &actual);
        assert!(!diff.matches);
        assert!(diff.first_difference.is_some());
    }

    #[test]
    fn text_render_exposes_human_readable_layout() {
        let store = SimulatorStore::new(SimulatorState::for_scenario(ScenarioName::ConnectedChat));
        let text = render_text(&store.semantic_tree());

        assert!(text.contains("jcode mobile simulator"));
        assert!(text.contains("screen: Chat"));
        assert!(text.contains("chat.send [Button]"));
        assert!(text.contains("@280,766 94x44"));
    }
}
