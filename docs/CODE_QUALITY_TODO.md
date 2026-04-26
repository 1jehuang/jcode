# Code Quality Program Todo List

This file tracks the execution backlog for the code-quality uplift program described in `docs/CODE_QUALITY_10_10_PLAN.md`.

Status values:

- `pending`
- `in_progress`
- `blocked`
- `done`

## Phase 0: Prevent Further Decay

- [x] Add CI job for `cargo check --all-targets --all-features`
- [x] Add CI job for `cargo clippy --all-targets --all-features -- -D warnings`
- [x] Keep warning policy on a downward ratchet
- [x] Add documented file-size and function-size targets to contributor guidance

## Phase 1: Warning and Dead-Code Burn-Down

- [x] Inventory all `#![allow(dead_code)]` locations and justify or remove them
- [x] Reduce baseline warning count significantly from the current level
- [ ] Remove stale unused functions in `setup_hints.rs`
- [ ] Remove stale unused code in TUI support modules
- [ ] Audit broad suppressions and replace with narrow local allowances

## Phase 2: Decompose the Biggest Files

### Highest priority
- [ ] Split `tests/e2e/main.rs` by feature area
  - Started 2026-03-24: extracted feature modules `session_flow`, `transport`, `provider_behavior`, `binary_integration`, `safety`, and `ambient`
  - Completed 2026-03-24: extracted shared helpers into `tests/e2e/test_support/mod.rs`
