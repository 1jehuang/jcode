use super::*;

impl App {
    pub(super) fn note_runtime_memory_event(&mut self, category: &str, reason: &str) {
        self.note_runtime_memory_event_impl(category, reason, false);
    }

    pub(super) fn note_runtime_memory_event_force(&mut self, category: &str, reason: &str) {
        self.note_runtime_memory_event_impl(category, reason, true);
    }

    fn note_runtime_memory_event_impl(&mut self, category: &str, reason: &str, force: bool) {
        let Some(mut controller) = self.runtime_memory_log.take() else {
            return;
        };

        let event = if force {
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(category, reason)
                .with_session_id(self.session.id.clone())
                .force_attribution()
        } else {
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(category, reason)
                .with_session_id(self.session.id.clone())
        };

        let now = Instant::now();
        let should_write_process = controller.should_write_process_for_event(now, &event);
        let process_sample = if should_write_process {
            let sample = self.capture_runtime_memory_process_sample(
                &format!("process:event:{}", event.category),
                crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                    category: event.category.clone(),
                    reason: event.reason.clone(),
                    session_id: event.session_id.clone(),
                    detail: event.detail.clone(),
                },
                controller.build_sampling_for_process(Some(&event)),
            );
            controller.record_process_sample(now);
            Some(sample)
        } else {
            None
        };

        if let Some(sample) = process_sample.as_ref() {
            self.append_runtime_memory_sample(sample);
        }

