# Remote compilation validation

## Observed checks

- `cargo check -p jcode-app-core --no-default-features`: passed.
- `cargo check -p jcode --no-default-features`: passed.
- `cargo test --lib -p jcode-app-core --no-default-features compile_remote`:
  26 passed (12 client/transport, 13 source snapshot, 1 agent schema regression).
- `cargo test -p jcode-tool-core subcall_ids_are_parent_scoped_and_retry_stable`:
  passed.
- `cargo test --lib -p jcode-app-core --no-default-features tool::batch`:
  15 existing batch regressions passed.
- New Rust modules pass targeted `rustfmt --check`. `git diff --check` passed.

## Requirement-to-evidence ledger

| Requirement | Concrete evidence | Boundary |
| --- | --- | --- |
| Signed-out/non-subscriber guidance, without pretending offline means unpaid | `authenticated_access_transport_and_fail_closed_states`, `explicit_compute_entitlement_overrides_promotional_budget_and_capability` | Real HTTP transport against synthetic account responses |
| Harness updates subscription guidance even with locked/deferred schemas | `compile_remote_account_guidance_refreshes_locked_and_deferred_snapshots` | Actual agent definition path with a controlled tool |
| No snapshot or upload on missing/revoked access, including stale cached success | `execute_denied_access_precedes_snapshot_and_upload_even_with_cached_ready` | Actual tool execution and captured loopback requests |
| Upload current edits/untracked source while omitting common credentials, ignored outputs and symlinks | Source snapshot tests use real temporary Git repositories, tracked secrets and symlink fixtures | Unix filesystem and Git behavior, not a complete secret detector |
| Bound files, paths, listing, total source and result bodies | Thirteen source tests plus `responses_are_bounded_with_content_length_and_chunked_encoding` | Boundary tests include exact allowed sizes and chunked oversized responses |
| Authenticated remote command submission returns actual compiler diagnostics | `execute_uploads_selected_worktree_and_loopback_compiles_real_source` uploads source through the tool to a loopback service which runs real `rustc` | Real compiler, but **not** cloud isolation or E2B provisioning |
| Preserve nonzero build exits and measured usage | `submit_preserves_nonzero_exit_output_and_metered_usage` | Typed protocol fixture, including cleanup confirmation |
| Expose shared balance/recent jobs without inventing a zero balance on service failure | `status_never_snapshots_and_invalid_timeout_never_contacts_service`, `compute_credits_rejects_unknown_units_and_malformed_balances` | Account usage HTTP fixtures, not a live balance |
| Distinguish exhausted credits from missing subscription | `server_admission_errors_distinguish_credits_subscription_and_uncertain_jobs` | HTTP 402/403 and ambiguous job error fixtures |
| Do not leak source or bearer credentials through redirects | `redirects_never_forward_credentials_or_source`, `endpoint_rejects_insecure_or_credential_bearing_destinations` | Two loopback servers plus destination-validation cases |
| Repeated batch positions do not collide in billing job IDs | `subcall_ids_are_parent_scoped_and_retry_stable` | Public context API, distinct parents and stable same-parent retries |

## Server-side validation

The companion `solosystems-backend/workers/api` changes contain real-SQLite
credit-ledger tests and endpoint tests using the pinned E2B SDK with controlled
sandbox responses. They cover shared `remote_compile`/`cloud_agent` balances,
atomic account/global admission, idempotent grants and jobs, rounding, failed
build charges, expiry, rollback, uncertain cleanup retaining capacity, invalid
uploads and bounded outputs. See its `REMOTE_COMPILE.md` and `COMPUTE_CREDITS.md`
for current commands and deployment requirements.

The backend's retained `test/compile-runtime-smoke.mjs` exercises the actual
local workerd runtime and D1 through the full handler with a controlled E2B
boundary. A 2 MiB minus one byte file, an exact 2 MiB file, and an exact 20 MiB
source snapshot (27,962,741 encoded bytes) all returned HTTP 200 and settled
credits. An approximately 9 MiB request containing three million empty objects
returned HTTP 400 before object-graph parsing. These checks exercise real Worker
memory constraints and native base64 decoding, but not a live cloud machine.

Independent review found and corrected two important integration issues:
reused batch child IDs causing permanent job conflicts, and JSON graph expansion
before file-count validation. Maximum-size upload parsing also has boundary
regressions in the server tests. Unconfirmed sandbox cleanup must retain its
reservation and concurrency slot until confirmed termination or conservative
expiry, not merely settle a full charge early.

## What was not exercised

No live paid E2B sandbox, production database migration, deployment, credit grant,
subscription purchase, or cloud-agent runner was started. The current daemon was
not replaced for this feature. Cloud execution and actual billed machine lifetime
still require an operator-provisioned, reviewed compiler template and approved
credit allocation/rates before rollout. Persistent caches and artifact download
are deliberately outside this initial implementation.
