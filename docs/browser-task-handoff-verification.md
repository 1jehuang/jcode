# Jev whole-task handoff verification

## Scope and reproducible run

Verified 2026-09-20 against implementation commits `f28e346a7` and `c5882da59`,
plus the verification-only assertions committed with this report. No production
behavior changed after the successful deployed GitHub checks.

The requested outcome is a single handoff that gives Jev the task and context,
provides available browser actions, executes its choices, feeds back observations
and results, and repeats until completion or a genuine blocker. This report does
not claim that the earlier LinkedIn research or Amazon cart tasks were completed.

Final coordinated command:

```sh
cargo test -p jcode-app-core --lib browser -- --nocapture
BROWSER_SESSION=<existing-session> python3 scripts/test_browser_handoff_live.py --tab-id <disposable-tab>
```

Observed run `2059807twx`, exit 0 at 09:40:32 UTC:

- 82 regression tests passed, 0 failed, 6 opt-in tests ignored by the first command.
- The live runner then explicitly ran and passed five opt-in cases with real Jev,
  the freshly compiled `BrowserTool::execute` public interface, the browser bridge,
  real Firefox DOM execution and caller-owned loopback pages.
- The subscription-only smoke case was not run. Live decisions used OpenRouter.
- Earlier complete rerun `163628unon` also passed all five live cases.

Local full logs: `/tmp/jcode-bg-tasks/2059807twx.output` and
`/tmp/jcode-bg-tasks/163628unon.output`. These are ephemeral, not repository artifacts.
The concrete outcomes and named assertions below are the durable evidence summary.

## Explicit requirements mapped to observations

Test names below are in `crates/jcode-app-core/src/tool/`.

| Requirement | Concrete check | Observed result |
| --- | --- | --- |
| Delegate the whole task, not a single action | `live_browser_handoff_completes_search_and_nested_navigation` in `browser_fast_live_tests.rs` | One public call completed search/type+submit (0.98), Documentation click (0.96), nested-panel scroll (0.99), Browser controls click (0.92), then `done`. Final observed text was `Browser controls Whole browser task verified`. No parent calls between actions. |
| Give Jev the original task and context throughout | `entire_task_keeps_context_and_results_across_three_actions` in `browser_fast_tests.rs`, plus `task_contract_preserves_evidence_and_distinguishes_trust` in `browser_jev.rs` | Four successive decisions received the identical original goal/context. Serialized Jev instructions contained the goal, and the serialized state retained context and evidence. All assertions passed. |
| Browser controller supplies available actions | Same three-action regression and `uses_decisions_protocol_not_chat_completions`, `action_menu_is_not_duplicated_in_bounded_task_requests` | Every cycle offered labeled action IDs plus done/handback/script/text choices. Wire criteria matched the authoritative menu without duplicating it in state. Live Jev selected offered actions. |
| Execute, append result/context, and consult Jev again | Same three-action regression and `exact_actions_execute_once_and_results_feed_next_decision` | History lengths were 0, 1, 2, 3. Every retained action included actual successful result metadata and before/after page evidence. Fourth request saw the finished page and the first page's evidence. Exact action results reached the next decision and were not reoffered. |
| Continue across navigation until the actual goal is done | `live_browser_handoff_completes_local_navigation`, whole-task live case, and deployed GitHub checks below | Two link clicks in one call ended with `Fast browser integration verified`. The more complex case did not stop after search, first link, or scroll. GitHub stopped on the actual contribution file with policy visible, not the repository landing page. |
| Return to parent only at a real boundary | `live_browser_handoff_requests_script_and_resumes`, `requests_main_agent_script_or_text_without_executing`, sensitive live case | Missing script produced `hand_back`, `requested_help=script`, zero actions. Supplying an exact candidate completed with one eval, verified title, and null help. Missing text is covered by the deterministic regression. Password fixture returned sensitive handback with zero actions. |

## Changed public contract and integration boundaries