        let mut wrote_attribution = false;
        let preflight_sample = if process_sample.is_none() && controller.can_write_attribution(now)
        {
            Some(self.capture_runtime_memory_process_sample(
                &format!("process:event-preflight:{}", event.category),
                crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                    category: event.category.clone(),
                    reason: "preflight".to_string(),
                    session_id: event.session_id.clone(),
                    detail: event.detail.clone(),
                },
                crate::runtime_memory_log::RuntimeMemoryLogSampling::default(),
            ))
        } else {
            None
        };
        let preflight = process_sample.as_ref().or(preflight_sample.as_ref());
        if let Some(preflight) = preflight
            && let Some(sampling) = controller.build_sampling_for_attribution(
                now,
                &preflight.process,
                Some(&event),
                None,
            )
        {
            let mut sample = self.capture_runtime_memory_attribution_sample(
                &format!("attribution:event:{}", event.category),
                crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                    category: event.category.clone(),
                    reason: event.reason.clone(),
                    session_id: event.session_id.clone(),
                    detail: event.detail.clone(),
                },
                sampling,
            );
            controller.finalize_attribution_totals(
                now,
                sample.process.os.as_ref().and_then(|os| os.pss_bytes),
                Some(sample.totals.total_attributed_bytes),
                &mut sample.sampling.threshold_reasons,
            );
            self.append_runtime_memory_sample(&sample);
            wrote_attribution = true;
        }

        if !wrote_attribution {
            controller.defer_event(event);
        }

        self.runtime_memory_log = Some(controller);
    }

    pub(super) fn maybe_capture_runtime_memory_heartbeat(&mut self) {
        let Some(mut controller) = self.runtime_memory_log.take() else {
            return;
        };

        let now = Instant::now();
        if controller.process_heartbeat_due(now) {
            let process_sample = self.capture_runtime_memory_process_sample(
                "process:heartbeat",
                crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                    category: "process_heartbeat".to_string(),
                    reason: "periodic".to_string(),
                    session_id: Some(self.session.id.clone()),
                    detail: None,
                },
                controller.build_sampling_for_process(None),
            );
            controller.record_process_sample(now);
            self.append_runtime_memory_sample(&process_sample);

            if let Some(sampling) =
                controller.build_sampling_for_attribution(now, &process_sample.process, None, None)
            {
                let mut sample = self.capture_runtime_memory_attribution_sample(
                    "attribution:process-heartbeat",
                    crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                        category: "process_heartbeat".to_string(),
                        reason: "threshold_flush".to_string(),
                        session_id: Some(self.session.id.clone()),
                        detail: None,
                    },
                    sampling,
                );
                controller.finalize_attribution_totals(
                    now,
                    sample.process.os.as_ref().and_then(|os| os.pss_bytes),
                    Some(sample.totals.total_attributed_bytes),
                    &mut sample.sampling.threshold_reasons,
                );
                self.append_runtime_memory_sample(&sample);
            }
        }

        if controller.attribution_heartbeat_due(now) {
            let preflight = self.capture_runtime_memory_process_sample(
                "process:attribution-preflight",
                crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                    category: "attribution_heartbeat".to_string(),
                    reason: "preflight".to_string(),
                    session_id: Some(self.session.id.clone()),
                    detail: None,
                },
                crate::runtime_memory_log::RuntimeMemoryLogSampling::default(),
            );
            if let Some(sampling) = controller.build_sampling_for_attribution(
                now,
                &preflight.process,
                None,
                Some("attribution_heartbeat"),
            ) {
                let mut sample = self.capture_runtime_memory_attribution_sample(
                    "attribution:heartbeat",
                    crate::runtime_memory_log::RuntimeMemoryLogTrigger {
                        category: "attribution_heartbeat".to_string(),
                        reason: "periodic".to_string(),
                        session_id: Some(self.session.id.clone()),
                        detail: None,
                    },
                    sampling,
                );
                controller.finalize_attribution_totals(
                    now,
                    sample.process.os.as_ref().and_then(|os| os.pss_bytes),
                    Some(sample.totals.total_attributed_bytes),
                    &mut sample.sampling.threshold_reasons,
                );
                self.append_runtime_memory_sample(&sample);
            } else {
                controller.mark_attribution_heartbeat_pending();
            }
        }

        self.runtime_memory_log = Some(controller);
    }

    fn capture_runtime_memory_process_sample(
        &self,
        source: &str,
        trigger: crate::runtime_memory_log::RuntimeMemoryLogTrigger,
        sampling: crate::runtime_memory_log::RuntimeMemoryLogSampling,
    ) -> crate::runtime_memory_log::ClientRuntimeMemorySample {
        let now = chrono::Utc::now();
        let process =
            crate::process_memory::snapshot_with_source(format!("client:runtime-log:{source}"));
        crate::runtime_memory_log::ClientRuntimeMemorySample {
            schema_version: 2,
            kind: "process".to_string(),
            timestamp: now.to_rfc3339(),
            timestamp_ms: now.timestamp_millis(),
            source: source.to_string(),
            trigger,
            sampling,
            client: self.runtime_memory_client_info(),
            process_diagnostics: crate::runtime_memory_log::build_process_diagnostics(&process),
            process,
            totals: crate::runtime_memory_log::ClientRuntimeMemoryTotals::default(),
            session: None,
            ui: None,
            ui_render: None,
            side_panel_render: None,
            markdown: None,
            mermaid: None,
            visual_debug: None,
        }
    }

    fn capture_runtime_memory_attribution_sample(
        &self,
        source: &str,
        trigger: crate::runtime_memory_log::RuntimeMemoryLogTrigger,
        sampling: crate::runtime_memory_log::RuntimeMemoryLogSampling,
    ) -> crate::runtime_memory_log::ClientRuntimeMemorySample {
        let now = chrono::Utc::now();
        let process =
            crate::process_memory::snapshot_with_source(format!("client:runtime-log:{source}"));
        let profile = self.runtime_memory_profile();
        let session = profile.get("session").cloned();
        let ui = profile.get("ui").cloned();
        let ui_render = profile.get("ui_render").cloned();
        let side_panel_render = profile.get("side_panel_render").cloned();
        let markdown = profile.get("markdown").cloned();
        let mermaid = profile.get("mermaid").cloned();
        let visual_debug = profile.get("visual_debug").cloned();
        let totals = client_runtime_totals_from_profile(&profile);

        crate::runtime_memory_log::ClientRuntimeMemorySample {
            schema_version: 2,
            kind: "attribution".to_string(),
            timestamp: now.to_rfc3339(),
            timestamp_ms: now.timestamp_millis(),
            source: source.to_string(),
            trigger,
            sampling,
            client: self.runtime_memory_client_info(),
            process_diagnostics: crate::runtime_memory_log::build_process_diagnostics(&process),
            process,
            totals,
            session,
            ui,
            ui_render,
            side_panel_render,
            markdown,
            mermaid,
            visual_debug,
        }
    }

    fn runtime_memory_client_info(&self) -> crate::runtime_memory_log::ClientRuntimeMemoryClient {
        crate::runtime_memory_log::ClientRuntimeMemoryClient {
            client_instance_id: self.remote_client_instance_id.clone(),
            session_id: self.session.id.clone(),
            remote_session_id: self.remote_session_id.clone(),
            provider: self.provider.name().to_string(),
            model: self.provider.model(),
            is_remote: self.is_remote,
            is_processing: self.is_processing,
            uptime_secs: self.app_started.elapsed().as_secs(),
        }
    }

    fn append_runtime_memory_sample(
        &self,
        sample: &crate::runtime_memory_log::ClientRuntimeMemorySample,
    ) {
        if let Err(err) = crate::runtime_memory_log::append_client_sample(sample) {
            crate::logging::info(&format!(
                "Client runtime memory logging sample failed: {}",
                err
            ));
            return;
        }
        let _ = crate::runtime_memory_log::prune_old_client_logs();
    }
}

