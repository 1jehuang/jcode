# Code Quality Audit - 2026-04-18

This report inventories the repo-wide code-quality issues detectable with static scanning and targeted structural heuristics. It is intended as a comprehensive backlog seed, not just a shortlist.

## Scope and method

- scanned all Rust files outside `target`, `.git`, and `node_modules`
- measured file size by LOC
- approximated function size by brace-balanced `fn` blocks
- counted panic-prone macros and methods with path-based test classification
- inventoried `allow(...)` suppressions and TODO/FIXME/HACK/XXX markers
- note: path-based production vs test classification is approximate and may overcount test-only code embedded inside production files

## Current positives

- `cargo clippy --all-targets --all-features -- -D warnings` passes cleanly
- no `#[allow(dead_code)]` suppressions remain in Rust sources
- formatting is currently clean

## Repo metrics

- Rust files scanned: **455**
- `src/` Rust files: **429** totaling **277,014 LOC**
- `tests/` Rust files: **11** totaling **4,802 LOC**
- `crates/` Rust files: **14** totaling **5,335 LOC**
- Production files over 1200 LOC: **50**
- Production files between 801 and 1200 LOC: **62**
- Approximate production functions over 100 LOC: **304** across **165** files

## `unwrap` / `expect` split by production vs test-only files

Using improved path-based classification for Rust files:
- production files exclude `tests/`, `*_test.rs`, `*_tests.rs`, and directories ending in `_test` / `_tests`
- test-only files include `tests/` and Rust files or directories explicitly marked as tests
- note: this is still path-based, so test-only code embedded inside production files is counted as production

### Counts

| Scope | `unwrap` / `expect` occurrences |
|---|---:|
| Production files | **1258** |
| Test-only files | **1334** |

### Highest-count production files

| Count | File |
|---:|---|
| 136 | `src/tool/communicate.rs` |
| 62 | `src/build.rs` |
| 52 | `src/auth/cursor.rs` |
| 46 | `src/auth/codex.rs` |
| 42 | `src/provider/openai.rs` |
| 37 | `src/auth/claude.rs` |
| 30 | `src/cli/dispatch.rs` |
| 28 | `src/tool/bash.rs` |
| 26 | `src/storage.rs` |
| 25 | `src/auth/gemini.rs` |
| 25 | `src/tool/read.rs` |
| 25 | `src/tui/session_picker/loading.rs` |
| 24 | `src/side_panel.rs` |
| 24 | `src/cli/args.rs` |
| 24 | `src/server/comm_control.rs` |

### Highest-count test-only files

| Count | File |
|---:|---|
| 788 | `src/tui/app/tests.rs` |
| 98 | `src/tool/selfdev/tests.rs` |
| 59 | `src/memory_tests.rs` |
| 44 | `src/import_tests.rs` |
| 26 | `src/provider/tests.rs` |
| 26 | `src/tool/agentgrep_tests.rs` |
| 24 | `src/tui/mermaid_tests.rs` |
| 24 | `src/server/socket_tests.rs` |
| 21 | `src/tui/markdown_tests/cases.rs` |
| 20 | `src/provider/openrouter_tests.rs` |
| 18 | `src/tui/ui_pinned_tests.rs` |
| 17 | `src/cli/provider_init_tests.rs` |
| 15 | `src/agent_tests.rs` |
| 12 | `tests/e2e/provider_behavior.rs` |
| 12 | `src/server/startup_tests.rs` |

## Structural debt

### Production files over 1200 LOC

