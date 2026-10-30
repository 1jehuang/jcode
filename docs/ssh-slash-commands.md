# Slash commands over SSH and cloud VMs

Audit date: 2026-10-30. Scope: the entire `REGISTERED_COMMANDS` registry in
`crates/jcode-tui/src/tui/app/state_ui_input_helpers.rs`, including hidden aliases.
This describes the TUI attached using `jcode --ssh`, including a cloud VM attached
through that transport. It does not claim every command is implemented on the VM.

## Safety contract

The SSH daemon owns sessions, credentials, models, tools and working-directory
operations. The laptop owns presentation and the SSH transport. A command without
an audited remote implementation must fail closed, not interpret a VM session ID
or path against laptop storage, and not become an accidental agent instruction.
Unknown skill invocations and absolute paths are ordinary remote input. Skill
resolution belongs to the daemon, not the laptop's installed skill catalog.

Enter and alternate slash submissions (including transcript send) share the same
prepared-input dispatcher. Existing wire methods are reused, with no new protocol
messages for existing handlers. **Runtime** uses the daemon's structured runtime
command API and renders its result without starting a conversation turn. Local daemon mode is distinct from SSH mode and retains its existing
behavior. Command sequences are not an atomic remote-command API.

## Capability matrix

Every registered name appears below. **Wire** means an existing request targets
the attached daemon. **Prompt** means the client creates an instruction for the
remote agent, not a synchronous operation. **UI** means presentation only.
**Blocked** means no supported SSH implementation, even if local mode supports it.
A blocked command must not silently execute against the laptop.