| Public input/output or changed behavior | Concrete check | Observed result |
| --- | --- | --- |
| Optional trusted `context`, 12,000-byte runtime bound | `handoff_context_is_optional_and_deserializes`, `handoff_schema_exposes_task_context_and_extended_budget`, `validates_inputs_before_browser_calls` | Missing/null context remains compatible, supplied context deserializes, schema exposes limit and trust distinction. Oversized context hands back before any browser call. |
| Default action budget 40, bounds 1 through 100 | Schema test above, three-action regression, `validates_inputs_before_browser_calls`, `budget_always_observes_last_action` | Schema matches limits, per-cycle remaining actions were 40/39/38/37, 0 and 101 rejected before browser calls, final budgeted action still observed before handback. |
| Confidence threshold permits safe observation but not uncertain interactions/completion | `low_confidence_scrolling_gathers_evidence_without_parent_intervention`, `uncertain_click_is_reconsidered_after_safe_exploration`, `low_confidence_exact_scroll_and_completion_still_hand_back` | Low-confidence automatic scroll succeeded. Uncertain click was not executed and was replaced by an observation-only decision. Exact caller scroll and done remained blocked with an explicit low-confidence reason. Both real GitHub successes used scroll below 0.8 and click at 1.0. |
| Public JSON text and metadata agree, with structured `status`, `reason`, `model`, `decision_provider`, `requested_help`, `action_trace`, `final_observation` | `live_browser_handoff_completes_search_and_nested_navigation` and `live_browser_handoff_completes_local_navigation` | Public text equaled metadata. Status was done, model was `typesafe/jev-1.13`, provider/reason were present, help was null on success. Each live step had executed status, confidence, before/after URL evidence and an object result. Final observation contained the required completion text. |
| New trace result evidence and bounded retention | `rolling_action_results_keep_newest_and_bound_history`, `oversized_history_compacts_old_evidence_then_entries_preserving_newest`, `newest_result_outlives_its_oversized_page_snapshots` | Newest 15KB result remained available over 100 actions, retained result budget stayed bounded, old evidence compacted before latest results, omission notice warned against repeating side effects, final serialized wire request fit 80KiB. |
| Exact capabilities are executable, one-shot and retired across document changes | Live script resumption, `exact_actions_execute_once_and_results_feed_next_decision`, `navigation_retires_old_exact_actions_but_continues_task` | Live script ran once and returned `Jev hybrid verified` in result metadata and final title. Capability manifest identified trusted caller action, disappeared after use, and old candidates did not survive navigation. |
| Main-content observations and nested scrolling feed actual browser execution | Whole-task live fixture with unrelated header links, `nested_scroll_uses_container_delta_without_escaping_scope`, `nested_scroll_actions_are_scoped_and_directional` | Jev found search and documentation despite header clutter. The real container scrolled, revealing the final link. Bridge mapping uses container delta rather than merely scrolling the element into view. Scope/direction assertions passed. |
| Search only uses supplied text and stays in scoped browsing context | Whole-task live case, `search_submission_requires_search_observation_and_caller_text`, `links_allow_self_but_not_other_browsing_contexts` | Actual GET search submitted supplied `browser controls`, reached results, and continued the task. Search eligibility and target restrictions passed candidate checks. |
| Scoped tab/frame/window and no recursive handoff/setup | `rejects_scope_escapes_and_recursive_actions`, `whole_tab_actions_cannot_escape_explicit_subframe`, `mismatched_window_is_rejected_before_observing_page`, `raw_commands_get_authoritative_scope` | Scope-escaping payloads rejected. Mismatched window stopped before page observation. Raw commands received authoritative tab/frame scope. |
| Fresh target/page evidence rather than stale execution or stale done | `stale_dom_replans_without_executing_old_target`, `identical_url_and_target_on_new_document_requires_replan`, `done_checks_latest_dom_and_returns_changed_observation` | Stale target was not executed. Same-URL document replacement forced replanning. Completion used a fresh observation. |
| Navigation/disconnect recovery does not replay uncertain side effects | `initial_and_predecision_observation_disconnects_are_retried_read_only`, `navigation_disconnect_is_observed_without_repeating_click`, `uncertain_exact_side_effect_is_not_replayed_or_assumed_successful`, `delayed_navigation_settles_before_second_model_decision` | Read-only retries recovered observations. Click was not repeated. Opaque exact-action uncertainty handed back rather than claiming success. Delayed navigation settled before another decision. |
| Safe credential/result handling and ordinary compatibility | `sensitive_pages_never_reach_transport`, `sensitive_action_results_do_not_enter_next_decision`, `structured_credentials_are_redacted_even_in_encoded_results`, sensitive live case, `ordinary_click_preserves_existing_bridge_dispatch`, `nullable_handoff_options_do_not_break_direct_actions` | Sensitive observations/results were withheld from further Jev decisions, structured/encoded secret fields redacted, live sensitive page caused zero actions. Existing direct-click dispatch and nullable options passed. |
| Bounded stopping and meaningful progress | `decision_timeout_is_structured_handback`, `timeout_and_cancellation_drop_pending_work`, `stall_returns_control`, `distinct_successful_actions_on_unchanged_page_do_not_stall` | Timeouts/cancellation stopped pending work, repeated unproductive action handed back, distinct successful actions on an unchanged observed DOM did not falsely stall. |