| LOC | File |
|---:|---|
| 3228 | `src/server/comm_control.rs` |
| 3165 | `src/tool/communicate.rs` |
| 2729 | `src/session.rs` |
| 2704 | `src/server/client_lifecycle.rs` |
| 2683 | `src/provider/openai.rs` |
| 2437 | `src/tui/ui.rs` |
| 2397 | `src/memory.rs` |
| 2365 | `src/provider/mod.rs` |
| 2217 | `src/telemetry.rs` |
| 2131 | `src/tui/ui_messages.rs` |
| 2115 | `src/tui/session_picker.rs` |
| 2041 | `src/tui/app/inline_interactive.rs` |
| 2023 | `src/tui/app/input.rs` |
| 2005 | `src/config.rs` |
| 1969 | `src/provider/anthropic.rs` |
| 1919 | `src/tui/app/remote/key_handling.rs` |
| 1912 | `src/tui/app/auth.rs` |
| 1900 | `src/usage.rs` |
| 1888 | `src/tui/session_picker/loading.rs` |
| 1881 | `src/cli/login.rs` |
| 1794 | `src/replay.rs` |
| 1769 | `src/cli/provider_init.rs` |
| 1738 | `src/bin/tui_bench.rs` |
| 1718 | `src/compaction.rs` |
| 1708 | `src/tui/ui_prepare.rs` |
| 1696 | `src/memory_agent.rs` |
| 1688 | `src/tui/info_widget.rs` |
| 1678 | `src/tui/ui_pinned.rs` |
| 1670 | `src/cli/tui_launch.rs` |
| 1630 | `src/tui/app/commands.rs` |
| 1607 | `src/auth/mod.rs` |
| 1572 | `src/tui/ui_input.rs` |
| 1559 | `src/server.rs` |
| 1551 | `src/tui/app/helpers.rs` |
| 1516 | `src/tool/agentgrep.rs` |
| 1504 | `src/import.rs` |
| 1496 | `src/ambient.rs` |
| 1491 | `src/server/swarm.rs` |
| 1446 | `src/tui/ui_tools.rs` |
| 1375 | `src/tui/markdown.rs` |
| 1362 | `src/protocol.rs` |
| 1341 | `src/tool/ambient.rs` |
| 1308 | `src/auth/oauth.rs` |
| 1300 | `src/tui/app/remote.rs` |
| 1292 | `src/tui/app/turn.rs` |
| 1263 | `src/provider/models.rs` |
| 1257 | `src/server/client_actions.rs` |
| 1211 | `src/tui/app/model_context.rs` |
| 1210 | `src/tui/app/tui_state.rs` |
| 1202 | `src/provider/gemini.rs` |

### Production files between 801 and 1200 LOC

| LOC | File |
|---:|---|
| 1195 | `src/video_export.rs` |
| 1192 | `src/tui/app/auth_account_picker.rs` |
| 1167 | `src/tui/mod.rs` |
| 1155 | `src/provider/copilot.rs` |
| 1150 | `src/tui/app/state_ui.rs` |
| 1144 | `src/tool/browser.rs` |
| 1142 | `src/provider/claude.rs` |
| 1132 | `src/provider/openrouter.rs` |
| 1125 | `src/tui/app/remote/server_events.rs` |
| 1124 | `src/tui/app/debug_bench.rs` |
| 1116 | `src/tui/mermaid.rs` |
| 1109 | `src/update.rs` |
| 1094 | `src/server/client_session.rs` |
| 1093 | `src/provider/openai_stream_runtime.rs` |
| 1087 | `src/tool/mod.rs` |
| 1075 | `src/tui/app/state_ui_input_helpers.rs` |
| 1071 | `src/server/comm_session.rs` |
| 1057 | `src/ambient/runner.rs` |
| 1043 | `src/provider/cursor.rs` |
| 1039 | `src/cli/commands.rs` |
| 1038 | `src/server/debug.rs` |
| 1038 | `src/message.rs` |
| 1037 | `src/tui/app/commands_review.rs` |
| 1014 | `src/tui/app/navigation.rs` |
| 1012 | `src/tui/account_picker.rs` |
| 995 | `src/goal.rs` |
| 980 | `src/memory_graph.rs` |
| 979 | `src/tui/markdown_render_full.rs` |
| 976 | `src/auth/claude.rs` |
| 970 | `src/auth/cursor.rs` |
| 958 | `src/browser.rs` |
| 956 | `src/runtime_memory_log.rs` |
| 945 | `src/agent/turn_streaming_mpsc.rs` |
| 929 | `src/cli/dispatch.rs` |
| 925 | `src/tui/ui_animations.rs` |
| 923 | `src/tui/app/auth_account_commands.rs` |
| 918 | `src/tui/test_harness.rs` |
| 911 | `src/auth/codex.rs` |
| 902 | `src/tui/keybind.rs` |
| 900 | `src/tui/ui_inline_interactive.rs` |
| 897 | `src/tui/ui_header.rs` |
| 895 | `src/server/state.rs` |
| 892 | `src/build.rs` |
| 881 | `src/tui/backend.rs` |
| 878 | `src/tui/login_picker.rs` |
| 872 | `src/sidecar.rs` |
| 868 | `src/tui/app/tui_lifecycle.rs` |
| 865 | `src/tui/permissions.rs` |
| 865 | `src/tui/markdown_render_lazy.rs` |
| 863 | `src/gateway.rs` |
| 862 | `src/tool/read.rs` |
| 860 | `src/provider/antigravity.rs` |
| 859 | `src/tool/apply_patch.rs` |
| 858 | `src/tool/bash.rs` |
| 849 | `src/auth/gemini.rs` |
| 847 | `src/tui/visual_debug.rs` |
| 827 | `src/setup_hints.rs` |
| 826 | `src/server/reload.rs` |
| 815 | `src/auth/copilot.rs` |
| 812 | `src/tui/app.rs` |
| 804 | `src/tui/app/remote/reconnect.rs` |
| 803 | `src/server/debug_swarm_read.rs` |