fn client_runtime_totals_from_profile(
    profile: &serde_json::Value,
) -> crate::runtime_memory_log::ClientRuntimeMemoryTotals {
    let mcp_estimate_bytes = nested_u64(profile, &["ui", "mcp", "configured_json_bytes"])
        + nested_u64(profile, &["ui", "mcp", "tool_schema_estimate_bytes"]);
    let remote_state_bytes =
        nested_u64(profile, &["ui", "remote_state", "available_entries_bytes"])
            + nested_u64(profile, &["ui", "remote_state", "model_options_json_bytes"])
            + nested_u64(profile, &["ui", "remote_state", "skills_bytes"])
            + nested_u64(profile, &["ui", "remote_state", "mcp_servers_bytes"])
            + nested_u64(profile, &["ui", "remote_state", "mcp_server_names_bytes"])
            + nested_u64(
                profile,
                &["ui", "remote_state", "remote_total_tokens_json_bytes"],
            );
    let markdown_cache_estimate_bytes =
        nested_u64(profile, &["markdown", "highlight_cache_estimate_bytes"]);
    let ui_body_cache_estimate_bytes = nested_u64(
        profile,
        &["ui_render", "body_cache", "unique_prepared_bytes"],
    );
    let ui_full_prep_cache_estimate_bytes = nested_u64(
        profile,
        &["ui_render", "full_prep_cache", "unique_prepared_bytes"],
    );
    let ui_visible_copy_targets_estimate_bytes = nested_u64(
        profile,
        &["ui_render", "visible_copy_targets", "estimate_bytes"],
    );
    let ui_render_total_estimate_bytes =
        nested_u64(profile, &["ui_render", "total_estimate_bytes"]);
    let side_panel_pinned_cache_estimate_bytes = nested_u64(
        profile,
        &["side_panel_render", "pinned_cache", "entries_bytes"],
    ) + nested_u64(
        profile,
        &["side_panel_render", "pinned_cache", "rendered_lines_bytes"],
    );
    let side_panel_markdown_cache_estimate_bytes = nested_u64(
        profile,
        &[
            "side_panel_render",
            "side_panel_markdown_cache",
            "entries_bytes",
        ],
    ) + nested_u64(
        profile,
        &[
            "side_panel_render",
            "side_panel_markdown_cache",
            "key_bytes",
        ],
    );
    let side_panel_render_cache_estimate_bytes = nested_u64(
        profile,
        &[
            "side_panel_render",
            "side_panel_render_cache",
            "entries_bytes",
        ],
    ) + nested_u64(
        profile,
        &["side_panel_render", "side_panel_render_cache", "key_bytes"],
    );
    let side_panel_render_total_estimate_bytes =
        nested_u64(profile, &["side_panel_render", "total_estimate_bytes"]);
    let mermaid_working_set_estimate_bytes =
        nested_u64(profile, &["mermaid", "mermaid_working_set_estimate_bytes"]);
    let mermaid_cache_metadata_estimate_bytes = nested_u64(
        profile,
        &["mermaid", "render_cache_metadata_estimate_bytes"],
    ) + nested_u64(
        profile,
        &["mermaid", "image_state_protocol_min_estimate_bytes"],
    );
    let visual_debug_frame_estimate_bytes =
        nested_u64(profile, &["visual_debug", "frame_json_estimate_bytes"]);

    let mut totals = crate::runtime_memory_log::ClientRuntimeMemoryTotals {
        session_json_bytes: nested_u64(profile, &["session", "totals", "json_bytes"]),
        canonical_transcript_json_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "canonical_transcript_json_bytes",
            ],
        ),
        provider_cache_json_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "session_provider_cache_json_bytes",
            ],
        ),
        provider_messages_json_bytes: nested_u64(
            profile,
            &["ui", "provider_messages", "json_bytes"],
        ),
        provider_view_json_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "provider_view_json_bytes",
            ],
        ),
        transient_provider_materialization_json_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "transient_provider_materialization_json_bytes",
            ],
        ),
        display_messages_estimate_bytes: nested_u64(
            profile,
            &["ui", "display_messages", "estimate_bytes"],
        ),
        display_content_bytes: nested_u64(
            profile,
            &["ui", "transcript_memory", "totals", "display_content_bytes"],
        ),
        display_tool_metadata_json_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "display_tool_metadata_json_bytes",
            ],
        ),
        display_large_tool_output_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "display_large_tool_output_bytes",
            ],
        ),
        side_panel_estimate_bytes: nested_u64(
            profile,
            &["ui", "transcript_memory", "side_panel", "estimate_bytes"],
        ),
        side_panel_content_bytes: nested_u64(
            profile,
            &[
                "ui",
                "transcript_memory",
                "totals",
                "side_panel_content_bytes",
            ],
        ),
        remote_side_pane_images_bytes: nested_u64(
            profile,
            &["ui", "images_and_views", "remote_side_pane_images_bytes"],
        ),
        input_text_bytes: nested_u64(profile, &["ui", "input", "text_bytes"]),
        streaming_text_bytes: nested_u64(profile, &["ui", "streaming", "streaming_text_bytes"]),
        thinking_buffer_bytes: nested_u64(profile, &["ui", "streaming", "thinking_buffer_bytes"]),
        stream_buffered_text_bytes: nested_u64(
            profile,
            &["ui", "streaming", "stream_buffer", "buffered_text_bytes"],
        ),
        streaming_tool_calls_json_bytes: nested_u64(
            profile,
            &["ui", "streaming", "streaming_tool_calls_json_bytes"],
        ),
        pasted_contents_bytes: nested_u64(
            profile,
            &["ui", "clipboard_and_input_media", "pasted_contents_bytes"],
        ),
        pending_images_bytes: nested_u64(
            profile,
            &["ui", "clipboard_and_input_media", "pending_images_bytes"],
        ),
        remote_state_bytes,
        mcp_estimate_bytes,
        markdown_cache_estimate_bytes,
        ui_render_total_estimate_bytes,
        ui_body_cache_estimate_bytes,
        ui_full_prep_cache_estimate_bytes,
        ui_visible_copy_targets_estimate_bytes,
        side_panel_render_total_estimate_bytes,
        side_panel_pinned_cache_estimate_bytes,
        side_panel_markdown_cache_estimate_bytes,
        side_panel_render_cache_estimate_bytes,
        mermaid_working_set_estimate_bytes,
        mermaid_cache_metadata_estimate_bytes,
        visual_debug_frame_estimate_bytes,
        total_attributed_bytes: 0,
    };

    totals.total_attributed_bytes = totals.session_json_bytes
        + totals.provider_messages_json_bytes
        + totals.display_messages_estimate_bytes
        + totals.side_panel_estimate_bytes
        + totals.remote_side_pane_images_bytes
        + totals.input_text_bytes
        + totals.streaming_text_bytes
        + totals.thinking_buffer_bytes
        + totals.stream_buffered_text_bytes
        + totals.streaming_tool_calls_json_bytes
        + totals.pasted_contents_bytes
        + totals.pending_images_bytes
        + totals.remote_state_bytes
        + totals.mcp_estimate_bytes
        + totals.markdown_cache_estimate_bytes
        + totals.ui_render_total_estimate_bytes
        + totals.side_panel_render_total_estimate_bytes
        + totals.mermaid_working_set_estimate_bytes
        + totals.visual_debug_frame_estimate_bytes;
    totals
}

