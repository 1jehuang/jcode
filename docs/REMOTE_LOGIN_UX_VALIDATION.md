# Remote login UX validation

Validated on 2026-09-07 against client commit `8121bebf4`.
The installed native SSH server/CLI remained at `b601b8b` (dirty build) and
accepted the existing status, login and credential-import interfaces. No server
upgrade or restart was needed for this client-side UX change.

## Scope and acceptance boundary

The request was to make remote `/login` feel like local login and make local-login
import easy to discover. The shipped change shares the inline picker UI, obtains
status from the SSH host, and exposes explicit OpenAI/Claude import actions.
It does not promise automatic credential synchronization, importing every provider,
or validating the user's personal tokens without their approval.

The final check used the **installed shortcut launcher in an actual Kitty OS
window**, the user's normal local home, and the real VM daemon and account stores.
No synthetic home, credential fixture, mock service, alternative executable or
auth wrapper was used in that check. It reached both providers' consent screens
and cancelled before approval. This directly validates the deployed UI and its
safe pre-transfer behavior, but not a real account's successful transfer/inference.

The ten additional import integration scenarios used the real native TUI, OpenSSH,
remote CLI and daemon, but **synthetic tokens and a safety wrapper**. They support
transport, storage, cancellation and no-overwrite correctness. They are not a
substitute for personal-token validity, provider acceptance or refresh coexistence.

## Requirement-to-observation ledger

| Requirement or changed public output | Concrete check | Observed result and evidence class |
| --- | --- | --- |
| Remote `/login` uses the same navigable/filterable inline picker instead of a numbered text message | Typed `/login` in the installed launcher's real terminal. Also ran `ssh_login_picker_uses_shared_inline_ui_without_local_auth` and the nine existing `login_picker` tests. | **Actual deployed UI:** an eight-row ITEM / PROVIDER / ACTION picker appeared, with normal navigation and filter behavior. Local-picker regression tests passed. |
| Import is discoverable without memorizing flags | Inspected that real `/login` screen, then pressed Enter on `Import local OpenAI login`. | **Actual deployed UI:** OpenAI and Claude import rows were visible at the top. Enter opened OpenAI consent, not an OAuth browser flow. |
| A short import-only command is usable | Typed `/login --import-local` in the same real terminal. | **Actual deployed UI:** exactly two import rows appeared. No credential selection or copying occurred automatically. |
| Destination is clear before transferring an account | Read both providers' consent screens reached through picker selection. | **Actual deployed UI:** the SSH destination, selected provider, usable-credential warning, one-time-copy/no-sync warning, refresh-conflict warning, no-overwrite rule and exact `confirm` requirement were visible. |
| Status belongs to the VM, not the laptop | Compared the real remote `auth status --json` provider states with the installed picker. | **Actual public CLI + deployed UI:** OpenAI, Claude, Gemini, Antigravity and Copilot were `not_configured` remotely and displayed `setup`. Google was absent from that server's response and displayed `status unknown`, not a fabricated signed-out/configured state. |
| Available/expired states and status-fetch failure render correctly without changing selection | Ran `ssh_inline_picker_status_is_remote_only_and_failure_stays_unknown`. | **Injected-response regression:** available became configured, expired became attention, omitted/failed status became unknown, and the selected row remained unchanged. These branches were not observed with personal remote accounts. |
| New status transport respects the SSH host boundary and rejects unsafe/malformed status data | Ran the five `ssh_status_*` command tests and used the actual deployed status path above. | **Actual status path + unit edge cases:** hardened SSH options and `auth status --json` command passed, known-provider whitelist/fixed labels passed, missing providers stayed unknown, malformed/duplicate/unknown states were rejected, and status could not masquerade as import/login completion. |
| Typing and pasting select the action visible in the picker | Pasted `claude` as a real bracketed-paste event into `/login --import-local`, then pressed Enter. Also ran `ssh_picker_paste_selects_visible_import_action_not_browser_login`. | **Actual deployed UI:** filter selected Claude import and Enter displayed Claude import consent. It did not start browser OAuth. This caught and fixed a review-discovered legacy paste-to-OAuth bypass. |
| Escape cancels before transfer and leaves user credentials unchanged | Cancelled OpenAI consent and Claude consent with Escape. Compared local and remote selected-provider credential-file metadata before/after. | **Actual stores:** cancellation feedback said no credentials were copied. Existence, size, inode and nanosecond modification time were unchanged for both stores on both machines. The check did not read token contents. Metadata comparison is not claimed as a byte-content audit. |
| Other cancellation/private-input paths remain safe | Ran `ssh_import_all_consent_cancel_paths_are_local_and_quit_is_preserved`, `ssh_import_requires_explicit_consent_before_any_task_and_masks_private_input`, callback-privacy tests and real SSH login harness. | **Regression + real transport with synthetic callback:** `/cancel`, `/quit`, Ctrl+C, invalid confirmations and private callback input passed. Wrong-state callback was rejected by the real remote CLI before token exchange, with no transcript/local-auth leak. |
| Confirmed import transfers only the selected provider and persists privately | Ran OpenAI `import` and Claude `import` scenarios in `test_native_ssh_import.py`. | **Real interfaces with synthetic credentials:** both passed selected-only stdin transport, source isolation, private output and storage checks. This is not evidence that the user's personal provider account will accept a copied token. |
| Existing remote logins are never silently overwritten | Ran OpenAI `repeat` and Claude `repeat` scenarios against the already-imported synthetic destination stores. | **Real interfaces with synthetic credentials:** both repeat attempts were refused and preserved the destination. No personal destination was modified for testing. |
| Import success refreshes the matching remote daemon/catalog, never laptop auth state | Ran `ssh_import_success_refreshes_attached_remote_daemon_and_catalog` and `ssh_login_success_refreshes_attached_daemon_and_catalog_without_local_login_event`. | **Real protocol connection with injected completion:** received `notify_auth_changed` with the selected provider followed by `get_model_catalog`. No local authentication event was used. Real personal-provider model readiness remains untested. |
| Ordinary remote OAuth initiation/cancel still works | Ran the real SSH login acceptance suite against the installed remote ELF. | **Real CLI/SSH, isolated home:** OpenAI/Claude authorization URLs matched VM-side flow/PKCE state, scoped cancellation passed, private error handling passed, and owned children/sockets were reaped. Browser approval and successful token exchange were not performed. |
| User's shortcut actually runs the new experience | Launched the exact installed `jcode-dev tui` executable/arguments used by the shortcut in a fresh real Kitty window. | **Actual deployment:** connected to the real workspace; client reported `v0.83.6-dev (8121bebf4)`, server remained `v0.82.0-dev`. All final UI observations above used that window, not a test binary. |
| Verification does not disrupt the user's work or week-long VM setup | Quit/closed only the acceptance window. Cleaned the separately identified synthetic test daemons. Rechecked the VM shutdown timer. | **Actual deployment:** user daemon stayed running; the one-week timer stayed active at its original deadline. No personal token was copied and no provider inference was requested. |