### Test files over 1200 LOC

| LOC | File |
|---:|---|
| 13615 | `src/tui/app/tests.rs` |
| 1263 | `src/server/client_session_tests/resume.rs` |
| 1252 | `src/provider/tests.rs` |
| 1226 | `src/cli/auth_test.rs` |

### Files with the most >100 LOC production functions

| Count | File |
|---:|---|
| 8 | `src/server/comm_control.rs` |
| 7 | `src/tool/communicate.rs` |
| 6 | `src/provider/mod.rs` |
| 5 | `src/auth/mod.rs` |
| 5 | `src/tui/app/auth.rs` |
| 5 | `src/tui/app/debug_bench.rs` |
| 4 | `src/provider/anthropic.rs` |
| 4 | `src/tui/ui_pinned.rs` |
| 4 | `src/tui/ui_prepare.rs` |
| 4 | `src/tui/app/inline_interactive.rs` |
| 4 | `src/tui/app/auth_account_picker.rs` |
| 4 | `src/cli/tui_launch.rs` |
| 4 | `src/server/client_comm.rs` |
| 3 | `src/import.rs` |
| 3 | `src/memory_agent.rs` |
| 3 | `src/replay.rs` |
| 3 | `src/video_export.rs` |
| 3 | `src/server.rs` |
| 3 | `src/usage.rs` |
| 3 | `src/config.rs` |
| 3 | `src/bin/tui_bench.rs` |
| 3 | `src/provider/claude.rs` |
| 3 | `src/provider/copilot.rs` |
| 3 | `src/provider/openai_stream_runtime.rs` |
| 3 | `src/tui/ui_animations.rs` |
| 3 | `src/tui/ui_input.rs` |
| 3 | `src/tui/ui_header.rs` |
| 3 | `src/tui/info_widget.rs` |
| 3 | `src/tui/app/model_context.rs` |
| 3 | `src/tui/app/tui_state.rs` |
| 3 | `src/tui/app/auth_account_commands.rs` |
| 3 | `src/tui/app/commands.rs` |
| 3 | `src/tui/app/remote.rs` |
| 3 | `src/tui/app/debug_profile.rs` |
| 3 | `src/tui/session_picker/loading.rs` |
| 3 | `src/server/comm_plan.rs` |
| 3 | `src/server/client_actions.rs` |
| 3 | `src/server/client_session.rs` |
| 3 | `src/server/swarm.rs` |
| 3 | `src/server/client_lifecycle.rs` |
| 2 | `src/compaction.rs` |
| 2 | `src/telemetry.rs` |
| 2 | `src/background.rs` |
| 2 | `src/auth/oauth.rs` |
| 2 | `src/provider/dispatch.rs` |
| 2 | `src/tool/apply_patch.rs` |
| 2 | `src/tool/agentgrep.rs` |
| 2 | `src/tool/bash.rs` |
| 2 | `src/tool/browser.rs` |
| 2 | `src/tool/selfdev/build_queue.rs` |

### Longest production functions detected