fn nested_u64(value: &serde_json::Value, path: &[&str]) -> u64 {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return 0;
        };
        cursor = next;
    }
    cursor.as_u64().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_runtime_totals_include_ui_render_side_panel_render_and_remote_images() {
        let profile = serde_json::json!({
            "ui": {
                "images_and_views": {
                    "remote_side_pane_images_bytes": 4096,
                },
            },
            "ui_render": {
                "body_cache": {
                    "unique_prepared_bytes": 10,
                },
                "full_prep_cache": {
                    "unique_prepared_bytes": 20,
                },
                "visible_copy_targets": {
                    "estimate_bytes": 30,
                },
                "total_estimate_bytes": 60,
            },
            "side_panel_render": {
                "pinned_cache": {
                    "entries_bytes": 40,
                    "rendered_lines_bytes": 2,
                },
                "side_panel_markdown_cache": {
                    "entries_bytes": 50,
                    "key_bytes": 3,
                },
                "side_panel_render_cache": {
                    "entries_bytes": 60,
                    "key_bytes": 4,
                },
                "total_estimate_bytes": 159,
            },
            "mermaid": {
                "render_cache_metadata_estimate_bytes": 100,
                "image_state_protocol_min_estimate_bytes": 200,
                "source_cache_decoded_estimate_bytes": 300,
                "mermaid_working_set_estimate_bytes": 600,
            },
        });

        let totals = client_runtime_totals_from_profile(&profile);

        assert_eq!(totals.remote_side_pane_images_bytes, 4096);
        assert_eq!(totals.ui_body_cache_estimate_bytes, 10);
        assert_eq!(totals.ui_full_prep_cache_estimate_bytes, 20);
        assert_eq!(totals.ui_visible_copy_targets_estimate_bytes, 30);
        assert_eq!(totals.ui_render_total_estimate_bytes, 60);
        assert_eq!(totals.side_panel_pinned_cache_estimate_bytes, 42);
        assert_eq!(totals.side_panel_markdown_cache_estimate_bytes, 53);
        assert_eq!(totals.side_panel_render_cache_estimate_bytes, 64);
        assert_eq!(totals.side_panel_render_total_estimate_bytes, 159);
        assert_eq!(totals.mermaid_working_set_estimate_bytes, 600);
        assert_eq!(totals.mermaid_cache_metadata_estimate_bytes, 300);
        assert_eq!(totals.total_attributed_bytes, 4096 + 60 + 159 + 600);
    }
}