## Real-site feedback: demonstrated improvement

Same task and read-only context, default interaction confidence 0.8, max 15 actions:
find/open `CONTRIBUTING.md` in `1jehuang/jcode`, and finish only with the actual
contribution guidelines visible. No exact scripts, site-specific action candidates,
or parent-directed clicks/scrolls were supplied.

| Iteration | Observation |
| --- | --- |
| Before exploration fix, 09:33 UTC | Handback with zero actions: proposed scroll confidence 0.34 was below 0.8. |
| Scroll-only exception, 09:34 UTC | Handback with zero actions: tentative `.github` click confidence 0.41. Unsafe guessing remained blocked but the task could not gather more evidence. |
| Deployed safe exploration retry, 09:37 UTC | One handoff scrolled at 0.70, clicked `CONTRIBUTING.md` at 1.0, returned done with actual PR policy visible. |
| Fresh repository tab, 09:40:16 through 09:40:21 UTC | Repeated single handoff scrolled at 0.74, clicked the contribution file at 1.0, returned done. No parent intervention during the task. |

Both successful final observations had URL
`https://github.com/1jehuang/jcode/blob/master/CONTRIBUTING.md` and text stating
that pull requests from everyone are welcome and review is based on correctness,
tests, security, architecture, maintainability and project fit rather than author
status. Only read-only navigation and observation actions were executed.

This is observed behavioral improvement, not a claim based only on source inspection
or aggregate test counts. It is not a statistically powered reliability benchmark.

## Remaining limits

- Only Firefox and the active OpenRouter Jev route were exercised live. The
  subscription-only route was not independently exercised in this pass.
- Goal understanding/completion still depends on a typed-choice model. These
  results do not prove reliable behavior on arbitrary sites or adversarial pages.
- Some race, scope and redaction cases use deterministic fixtures rather than
  reproducing every condition on public websites. Their evidence is identified above.
- The ten-minute outer task deadline was not tested by spending ten minutes. Bounded
  timeout/cancellation primitives and structured timeout behavior were tested.
- Arbitrary scripts and missing typing text still require explicit parent capability
  input. Handoff returns trace and observations, not a free-form research report.
- No shopping mutations, payments, account changes, LinkedIn outreach, or GitHub
  writes were part of this verification.

## Post-commit whole-result rerun

Run `312075g2ku` completed at 09:42:18 UTC with exit 0 on the committed
implementation and tests. It again passed all 82 regressions and all five live
scenarios. An explicit reconciliation found all 44 named mapped tests passing
and each of the 19 requirement/public-contract rows linked to fresh passing checks.
The fresh live task executed `type → click → scroll → click`, then returned `done` with
`Browser controls Whole browser task verified`. JSON/metadata, result history,
scope, safety and confidence assertions all passed again. No implementation or
test changes were made during this rerun.