| LOC | Function | Location |
|---:|---|---|
| 1827 | `handle_remote_key_internal` | `src/tui/app/remote/key_handling.rs:93-1919` |
| 1658 | `handle_client` | `src/server/client_lifecycle.rs:669-2326` |
| 1121 | `handle_server_event` | `src/tui/app/remote/server_events.rs:5-1125` |
| 1016 | `run_turn_interactive` | `src/tui/app/turn.rs:23-1038` |
| 976 | `render_markdown_with_width` | `src/tui/markdown_render_full.rs:4-979` |
| 941 | `run_turn_streaming_mpsc` | `src/agent/turn_streaming_mpsc.rs:4-944` |
| 863 | `render_markdown_lazy` | `src/tui/markdown_render_lazy.rs:3-865` |
| 783 | `maybe_handle_swarm_read_command` | `src/server/debug_swarm_read.rs:21-803` |
| 780 | `execute` | `src/tool/communicate.rs:727-1506` |
| 771 | `run_turn_streaming` | `src/agent/turn_streaming_broadcast.rs:4-774` |
| 760 | `run_turn` | `src/agent/turn_loops.rs:9-768` |
| 602 | `complete` | `src/provider/openrouter_provider_impl.rs:6-607` |
| 591 | `draw_inner` | `src/tui/ui.rs:1758-2348` |
| 556 | `handle_debug_command` | `src/tui/app/debug_cmds.rs:4-559` |
| 548 | `handle_lightweight_control_request` | `src/server/client_lifecycle.rs:105-652` |
| 525 | `draw_messages` | `src/tui/ui_viewport.rs:147-671` |
| 509 | `get_suggestions_for` | `src/tui/app/state_ui_input_helpers.rs:374-882` |
| 501 | `handle_login_input` | `src/tui/app/auth.rs:1166-1666` |
| 490 | `get_tool_summary_with_budget` | `src/tui/ui_tools.rs:887-1376` |
| 487 | `execute_debug_command` | `src/server/debug_command_exec.rs:88-574` |
| 482 | `spawn_background_tasks` | `src/server.rs:651-1132` |
| 470 | `main` | `src/bin/tui_bench.rs:1269-1738` |
| 443 | `test_parse_openai_response_function_call_arguments_streaming` | `src/provider/openai.rs:2241-2683` |
| 433 | `apply_env_overrides` | `src/config.rs:773-1205` |
| 429 | `draw_inline_interactive` | `src/tui/ui_inline_interactive.rs:259-687` |
| 422 | `maybe_handle_swarm_write_command` | `src/server/debug_swarm_write.rs:11-432` |
| 408 | `build_server_memory_payload` | `src/server/debug_server_state.rs:254-661` |
| 405 | `handle_resume_session` | `src/server/client_session.rs:686-1090` |
| 404 | `prepare_body_incremental` | `src/tui/ui_prepare.rs:608-1011` |
| 401 | `handle_info_command` | `src/tui/app/state_ui.rs:750-1150` |
| 401 | `handle_comm_task_control` | `src/server/comm_control.rs:1546-1946` |
| 393 | `render_preview` | `src/tui/session_picker.rs:795-1187` |
| 382 | `draw_pinned_content_cached` | `src/tui/ui_pinned.rs:842-1223` |
| 380 | `build_responses_input` | `src/provider/openai_request.rs:286-665` |
| 376 | `stream_response_websocket_persistent` | `src/provider/openai_stream_runtime.rs:551-926` |
| 371 | `handle_inline_interactive_key` | `src/tui/app/inline_interactive.rs:1551-1921` |
| 369 | `set_model` | `src/provider/mod.rs:821-1189` |
| 367 | `render_tool_message` | `src/tui/ui_messages.rs:864-1230` |
| 362 | `prepare_body` | `src/tui/ui_prepare.rs:1066-1427` |
| 358 | `handle_comm_assign_task` | `src/server/comm_control.rs:1008-1365` |
| 346 | `draw_help_overlay` | `src/tui/ui_overlays.rs:85-430` |
| 340 | `debug_app_owned_memory_profile` | `src/tui/app/debug_profile.rs:170-509` |
| 339 | `handle_session_command` | `src/tui/app/commands.rs:578-916` |
| 324 | `try_persistent_ws_continuation` | `src/provider/openai_stream_runtime.rs:224-547` |
| 320 | `init_provider_with_options` | `src/cli/provider_init.rs:1428-1747` |
| 316 | `new` | `src/tui/app/tui_lifecycle.rs:422-737` |
| 316 | `execute` | `src/tool/gmail.rs:93-408` |
| 315 | `draw_status` | `src/tui/ui_input.rs:397-711` |
| 313 | `model_routes` | `src/provider/mod.rs:1342-1654` |
| 312 | `handle_debug_client` | `src/server/debug.rs:184-495` |
| 307 | `get_relevant_parallel` | `src/memory.rs:1752-2058` |
| 304 | `list_sessions` | `src/cli/tui_launch.rs:1146-1449` |
| 303 | `execute` | `src/tool/memory.rs:116-418` |
| 296 | `complete` | `src/provider/openai_provider_impl.rs:8-303` |
| 294 | `run_loop` | `src/ambient/runner.rs:443-736` |
| 291 | `run_scroll_test` | `src/tui/app/debug_bench.rs:710-1000` |
| 290 | `new_minimal_with_session` | `src/tui/app/tui_lifecycle.rs:131-420` |
| 289 | `open_account_center` | `src/tui/app/auth_account_picker.rs:4-292` |
| 277 | `handle_model_command` | `src/tui/app/model_context.rs:862-1138` |
| 277 | `monitor_bus` | `src/server.rs:1162-1438` |
| 275 | `display_string` | `src/config.rs:1688-1962` |
| 267 | `emit_lifecycle_event` | `src/telemetry.rs:1929-2195` |
| 261 | `prepare_messages_inner` | `src/tui/ui_prepare.rs:300-560` |
| 261 | `render_mermaid_sized_internal` | `src/tui/mermaid_cache_render.rs:427-687` |
| 261 | `handle_mouse_event` | `src/tui/app/navigation.rs:683-943` |
| 260 | `open_model_picker` | `src/tui/app/inline_interactive.rs:728-987` |
| 258 | `build_all_inline_account_picker` | `src/tui/app/auth_account_picker.rs:445-702` |
| 256 | `buffer_to_svg` | `src/video_export.rs:610-865` |
| 253 | `info_widget_data` | `src/tui/app/tui_state.rs:769-1021` |
| 249 | `build_header_lines` | `src/tui/ui_header.rs:421-669` |
| 249 | `extract_from_context` | `src/memory_agent.rs:725-973` |
| 245 | `box_drawing_to_svg` | `src/video_export.rs:931-1175` |
| 243 | `do_build` | `src/tool/selfdev/build_queue.rs:340-582` |
| 241 | `spawn_assigned_task_run` | `src/server/comm_control.rs:441-681` |
| 240 | `complete` | `src/provider/gemini.rs:393-632` |
| 240 | `process_context` | `src/memory_agent.rs:393-632` |
| 240 | `create_default_config_file` | `src/config.rs:1446-1685` |
| 238 | `handle_comm_propose_plan` | `src/server/comm_plan.rs:25-262` |
| 238 | `login_google_flow` | `src/cli/login.rs:1538-1775` |
| 235 | `new_with_auth_status` | `src/provider/startup.rs:50-284` |
| 234 | `run_side_panel_latency_bench` | `src/tui/app/debug_bench.rs:79-312` |
| 233 | `try_auto_compact_and_retry` | `src/tui/app/model_context.rs:441-673` |
| 233 | `handle_config_command` | `src/tui/app/commands.rs:1279-1511` |
| 232 | `run_mermaid_ui_bench` | `src/tui/app/debug_bench.rs:314-545` |
| 232 | `build_ambient_system_prompt` | `src/ambient.rs:788-1019` |
| 229 | `maybe_handle_server_state_command` | `src/server/debug_server_state.rs:20-248` |
| 228 | `run_memory_command` | `src/cli/commands.rs:161-388` |
| 225 | `draw_side_panel_markdown` | `src/tui/ui_pinned.rs:1225-1449` |
| 224 | `compact_tool_input_for_display` | `src/tui/app/state_ui_storage.rs:3-226` |
| 223 | `send_history` | `src/server/client_state.rs:232-454` |
| 221 | `handle_debug_command` | `src/tui/app/debug.rs:538-758` |
| 221 | `run_main` | `src/cli/dispatch.rs:21-241` |
| 220 | `rebuild_items` | `src/tui/session_picker/filter.rs:125-344` |
| 220 | `export_timeline` | `src/replay.rs:138-357` |
| 217 | `handle_comm_message` | `src/server/client_comm_message.rs:149-365` |
| 215 | `bridge_request` | `src/tool/browser.rs:472-686` |
| 213 | `connect_with_retry` | `src/tui/app/remote/reconnect.rs:339-551` |
| 212 | `restore_input_for_reload` | `src/tui/app/state_ui.rs:240-451` |
| 210 | `run_replay_command` | `src/cli/tui_launch.rs:437-646` |
| 208 | `shape_char_3x3` | `src/tui/ui_animations.rs:574-781` |
| 208 | `selfdev_status_output` | `src/tool/selfdev/status.rs:3-210` |
| 208 | `execute` | `src/tool/goal.rs:141-348` |
| 207 | `parse_account_command` | `src/tui/app/auth_account_commands.rs:69-275` |
| 206 | `restore_session` | `src/tui/app/tui_lifecycle_runtime.rs:212-417` |
| 205 | `render_image_widget` | `src/tui/mermaid_widget.rs:91-295` |
| 205 | `render_image_widget_viewport` | `src/tui/mermaid_viewport.rs:552-756` |
| 205 | `spawn_swarm_agent` | `src/server/comm_session.rs:196-400` |
| 205 | `handle_subscribe` | `src/server/client_session.rs:339-543` |
| 204 | `parse_next_event` | `src/provider/openrouter_sse_stream.rs:264-467` |
| 203 | `cleanup_client_connection` | `src/server/client_disconnect_cleanup.rs:55-257` |
| 198 | `calculate_placements` | `src/tui/info_widget_layout.rs:39-236` |
| 198 | `calculate_widget_height` | `src/tui/info_widget.rs:751-948` |
| 195 | `parse_text_wrapped_tool_call` | `src/agent/response_recovery.rs:4-198` |
| 194 | `do_reload` | `src/tool/selfdev/reload.rs:88-281` |
| 192 | `render_image_widget_fit_inner` | `src/tui/mermaid_widget.rs:318-509` |
| 188 | `write_frame` | `src/tui/visual_debug.rs:573-760` |
| 187 | `emit_ndjson_event` | `src/cli/commands.rs:758-944` |
| 186 | `stream_request` | `src/provider/copilot.rs:608-793` |
| 183 | `handle_ws_connection` | `src/gateway.rs:282-464` |
| 182 | `process_sse_stream` | `src/provider/copilot.rs:795-976` |