| Registered commands | SSH capability | Details and limitations |
| --- | --- | --- |
| `/help`, `/?`, `/commands` | UI | Help overlay and command topics describe the build, not proof of VM capability. |
| `/model`, `/models` | Wire + UI | Server catalog picker and explicit model selection. `/model refresh` requests a fresh server catalog. |
| `/refresh-model-list` | Wire | Refresh provider catalogs on the server. |
| `/provider-test-coverage`, `/model-status` | Runtime | Read persisted verification evidence from the VM, not the laptop. |
| `/agents`, `/subagent-model`, `/subagent` | Runtime | Session/global role policy and daemon-side swarm spawn. No local headed terminal is opened. |
| `/swarm-prompt` | Blocked | No remote editor workflow. |
| `/observe`, `/splitview`, `/split-view`, `/btw` | Blocked | Side-panel and child-session launch behavior is not a remote session API. |
| `/todos`, `/todo` | Wire + UI | Fetch the attached daemon's todo card. No laptop todo-file lookup. |
| `/ssh` | Blocked | Nested SSH connection setup is not supported. |
| `/remote` | Runtime | Manage the VM gateway status, pairing and config. Changes may require coordinated server restart, never automatic laptop restart. |
| `/git` | Wire | `/git` and `/git status` run fixed `git status --short --branch` through daemon input-shell, using daemon CWD. Other arguments are rejected. |
| `/colors`, `/color`, `/alignment`, `/latex`, `/thinking-display`, `/thinking`, `/reasoning`, `/compact-notifications`, `/show-agentgrep-output`, `/tool-call-details` | UI | Client presentation preferences intentionally belong to the displaying terminal. Saved settings affect the laptop UI, never VM session storage. |
| `/hotkeys`, `/keys`, `/keybindings` | UI | Inspect the displaying terminal's keys and personal hotkey usage, not VM keyboard state. |
| `/terminal-setup` | Blocked | Terminal setup lacks an audited SSH route. |
| `/commit`, `/merge`, `/commit-push`, `/commit-and-push` | Prompt | Agent executes git operations on the VM. These are not client-side git invocations. |
| `/fast-release`, `/fast-macos-release`, `/merge-remote-release`, `/remote-release`, `/cut-release`, `/commit-push-release` | Prompt | Release instructions run through the VM agent. Host tools, credentials and platform builds must exist there. |
| `/triage` | Prompt | Remote agent triage instruction. Requires the VM's GitHub access. |
| `/transcript` | Blocked | No remote transcript download/open request is implemented. |
| `/autoreview`, `/autojudge`, `/review`, `/judge` | Runtime | Daemon session policy and daemon-side review/judge swarm spawn, not laptop child-window orchestration. |
| `/effort` | Wire | Server reasoning-effort setting, with server/model capability constraints. |
| `/fast` | Wire | Session service tier. `/fast default ...` remains blocked because it persists laptop defaults. |
| `/transport` | Wire | Provider transport on the daemon, not the laptop SSH transport. |
| `/cancel` | UI + Wire | Client cancellation flag is consumed by the remote follow-up loop. |
| `/clear` | Wire + UI | Clear server conversation and discard client rendering/queued state. |
| `/cls`, `/clear-view` | UI | Clear the rendered view without dropping remote context. |
| `/rewind` | Wire + UI | History display is client snapshot, rewind N and undo target the daemon. |
| `/poke`, `/improve`, `/refactor` | Blocked | Existing loop handlers read laptop todo files or persist local session metadata. A remote loop API is still needed. |
| `/plan` | Prompt | Plan instruction sent to the VM. Can interrupt an active turn. |
| `/compact` | Wire | Compaction and session compaction-mode changes happen on the daemon. |
| `/fix` | Blocked | Local recovery state is not authoritative for VM sessions. |
| `/voice`, `/dictate`, `/dictation` | Blocked | Local microphone/process execution is intentionally unavailable in SSH mode. |
| `/memory`, `/swarm` | Wire | Session feature toggles go to the daemon. |
| `/test` | Prompt | Verification instruction is staged for the remote tick loop, not a laptop test process. |
| `/initiatives`, `/goals` | Runtime | Explicit retired-feature notice, preserving old records. Does not resurrect obsolete tracking. |
| `/overnight` | Runtime | Daemon-owned coordinator, status, log, review and cancellation scoped to the attached session. |
| `/context`, `/skills`, `/info` | Runtime | Server-owned context, skills and session data. Remote skill invocation also remains available. |
| `/version`, `/changelog` | UI | Client build/changelog information. Version display labels cached server version separately. |
| `/reset`, `/usage`, `/subscribe` | Blocked | No audited SSH quota-reset/usage/browser subscription route. |
| `/subscription` | Runtime | VM subscription credentials and live account state when configured. |
| `/productivity`, `/wrapped` | Runtime | Generate the VM's report, not laptop usage data. |
| `/telemetry` | Runtime | Inspect/change runtime-host telemetry preferences. |
| `/feedback`, `/support` | Blocked | Laptop diagnostic and external-send workflows have no audited SSH route. |
| `/config`, `/log` | Blocked | Local config/log files are not VM config/logs. |
| `/diff` | UI | Changes rendering mode only, not a filesystem diff request. |
| `/onboarding-preview`, `/onboarding-sim` | Blocked | Local onboarding simulation and state. |
| `/reload`, `/restart`, `/rebuild`, `/selfdev`, `/update`, `/update-sim`, `/update-rehearsal`, `/client-reload` | Blocked | Do not inspect/build/update/restart the laptop in response to VM operations. |
| `/server-reload` | Wire | Explicit request to reload the remote daemon. Requires an available remote binary. |
| `/resume`, `/sessions`, `/session`, `/active`, `/catchup`, `/back`, `/save`, `/unsave` | Blocked | Local session pickers/bookmarks are not remote session management. Reconnect with `--ssh HOST --resume REMOTE_ID`. |
| `/rename` | Wire | Rename attached server session, including `--clear`. |
| `/fork`, `/split`, `/desktop`, `/transfer`, `/workspace` | Blocked | Child-session/window/handoff paths can invoke local launch and storage behavior. A wire split method alone does not make the response handling remote-safe. |
| `/cloud`, `/local` | Coordinated handoff | Cloud orchestration is explicit. Moving an already remote session to cloud is rejected. `/local` coordinates return, not an arbitrary local command fallback. Requires configured cloud authority and successful handoff checks. |
| `/quit` | UI | Detach/exit client without overwriting the server's session file. |
| `/auth` | Runtime | Credential readiness on the VM, without returning secrets. Not a live provider request. |
| `/logout` | Runtime | Requires `/logout <provider> confirm`. Unconfirmed requests only explain the cleanup scope. Does not revoke external credentials, and live provider instances may retain in-memory credentials until reconnect. |
| `/login` | Remote login | Dedicated SSH login flow. Never use the laptop provider login picker as fallback. |
| `/account`, `/accounts` | Remote account flow | Native remote account operations use the daemon account protocol. Never enumerate laptop account storage. See the account UI's explicit unsupported-operation notices. |
| `/cache` | Runtime | VM provider usage and Anthropic TTL setting. No cache eviction/prewarming or precise expiry guarantee. |
| `/debug-visual`, `/screenshot-mode`, `/screenshot`, `/record` | Blocked | Debug/capture facilities are local host operations or lack audited SSH handlers. |
| `/continue`, `/resumeall`, `/resume-all` | Wire | Resume interrupted daemon sessions using `ResumeAllSessions`, not laptop session enumeration. |
| `/z`, `/zz`, `/zzz`, `/zstatus` | Runtime | VM Copilot premium mode. Preserves normal/toggle-one/toggle-zero/status semantics without laptop config writes. |

## Explicit remaining limitations

- A connected wire request is not a negotiated capability guarantee. Older daemons
  can reject newer requests. Errors should remain visible rather than triggering
  a local implementation.
- Remote session listing, bookmarks, transcript/file opening, child-window launch,
  account file browsing and arbitrary persistent settings do not become available merely because
  similarly named local commands exist.
- The registry is a discoverability list, not a remote capability allowlist. Any
  newly registered but unaudited command is rejected by the SSH local fallback.
- Prompt commands can perform meaningful git/release work on the VM and retain
  the normal agent permission and approval behavior. They do not guarantee that
  the VM has the required software, authentication or a supported target OS.
- Host-isolation regressions are tested with an isolated child process and fake
  transport. This does not replace a real SSH VM integration test.