- [ ] Continue splitting `src/server.rs` into focused submodules ([#53](https://github.com/1jehuang/jcode/issues/53))
  - Progress 2026-03-24: extracted shared server/swarm state into `src/server/state.rs`
  - Progress 2026-03-24: extracted socket/bootstrap helpers into `src/server/socket.rs`
  - Progress 2026-03-24: extracted reload marker/signal state into `src/server/reload_state.rs`
  - Progress 2026-03-24: extracted path/update/swarm identity utilities into `src/server/util.rs`
- [ ] Split `src/agent.rs` into orchestration, stream, interrupt, and tool-exec modules

### Next wave
- [ ] Split `src/provider/mod.rs` into traits, pricing, routes, and shared HTTP helpers ([#52](https://github.com/1jehuang/jcode/issues/52))
- [ ] Split `src/provider/openai.rs` into request, stream, tool, and response modules ([#52](https://github.com/1jehuang/jcode/issues/52))
- [ ] Split `src/tui/ui.rs` by render responsibility ([#51](https://github.com/1jehuang/jcode/issues/51))
- [ ] Split `src/tui/info_widget.rs` by widget/domain sections ([#51](https://github.com/1jehuang/jcode/issues/51))

## Phase 3: Error Handling Hardening

- [ ] Count production `unwrap` / `expect` separately from test-only usages
- [ ] Replace easy production `unwrap` / `expect` hotspots with explicit errors
- [ ] Add better error context for provider stream parsing failures
- [ ] Add better error context for reload and socket lifecycle failures ([#53](https://github.com/1jehuang/jcode/issues/53))

## Phase 4: Test Strategy Improvements

- [ ] Extract shared e2e test support helpers
- [ ] Add focused tests for reload state transitions
- [ ] Add focused tests for malformed provider stream chunks
- [ ] Add snapshot or golden tests for stable TUI render outputs
- [ ] Add property tests for protocol serialization and tool parsing

## Phase 5: Reliability and Performance Guardrails

- [ ] Add repeated reload reliability test coverage
- [ ] Add repeated attach/detach and reconnect coverage
- [ ] Track memory regression expectations in a documented budget
- [ ] Improve observability around reload, swarm, and tool execution paths
- [ ] Execute the compile-performance roadmap in `docs/COMPILE_PERFORMANCE_PLAN.md`
- [ ] Add repeatable compile timing checkpoints for warm/cold self-dev loops

## Immediate Active Work

- [x] Land the quality plan document
- [x] Land this todo list
- [x] Tighten CI guardrails
- [ ] Begin the first high-ROI cleanup or split
  - Follow-up tracking issues: #51, #52, #53, #54

## Comprehensive Audit Backlog (2026-04-18)

Generated from `docs/CODE_QUALITY_AUDIT_2026-04-18.md`. This section enumerates the full file-level backlog detected by the audit so the todo list captures all current hotspots.

### Audit snapshot

- [x] Publish comprehensive audit report (`50` production files >1200 LOC, `62` production files 801-1200 LOC, `304` production functions >100 LOC across `165` files)
- [ ] Refresh this audit backlog after each major cleanup wave

### Structural backlog: production files over 1200 LOC

- [ ] Split `src/server/comm_control.rs` (3228 LOC)
- [ ] Split `src/tool/communicate.rs` (3165 LOC)
- [ ] Split `src/session.rs` (2729 LOC)
- [ ] Split `src/server/client_lifecycle.rs` (2704 LOC)
- [ ] Split `src/provider/openai.rs` (2683 LOC)
- [ ] Split `src/tui/ui.rs` (2437 LOC)
- [ ] Split `src/memory.rs` (2397 LOC)
- [ ] Split `src/provider/mod.rs` (2365 LOC)
- [ ] Split `src/telemetry.rs` (2217 LOC)
- [ ] Split `src/tui/ui_messages.rs` (2131 LOC)
- [ ] Split `src/tui/session_picker.rs` (2115 LOC)
- [ ] Split `src/tui/app/inline_interactive.rs` (2041 LOC)
- [ ] Split `src/tui/app/input.rs` (2023 LOC)
- [ ] Split `src/config.rs` (2005 LOC)
- [ ] Split `src/provider/anthropic.rs` (1969 LOC)
- [ ] Split `src/tui/app/remote/key_handling.rs` (1919 LOC)
- [ ] Split `src/tui/app/auth.rs` (1912 LOC)
- [ ] Split `src/usage.rs` (1900 LOC)
- [ ] Split `src/tui/session_picker/loading.rs` (1888 LOC)
- [ ] Split `src/cli/login.rs` (1881 LOC)
- [ ] Split `src/replay.rs` (1794 LOC)
- [ ] Split `src/cli/provider_init.rs` (1769 LOC)
- [ ] Split `src/bin/tui_bench.rs` (1738 LOC)
- [ ] Split `src/compaction.rs` (1718 LOC)
- [ ] Split `src/tui/ui_prepare.rs` (1708 LOC)
- [ ] Split `src/memory_agent.rs` (1696 LOC)
- [ ] Split `src/tui/info_widget.rs` (1688 LOC)
- [ ] Split `src/tui/ui_pinned.rs` (1678 LOC)
- [ ] Split `src/cli/tui_launch.rs` (1670 LOC)
- [ ] Split `src/tui/app/commands.rs` (1630 LOC)
- [ ] Split `src/auth/mod.rs` (1607 LOC)
- [ ] Split `src/tui/ui_input.rs` (1572 LOC)
- [ ] Split `src/server.rs` (1559 LOC)
- [ ] Split `src/tui/app/helpers.rs` (1551 LOC)
- [ ] Split `src/tool/agentgrep.rs` (1516 LOC)
- [ ] Split `src/import.rs` (1504 LOC)
- [ ] Split `src/ambient.rs` (1496 LOC)
- [ ] Split `src/server/swarm.rs` (1491 LOC)
- [ ] Split `src/tui/ui_tools.rs` (1446 LOC)
- [ ] Split `src/tui/markdown.rs` (1375 LOC)
- [ ] Split `src/protocol.rs` (1362 LOC)
- [ ] Split `src/tool/ambient.rs` (1341 LOC)
- [ ] Split `src/auth/oauth.rs` (1308 LOC)
- [ ] Split `src/tui/app/remote.rs` (1300 LOC)
- [ ] Split `src/tui/app/turn.rs` (1292 LOC)
- [ ] Split `src/provider/models.rs` (1263 LOC)
- [ ] Split `src/server/client_actions.rs` (1257 LOC)
- [ ] Split `src/tui/app/model_context.rs` (1211 LOC)
- [ ] Split `src/tui/app/tui_state.rs` (1210 LOC)
- [ ] Split `src/provider/gemini.rs` (1202 LOC)

### Structural backlog: production files between 801 and 1200 LOC

- [ ] Reduce `src/video_export.rs` below 800 LOC (1195 LOC today)
- [ ] Reduce `src/tui/app/auth_account_picker.rs` below 800 LOC (1192 LOC today)
- [ ] Reduce `src/tui/mod.rs` below 800 LOC (1167 LOC today)
- [ ] Reduce `src/provider/copilot.rs` below 800 LOC (1155 LOC today)
- [ ] Reduce `src/tui/app/state_ui.rs` below 800 LOC (1150 LOC today)
- [ ] Reduce `src/tool/browser.rs` below 800 LOC (1144 LOC today)
- [ ] Reduce `src/provider/claude.rs` below 800 LOC (1142 LOC today)
- [ ] Reduce `src/provider/openrouter.rs` below 800 LOC (1132 LOC today)
- [ ] Reduce `src/tui/app/remote/server_events.rs` below 800 LOC (1125 LOC today)
- [ ] Reduce `src/tui/app/debug_bench.rs` below 800 LOC (1124 LOC today)
- [ ] Reduce `src/tui/mermaid.rs` below 800 LOC (1116 LOC today)
- [ ] Reduce `src/update.rs` below 800 LOC (1109 LOC today)
- [ ] Reduce `src/server/client_session.rs` below 800 LOC (1094 LOC today)
- [ ] Reduce `src/provider/openai_stream_runtime.rs` below 800 LOC (1093 LOC today)
- [ ] Reduce `src/tool/mod.rs` below 800 LOC (1087 LOC today)
- [ ] Reduce `src/tui/app/state_ui_input_helpers.rs` below 800 LOC (1075 LOC today)
- [ ] Reduce `src/server/comm_session.rs` below 800 LOC (1071 LOC today)
- [ ] Reduce `src/ambient/runner.rs` below 800 LOC (1057 LOC today)
- [ ] Reduce `src/provider/cursor.rs` below 800 LOC (1043 LOC today)
- [ ] Reduce `src/cli/commands.rs` below 800 LOC (1039 LOC today)
- [ ] Reduce `src/server/debug.rs` below 800 LOC (1038 LOC today)
- [ ] Reduce `src/message.rs` below 800 LOC (1038 LOC today)
- [ ] Reduce `src/tui/app/commands_review.rs` below 800 LOC (1037 LOC today)
- [ ] Reduce `src/tui/app/navigation.rs` below 800 LOC (1014 LOC today)
- [ ] Reduce `src/tui/account_picker.rs` below 800 LOC (1012 LOC today)
- [ ] Reduce `src/goal.rs` below 800 LOC (995 LOC today)
- [ ] Reduce `src/memory_graph.rs` below 800 LOC (980 LOC today)
- [ ] Reduce `src/tui/markdown_render_full.rs` below 800 LOC (979 LOC today)
- [ ] Reduce `src/auth/claude.rs` below 800 LOC (976 LOC today)
- [ ] Reduce `src/auth/cursor.rs` below 800 LOC (970 LOC today)
- [ ] Reduce `src/browser.rs` below 800 LOC (958 LOC today)
- [ ] Reduce `src/runtime_memory_log.rs` below 800 LOC (956 LOC today)
- [ ] Reduce `src/agent/turn_streaming_mpsc.rs` below 800 LOC (945 LOC today)
- [ ] Reduce `src/cli/dispatch.rs` below 800 LOC (929 LOC today)
- [ ] Reduce `src/tui/ui_animations.rs` below 800 LOC (925 LOC today)
- [ ] Reduce `src/tui/app/auth_account_commands.rs` below 800 LOC (923 LOC today)
- [ ] Reduce `src/tui/test_harness.rs` below 800 LOC (918 LOC today)
- [ ] Reduce `src/auth/codex.rs` below 800 LOC (911 LOC today)
- [ ] Reduce `src/tui/keybind.rs` below 800 LOC (902 LOC today)
- [ ] Reduce `src/tui/ui_inline_interactive.rs` below 800 LOC (900 LOC today)
- [ ] Reduce `src/tui/ui_header.rs` below 800 LOC (897 LOC today)
- [ ] Reduce `src/server/state.rs` below 800 LOC (895 LOC today)
- [ ] Reduce `src/build.rs` below 800 LOC (892 LOC today)
- [ ] Reduce `src/tui/backend.rs` below 800 LOC (881 LOC today)
- [ ] Reduce `src/tui/login_picker.rs` below 800 LOC (878 LOC today)
- [ ] Reduce `src/sidecar.rs` below 800 LOC (872 LOC today)
- [ ] Reduce `src/tui/app/tui_lifecycle.rs` below 800 LOC (868 LOC today)
- [ ] Reduce `src/tui/permissions.rs` below 800 LOC (865 LOC today)
- [ ] Reduce `src/tui/markdown_render_lazy.rs` below 800 LOC (865 LOC today)
- [ ] Reduce `src/gateway.rs` below 800 LOC (863 LOC today)
- [ ] Reduce `src/tool/read.rs` below 800 LOC (862 LOC today)
- [ ] Reduce `src/provider/antigravity.rs` below 800 LOC (860 LOC today)
- [ ] Reduce `src/tool/apply_patch.rs` below 800 LOC (859 LOC today)
- [ ] Reduce `src/tool/bash.rs` below 800 LOC (858 LOC today)
- [ ] Reduce `src/auth/gemini.rs` below 800 LOC (849 LOC today)
- [ ] Reduce `src/tui/visual_debug.rs` below 800 LOC (847 LOC today)
- [ ] Reduce `src/setup_hints.rs` below 800 LOC (827 LOC today)
- [ ] Reduce `src/server/reload.rs` below 800 LOC (826 LOC today)
- [ ] Reduce `src/auth/copilot.rs` below 800 LOC (815 LOC today)
- [ ] Reduce `src/tui/app.rs` below 800 LOC (812 LOC today)
- [ ] Reduce `src/tui/app/remote/reconnect.rs` below 800 LOC (804 LOC today)
- [ ] Reduce `src/server/debug_swarm_read.rs` below 800 LOC (803 LOC today)

### Test concentration backlog: test files over 1200 LOC

- [x] Split test hotspot `src/tui/app/tests.rs` (was 13615 LOC; split into focused `src/tui/app/tests/*.rs` includes)
- [x] Split test hotspot `src/server/client_session_tests/resume.rs` (was 1263 LOC; split into focused `src/server/client_session_tests/resume/*.rs` includes)
- [x] Split test hotspot `src/provider/tests.rs` (was 1252 LOC; split into focused `src/provider/tests/*.rs` includes)
- [x] Split test hotspot `src/cli/auth_test.rs` (was 1226 LOC; split into focused `src/cli/auth_test/*.rs` includes)

### Long-function backlog outside already-oversized files

- [ ] Break down >100 LOC functions in `src/server/client_comm.rs` (4 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/debug_profile.rs` (3 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/comm_plan.rs` (3 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/ui_file_diff.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/session_picker/render.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/mermaid_widget.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/mermaid_cache_render.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/info_widget_todos.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/info_widget_model.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/tui_lifecycle_runtime.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/selfdev/build_queue.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_server_state.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/client_state.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/dispatch.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/background.rs` (2 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/ui_viewport.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/ui_overlays.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/ui_memory.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/ui_diagram_pane.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/session_picker/filter.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/mermaid_viewport.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/mermaid_debug.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/memory_profile.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/markdown_wrap.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/markdown_render_support.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/info_widget_layout.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/state_ui_storage.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/state_ui_maintenance.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/runtime_memory.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/run_shell.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/dictation.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/debug_script.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/debug_cmds.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tui/app/debug.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/task.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/session_search.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/selfdev/status.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/selfdev/reload.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/memory.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/grep.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/goal.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/gmail.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/conversation_search.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/bg.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/tool/batch.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/setup_hints/windows_setup.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/swarm_persistence.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/reload_state.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/headless.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_swarm_write.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_session_admin.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_jobs.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_help.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_command_exec.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/debug_ambient.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/comm_await.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/client_disconnect_cleanup.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/client_comm_message.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/server/client_comm_context.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/startup.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/openrouter_sse_stream.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/openrouter_provider_impl.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/openai_request.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/openai_provider_impl.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/provider/cli_common.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/memory_log.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/mcp/client.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/cli/selfdev.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/cli/hot_exec.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/catchup.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/bin/harness.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/agent/turn_streaming_broadcast.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/agent/turn_loops.rs` (1 oversized functions)
- [ ] Break down >100 LOC functions in `src/agent/response_recovery.rs` (1 oversized functions)

### Failure-path hardening backlog: production files with panic-prone calls

- [ ] Harden `src/tool/communicate.rs` (`unwrap`: 0, `expect`: 136, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 136)
- [ ] Harden `src/build.rs` (`unwrap`: 9, `expect`: 53, `panic!`: 2, `todo!`: 0, `unimplemented!`: 0, total: 64)
- [ ] Harden `src/provider/openai.rs` (`unwrap`: 7, `expect`: 38, `panic!`: 9, `todo!`: 0, `unimplemented!`: 0, total: 54)
- [ ] Harden `src/auth/cursor.rs` (`unwrap`: 48, `expect`: 4, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 52)
- [ ] Harden `src/auth/codex.rs` (`unwrap`: 45, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 46)
- [ ] Harden `src/server/comm_control.rs` (`unwrap`: 0, `expect`: 30, `panic!`: 11, `todo!`: 0, `unimplemented!`: 0, total: 41)
- [ ] Harden `src/cli/args.rs` (`unwrap`: 24, `expect`: 0, `panic!`: 16, `todo!`: 0, `unimplemented!`: 0, total: 40)
- [ ] Harden `src/auth/claude.rs` (`unwrap`: 28, `expect`: 9, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 37)
- [ ] Harden `src/cli/dispatch.rs` (`unwrap`: 0, `expect`: 28, `panic!`: 2, `todo!`: 0, `unimplemented!`: 0, total: 30)
- [ ] Harden `src/tool/bash.rs` (`unwrap`: 7, `expect`: 21, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 28)
- [ ] Harden `src/storage.rs` (`unwrap`: 0, `expect`: 26, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 26)
- [ ] Harden `src/tui/session_picker/loading.rs` (`unwrap`: 0, `expect`: 25, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 25)
- [ ] Harden `src/tool/read.rs` (`unwrap`: 0, `expect`: 25, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 25)
- [ ] Harden `src/auth/gemini.rs` (`unwrap`: 4, `expect`: 21, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 25)
- [ ] Harden `src/tool/apply_patch.rs` (`unwrap`: 15, `expect`: 1, `panic!`: 8, `todo!`: 0, `unimplemented!`: 0, total: 24)
- [ ] Harden `src/side_panel.rs` (`unwrap`: 0, `expect`: 24, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 24)
- [ ] Harden `src/server/client_comm.rs` (`unwrap`: 0, `expect`: 12, `panic!`: 11, `todo!`: 0, `unimplemented!`: 1, total: 24)
- [ ] Harden `src/server/reload.rs` (`unwrap`: 0, `expect`: 23, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 23)
- [ ] Harden `src/tui/session_picker.rs` (`unwrap`: 7, `expect`: 13, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 21)
- [ ] Harden `src/server/debug.rs` (`unwrap`: 0, `expect`: 18, `panic!`: 2, `todo!`: 0, `unimplemented!`: 1, total: 21)
- [ ] Harden `src/tool/goal.rs` (`unwrap`: 0, `expect`: 19, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 20)
- [ ] Harden `src/server/comm_session.rs` (`unwrap`: 0, `expect`: 20, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 20)
- [ ] Harden `src/cli/tui_launch.rs` (`unwrap`: 0, `expect`: 18, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 19)
- [ ] Harden `src/auth/external.rs` (`unwrap`: 19, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 19)
- [ ] Harden `src/provider/gemini.rs` (`unwrap`: 7, `expect`: 10, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 18)
- [ ] Harden `src/restart_snapshot.rs` (`unwrap`: 0, `expect`: 17, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 17)
- [ ] Harden `src/server/client_state.rs` (`unwrap`: 0, `expect`: 14, `panic!`: 1, `todo!`: 0, `unimplemented!`: 1, total: 16)
- [ ] Harden `src/replay.rs` (`unwrap`: 11, `expect`: 2, `panic!`: 3, `todo!`: 0, `unimplemented!`: 0, total: 16)
- [ ] Harden `src/goal.rs` (`unwrap`: 0, `expect`: 16, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 16)
- [ ] Harden `src/server/client_actions.rs` (`unwrap`: 3, `expect`: 9, `panic!`: 2, `todo!`: 0, `unimplemented!`: 1, total: 15)
- [ ] Harden `src/tui/app/remote.rs` (`unwrap`: 0, `expect`: 13, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 14)
- [ ] Harden `src/memory_graph.rs` (`unwrap`: 12, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 14)
- [ ] Harden `src/mcp/protocol.rs` (`unwrap`: 11, `expect`: 2, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 14)
- [ ] Harden `src/cli/selfdev.rs` (`unwrap`: 1, `expect`: 12, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 14)
- [ ] Harden `src/setup_hints/macos_launcher.rs` (`unwrap`: 0, `expect`: 13, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 13)
- [ ] Harden `src/server/client_lifecycle.rs` (`unwrap`: 0, `expect`: 10, `panic!`: 3, `todo!`: 0, `unimplemented!`: 0, total: 13)
- [ ] Harden `src/registry.rs` (`unwrap`: 0, `expect`: 13, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 13)
- [ ] Harden `src/tool/batch.rs` (`unwrap`: 12, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 12)
- [ ] Harden `src/server/swarm_mutation_state.rs` (`unwrap`: 0, `expect`: 8, `panic!`: 4, `todo!`: 0, `unimplemented!`: 0, total: 12)
- [ ] Harden `src/provider_catalog.rs` (`unwrap`: 0, `expect`: 12, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 12)
- [ ] Harden `src/prompt.rs` (`unwrap`: 11, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 12)
- [ ] Harden `src/tool/agentgrep.rs` (`unwrap`: 0, `expect`: 11, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 11)
- [ ] Harden `src/tool/ambient.rs` (`unwrap`: 10, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 10)
- [ ] Harden `src/soft_interrupt_store.rs` (`unwrap`: 0, `expect`: 9, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 9)
- [ ] Harden `src/server/provider_control.rs` (`unwrap`: 3, `expect`: 6, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 9)
- [ ] Harden `src/platform.rs` (`unwrap`: 0, `expect`: 9, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 9)
- [ ] Harden `src/cli/login.rs` (`unwrap`: 0, `expect`: 8, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 9)
- [ ] Harden `src/cli/commands/restart.rs` (`unwrap`: 0, `expect`: 9, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 9)
- [ ] Harden `src/tool/side_panel.rs` (`unwrap`: 0, `expect`: 8, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/tool/browser.rs` (`unwrap`: 6, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/stdin_detect.rs` (`unwrap`: 0, `expect`: 8, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/sidecar.rs` (`unwrap`: 0, `expect`: 8, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/runtime_memory_log.rs` (`unwrap`: 0, `expect`: 8, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/message.rs` (`unwrap`: 4, `expect`: 1, `panic!`: 3, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/gateway.rs` (`unwrap`: 1, `expect`: 7, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/ambient.rs` (`unwrap`: 8, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 8)
- [ ] Harden `src/server/swarm.rs` (`unwrap`: 0, `expect`: 6, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 7)
- [ ] Harden `src/server/debug_testers.rs` (`unwrap`: 0, `expect`: 7, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 7)
- [ ] Harden `src/provider/cursor.rs` (`unwrap`: 4, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 7)
- [ ] Harden `src/dictation.rs` (`unwrap`: 0, `expect`: 7, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 7)
- [ ] Harden `src/browser.rs` (`unwrap`: 2, `expect`: 5, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 7)
- [ ] Harden `src/tui/app/helpers.rs` (`unwrap`: 0, `expect`: 6, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/tool/session_search.rs` (`unwrap`: 1, `expect`: 5, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/tool/open.rs` (`unwrap`: 6, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/setup_hints.rs` (`unwrap`: 0, `expect`: 6, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/server/swarm_persistence.rs` (`unwrap`: 0, `expect`: 6, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/provider/antigravity.rs` (`unwrap`: 0, `expect`: 6, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/logging.rs` (`unwrap`: 0, `expect`: 6, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 6)
- [ ] Harden `src/tool/mcp.rs` (`unwrap`: 4, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 5)
- [ ] Harden `src/tool/conversation_search.rs` (`unwrap`: 5, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 5)
- [ ] Harden `src/telegram.rs` (`unwrap`: 5, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 5)
- [ ] Harden `src/server/debug_command_exec.rs` (`unwrap`: 0, `expect`: 4, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 5)
- [ ] Harden `src/provider/pricing.rs` (`unwrap`: 0, `expect`: 5, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 5)
- [ ] Harden `src/tui/ui.rs` (`unwrap`: 0, `expect`: 4, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `src/transport/windows.rs` (`unwrap`: 0, `expect`: 4, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `src/tool/skill.rs` (`unwrap`: 4, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `src/safety.rs` (`unwrap`: 2, `expect`: 1, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `src/login_qr.rs` (`unwrap`: 3, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `src/channel.rs` (`unwrap`: 4, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `crates/jcode-tui-workspace/src/workspace_map.rs` (`unwrap`: 0, `expect`: 4, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 4)
- [ ] Harden `src/tui/ui_messages.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/tui/ui_header.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 3)
- [ ] Harden `src/tui/login_picker.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/tui/keybind.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/tui/app/auth.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/session.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/server/comm_plan.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/cli/terminal.rs` (`unwrap`: 2, `expect`: 0, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/bin/tui_bench.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `crates/jcode-provider-openrouter/src/lib.rs` (`unwrap`: 0, `expect`: 3, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 3)
- [ ] Harden `src/video_export.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/tui/ui_animations.rs` (`unwrap`: 0, `expect`: 0, `panic!`: 2, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/tui/backend.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/tui/account_picker.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/tool/mod.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 2)
- [ ] Harden `src/server/debug_server_state.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/server/client_disconnect_cleanup.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/server/client_comm_channels.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/provider/openrouter_sse_stream.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/provider/jcode.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/perf.rs` (`unwrap`: 2, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/memory/activity.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/mcp/pool.rs` (`unwrap`: 0, `expect`: 0, `panic!`: 2, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/mcp/manager.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/copilot_usage.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/cache_tracker.rs` (`unwrap`: 2, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/auth/antigravity.rs` (`unwrap`: 0, `expect`: 2, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 2)
- [ ] Harden `src/ambient/runner.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 1, total: 2)
- [ ] Harden `src/tui/workspace_client.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/ui_prepare.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/ui_diagram_pane.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/test_harness.rs` (`unwrap`: 1, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/color_support.rs` (`unwrap`: 0, `expect`: 0, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/app/remote/reconnect.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/app/remote/input_dispatch.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/app/dictation.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/app/debug_bench.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tui/app/commands.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tool/todo.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tool/selfdev/reload.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/tool/memory.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/telemetry.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server/headless.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server/debug_swarm_read.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server/debug_session_admin.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server/comm_sync.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server/client_comm_message.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server/client_comm_context.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/server.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/provider/claude.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/provider/anthropic.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/protocol.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/plan.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/memory/pending.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/gmail.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/config.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/background.rs` (`unwrap`: 0, `expect`: 1, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `src/ambient/scheduler.rs` (`unwrap`: 1, `expect`: 0, `panic!`: 0, `todo!`: 0, `unimplemented!`: 0, total: 1)
- [ ] Harden `crates/jcode-tui-workspace/src/color_support.rs` (`unwrap`: 0, `expect`: 0, `panic!`: 1, `todo!`: 0, `unimplemented!`: 0, total: 1)

### Suppression cleanup backlog

- [ ] Remove or justify suppressions in `src/agent/turn_loops.rs` (unused_variables)
- [ ] Remove or justify suppressions in `src/auth/mod.rs` (unused_mut)
- [ ] Remove or justify suppressions in `src/cli/dispatch.rs` (deprecated, unused_mut, unused_mut)
- [ ] Remove or justify suppressions in `src/main.rs` (non_upper_case_globals, non_upper_case_globals)
- [ ] Remove or justify suppressions in `src/perf.rs` (non_snake_case)
- [ ] Remove or justify suppressions in `src/server.rs` (unused_mut, unused_mut)
- [ ] Remove or justify suppressions in `src/server/client_actions.rs` (clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/client_lifecycle.rs` (clippy::too_many_arguments, clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/client_session.rs` (clippy::too_many_arguments, clippy::too_many_arguments, clippy::too_many_arguments, clippy::too_many_arguments, clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/comm_await.rs` (clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/comm_session.rs` (clippy::too_many_arguments, clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/comm_sync.rs` (clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/debug_swarm_write.rs` (clippy::too_many_arguments)
- [ ] Remove or justify suppressions in `src/server/startup_tests.rs` (unused_mut)
- [ ] Remove or justify suppressions in `src/tui/app/remote.rs` (unused_imports, unused_imports)
- [ ] Remove or justify suppressions in `src/tui/app/state_ui.rs` (unused_mut)
- [ ] Remove or justify suppressions in `src/tui/info_widget.rs` (deprecated)

### Production `todo!` / `unimplemented!` backlog

- [ ] Remove `todo!` / `unimplemented!` from `src/tui/ui_header.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/tui/app/remote.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/tool/mod.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/server/debug_command_exec.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/server/debug.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/server/client_state.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/server/client_comm.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/server/client_actions.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/provider/gemini.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/cli/selfdev.rs` (1 occurrences)
- [ ] Remove `todo!` / `unimplemented!` from `src/ambient/runner.rs` (1 occurrences)

### Test `todo!` / `unimplemented!` backlog

- [ ] Replace test `todo!` / `unimplemented!` in `src/tui/app/tests.rs` (7 occurrences)
- [ ] Replace test `todo!` / `unimplemented!` in `src/server/startup_tests.rs` (1 occurrences)
- [ ] Replace test `todo!` / `unimplemented!` in `src/server/queue_tests.rs` (1 occurrences)
- [ ] Replace test `todo!` / `unimplemented!` in `src/server/client_session_tests.rs` (1 occurrences)

### TODO / FIXME / HACK marker backlog

- [ ] Resolve markers in `docs/CODE_QUALITY_AUDIT_2026-04-18.md` (9 markers)
- [ ] Resolve markers in `src/tui/ui_tests/prepare.rs` (5 markers)
- [ ] Resolve markers in `src/tui/ui_tests/tools.rs` (4 markers)
- [ ] Resolve markers in `src/stdin_detect.rs` (1 markers)
- [ ] Resolve markers in `docs/MEMORY_ARCHITECTURE.md` (1 markers)
- [ ] Resolve markers in `docs/IOS_CLIENT.md` (1 markers)