## Error-handling and panic-surface debt

Path-classified counts below are approximate. Inline `#[cfg(test)]` modules inside production files may inflate production totals.

### Macro/method counts

| Scope | unwrap | expect | panic! | todo! | unimplemented! | total |
|---|---:|---:|---:|---:|---:|---:|
| prod | 361 | 978 | 92 | 0 | 11 | 1442 |
| testlike | 501 | 832 | 52 | 0 | 10 | 1395 |

### Highest-count production files

| Total | File | unwrap | expect | panic! | todo! | unimplemented! |
|---:|---|---:|---:|---:|---:|---:|
| 136 | `src/tool/communicate.rs` | 0 | 136 | 0 | 0 | 0 |
| 64 | `src/build.rs` | 9 | 53 | 2 | 0 | 0 |
| 54 | `src/provider/openai.rs` | 7 | 38 | 9 | 0 | 0 |
| 52 | `src/auth/cursor.rs` | 48 | 4 | 0 | 0 | 0 |
| 46 | `src/auth/codex.rs` | 45 | 1 | 0 | 0 | 0 |
| 41 | `src/server/comm_control.rs` | 0 | 30 | 11 | 0 | 0 |
| 40 | `src/cli/args.rs` | 24 | 0 | 16 | 0 | 0 |
| 37 | `src/auth/claude.rs` | 28 | 9 | 0 | 0 | 0 |
| 30 | `src/cli/dispatch.rs` | 0 | 28 | 2 | 0 | 0 |
| 28 | `src/tool/bash.rs` | 7 | 21 | 0 | 0 | 0 |
| 26 | `src/storage.rs` | 0 | 26 | 0 | 0 | 0 |
| 25 | `src/tui/session_picker/loading.rs` | 0 | 25 | 0 | 0 | 0 |
| 25 | `src/tool/read.rs` | 0 | 25 | 0 | 0 | 0 |
| 25 | `src/auth/gemini.rs` | 4 | 21 | 0 | 0 | 0 |
| 24 | `src/tool/apply_patch.rs` | 15 | 1 | 8 | 0 | 0 |
| 24 | `src/side_panel.rs` | 0 | 24 | 0 | 0 | 0 |
| 24 | `src/server/client_comm.rs` | 0 | 12 | 11 | 0 | 1 |
| 23 | `src/server/reload.rs` | 0 | 23 | 0 | 0 | 0 |
| 21 | `src/tui/session_picker.rs` | 7 | 13 | 1 | 0 | 0 |
| 21 | `src/server/debug.rs` | 0 | 18 | 2 | 0 | 1 |
| 20 | `src/tool/goal.rs` | 0 | 19 | 1 | 0 | 0 |
| 20 | `src/server/comm_session.rs` | 0 | 20 | 0 | 0 | 0 |
| 19 | `src/cli/tui_launch.rs` | 0 | 18 | 1 | 0 | 0 |
| 19 | `src/auth/external.rs` | 19 | 0 | 0 | 0 | 0 |
| 18 | `src/provider/gemini.rs` | 7 | 10 | 0 | 0 | 1 |
| 17 | `src/restart_snapshot.rs` | 0 | 17 | 0 | 0 | 0 |
| 16 | `src/server/client_state.rs` | 0 | 14 | 1 | 0 | 1 |
| 16 | `src/replay.rs` | 11 | 2 | 3 | 0 | 0 |
| 16 | `src/goal.rs` | 0 | 16 | 0 | 0 | 0 |
| 15 | `src/server/client_actions.rs` | 3 | 9 | 2 | 0 | 1 |
| 14 | `src/tui/app/remote.rs` | 0 | 13 | 0 | 0 | 1 |
| 14 | `src/memory_graph.rs` | 12 | 2 | 0 | 0 | 0 |
| 14 | `src/mcp/protocol.rs` | 11 | 2 | 1 | 0 | 0 |
| 14 | `src/cli/selfdev.rs` | 1 | 12 | 0 | 0 | 1 |
| 13 | `src/setup_hints/macos_launcher.rs` | 0 | 13 | 0 | 0 | 0 |
| 13 | `src/server/client_lifecycle.rs` | 0 | 10 | 3 | 0 | 0 |
| 13 | `src/registry.rs` | 0 | 13 | 0 | 0 | 0 |
| 12 | `src/tool/batch.rs` | 12 | 0 | 0 | 0 | 0 |
| 12 | `src/server/swarm_mutation_state.rs` | 0 | 8 | 4 | 0 | 0 |
| 12 | `src/provider_catalog.rs` | 0 | 12 | 0 | 0 | 0 |
| 12 | `src/prompt.rs` | 11 | 1 | 0 | 0 | 0 |
| 11 | `src/tool/agentgrep.rs` | 0 | 11 | 0 | 0 | 0 |
| 10 | `src/tool/ambient.rs` | 10 | 0 | 0 | 0 | 0 |
| 9 | `src/soft_interrupt_store.rs` | 0 | 9 | 0 | 0 | 0 |
| 9 | `src/server/provider_control.rs` | 3 | 6 | 0 | 0 | 0 |
| 9 | `src/platform.rs` | 0 | 9 | 0 | 0 | 0 |
| 9 | `src/cli/login.rs` | 0 | 8 | 1 | 0 | 0 |
| 9 | `src/cli/commands/restart.rs` | 0 | 9 | 0 | 0 | 0 |
| 8 | `src/tool/side_panel.rs` | 0 | 8 | 0 | 0 | 0 |
| 8 | `src/tool/browser.rs` | 6 | 2 | 0 | 0 | 0 |
| 8 | `src/stdin_detect.rs` | 0 | 8 | 0 | 0 | 0 |
| 8 | `src/sidecar.rs` | 0 | 8 | 0 | 0 | 0 |
| 8 | `src/runtime_memory_log.rs` | 0 | 8 | 0 | 0 | 0 |
| 8 | `src/message.rs` | 4 | 1 | 3 | 0 | 0 |
| 8 | `src/gateway.rs` | 1 | 7 | 0 | 0 | 0 |
| 8 | `src/ambient.rs` | 8 | 0 | 0 | 0 | 0 |
| 7 | `src/server/swarm.rs` | 0 | 6 | 1 | 0 | 0 |
| 7 | `src/server/debug_testers.rs` | 0 | 7 | 0 | 0 | 0 |
| 7 | `src/provider/cursor.rs` | 4 | 3 | 0 | 0 | 0 |
| 7 | `src/dictation.rs` | 0 | 7 | 0 | 0 | 0 |