## Reproducible supporting checks

```sh
cargo test -p jcode-tui --lib auth_remote -- --test-threads=1
# Observed: 29 passed.

# Existing compiled test binary, serial execution:
# login_picker: 9 passed; ssh_remote: 5 passed.

python3 tests/test_native_ssh_login.py --self-test
python3 tests/test_native_ssh_import.py --self-test
# Observed: 18 and 12 offline harness checks passed.
```

The live harness invocations require the explicit opt-ins and host/binary settings
documented in `NATIVE_SSH.md`. All ten import scenarios passed: picker cancel,
filtered/pasted cancel, direct cancel, import and repeat/refusal for each provider.
The separate login suite passed initiation, cancellation, privacy and lifecycle.

## Evidence artifacts

The execution host retained these task-local artifacts under `$JCODE_SCRATCH_DIR`:

- `real-login-acceptance-8121bebf4/result.json`: actual installed entry point,
  observations and explicit consent boundary.
- `real-login-acceptance-8121bebf4/picker.txt`: actual eight-row picker.
- `real-login-acceptance-8121bebf4/openai-consent.txt` and
  `claude-consent.txt`: actual destination-specific warnings.
- `real-login-acceptance-8121bebf4/cancelled.txt`: actual cancellation feedback.
- `native-ssh-import-8121bebf4.log`: ten individual integration outcomes.
- `native-ssh-login-8121bebf4.log`: actual OAuth initiation/cancel and privacy outcomes.

A debug-socket tester was also attempted. Its actual PTY rendered the picker, but
its frame API returned `no frames captured`. No successful debug-frame capture is
claimed. The later real Kitty screen reads provide the deployed visual evidence.

## Remaining unverified outcome

Actual transfer of a personal OpenAI/Claude login, provider-backed inference and
long-term refresh-token coexistence require the user to approve the specific
provider/destination consent warning. Validation stopped at that boundary without
bypassing it. The requested UI changes and pre-transfer behavior were observed
through the installed product; personal-account acceptance remains unverified.