### Production files still containing `todo!` or `unimplemented!`

| Count | File |
|---:|---|
| 7 | `src/tui/app/tests.rs` |
| 1 | `src/tui/ui_header.rs` |
| 1 | `src/tui/app/remote.rs` |
| 1 | `src/tool/mod.rs` |
| 1 | `src/server/startup_tests.rs` |
| 1 | `src/server/queue_tests.rs` |
| 1 | `src/server/debug_command_exec.rs` |
| 1 | `src/server/debug.rs` |
| 1 | `src/server/client_state.rs` |
| 1 | `src/server/client_session_tests.rs` |
| 1 | `src/server/client_comm.rs` |
| 1 | `src/server/client_actions.rs` |
| 1 | `src/provider/gemini.rs` |
| 1 | `src/cli/selfdev.rs` |
| 1 | `src/ambient/runner.rs` |

## Suppression inventory

- Rust files containing `allow(...)`: **17**
- Total `allow(...)` attributes found: **28**

### Most common suppressions

| Count | Suppression |
|---:|---|
| 13 | `clippy::too_many_arguments` |
| 7 | `unused_mut` |
| 2 | `non_upper_case_globals` |
| 2 | `deprecated` |
| 2 | `unused_imports` |
| 1 | `non_snake_case` |
| 1 | `unused_variables` |

### Files containing suppressions

| Count | File | Suppressions |
|---:|---|---|
| 5 | `src/server/client_session.rs` | `clippy::too_many_arguments`, `clippy::too_many_arguments`, `clippy::too_many_arguments`, `clippy::too_many_arguments`, `clippy::too_many_arguments` |
| 3 | `src/cli/dispatch.rs` | `deprecated`, `unused_mut`, `unused_mut` |
| 2 | `src/tui/app/remote.rs` | `unused_imports`, `unused_imports` |
| 2 | `src/server/comm_session.rs` | `clippy::too_many_arguments`, `clippy::too_many_arguments` |
| 2 | `src/server/client_lifecycle.rs` | `clippy::too_many_arguments`, `clippy::too_many_arguments` |
| 2 | `src/server.rs` | `unused_mut`, `unused_mut` |
| 2 | `src/main.rs` | `non_upper_case_globals`, `non_upper_case_globals` |
| 1 | `src/tui/info_widget.rs` | `deprecated` |
| 1 | `src/tui/app/state_ui.rs` | `unused_mut` |
| 1 | `src/server/startup_tests.rs` | `unused_mut` |
| 1 | `src/server/debug_swarm_write.rs` | `clippy::too_many_arguments` |
| 1 | `src/server/comm_sync.rs` | `clippy::too_many_arguments` |
| 1 | `src/server/comm_await.rs` | `clippy::too_many_arguments` |
| 1 | `src/server/client_actions.rs` | `clippy::too_many_arguments` |
| 1 | `src/perf.rs` | `non_snake_case` |
| 1 | `src/auth/mod.rs` | `unused_mut` |
| 1 | `src/agent/turn_loops.rs` | `unused_variables` |

## TODO/FIXME/HACK debt

| Count | File |
|---:|---|
| 9 | `docs/CODE_QUALITY_AUDIT_2026-04-18.md` |
| 5 | `src/tui/ui_tests/prepare.rs` |
| 4 | `src/tui/ui_tests/tools.rs` |
| 1 | `src/stdin_detect.rs` |
| 1 | `docs/MEMORY_ARCHITECTURE.md` |
| 1 | `docs/IOS_CLIENT.md` |

## Highest-value improvement themes

1. **Split mega-files before adding more logic.** The repo has a very large number of production files far above the documented 1200 LOC ceiling, with especially acute concentration in TUI, server, provider, session, and tooling modules.
2. **Break down monster functions.** The biggest maintainability risk is not only file size but massive single functions like `handle_remote_key_internal`, `handle_client`, `handle_server_event`, `run_turn_interactive`, and multiple markdown/rendering paths.
3. **Reduce argument fan-out in server control/session code.** Repeated `#[allow(clippy::too_many_arguments)]` in server modules indicates missing request-context structs or narrower helper boundaries.
4. **Harden failure paths in real production code.** Even with clippy clean, there is still broad `unwrap`/`expect`/`panic!` presence, especially in tool execution, auth, server control, build, and provider code. Some of this is test-only code inside production files and should be moved out or isolated.
5. **Move or isolate inline tests embedded in giant production files.** Several production files carry substantial test bodies, inflating file size and panic-prone call counts.
6. **Reduce test concentration.** `src/tui/app/tests.rs` is itself a giant hotspot and should be split by domain like auth, remote, commands, rendering, and state restoration.
7. **Trim suppression surface.** Most suppressions are test-only clippy escapes, but the `too_many_arguments` suppressions in server code are architectural smell, not just lint noise.
8. **Burn down deferred work markers.** There are not many TODO/FIXME markers, which is good, but the remaining ones should still be converted into issues or resolved.

## Suggested execution order

1. split `src/server/comm_control.rs`, `src/server/client_lifecycle.rs`, `src/provider/mod.rs`, `src/provider/openai.rs`, and TUI remote/input modules
2. extract context/request structs to eliminate `too_many_arguments` suppressions in server paths
3. move inline tests out of production mega-files where practical
4. replace easy production `unwrap`/`expect` hotspots with explicit error handling, starting with tool/auth/build modules
5. continue splitting TUI render and event-handling functions into domain-focused helpers

