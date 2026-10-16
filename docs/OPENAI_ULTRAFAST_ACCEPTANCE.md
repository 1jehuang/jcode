# OpenAI Ultrafast acceptance, October 16, 2026

## Scope

This is an observed real Jcode executable and native Unix-socket session-control
workflow, not a mocked provider or copied implementation. No model generation,
message, or tool request was submitted, so this does **not** prove upstream
Ultrafast entitlement, billing, or throughput. The normal OpenAI provider and
credential stores were used. Credentials and account identities are omitted.

Implementation commits: `c4335e422` (Sol tiers and safeguards), `2e980f7e6`
(public RuntimeCommand Fast aliases). Runtime alias dispatch was additionally
checked by the hosted app-core regression test
`runtime_commands_read_actual_state_without_inference`, passed in hosted run
`20261016-142444857-125063` after hosted check `20261016-142434189-122090`.

## Build provenance

The first plain `justrust build --profile selfdev` chose local execution, as
confirmed by `justrust show 20261016-142312123-100345`. Its embedded version was
stale because development build metadata intentionally does not rerun for
every Git commit. It was not used as final acceptance evidence.

An explicitly hosted build was then attempted:

```bash
JCODE_BUILD_GIT_HASH=$(git rev-parse --short HEAD) \
JCODE_BUILD_GIT_DIRTY=false \
justrust build --remote --profile selfdev --bin jcode
```

Hosted compilation succeeded in 21.8 seconds, run
`20261016-142514110-131517`, but artifact retrieval stalled. At the 120-second
no-output watchdog, the local executable was still the old version, an owned
SSH child was waiting, and no local decompressor was running. The agent
cancelled that owned task after approximately 166 seconds total, without
killing unrelated SSH connections. This is a build-artifact retrieval blocker,
not a compile failure. No hosted artifact was claimed runnable.

A local build was necessary to obtain the executable for real acceptance:

```bash
JCODE_BUILD_GIT_HASH=$(git rev-parse --short HEAD) \
JCODE_BUILD_GIT_DIRTY=false \
justrust build --local --profile selfdev --bin jcode
```

Run `20261016-142815723-166769` passed in 18.0 seconds. The executable reported
`jcode v0.98.15-dev (2e980f7e6)`. Its SHA-256 was
`66a2b26dc6f49bf107778015411252abdc9d312d1d7907768ecaf373eb5a88e1`.

## Isolated real runtime

The startup command was the following, where `$ACCEPTANCE_SOCKET` was a unique
scratch directory Unix socket, not the shared daemon socket:

```bash
JCODE_OPENAI_SERVICE_TIER=off ./target/selfdev/jcode \
  --no-update --no-selfdev --provider openai --model gpt-6.1-sol \
  --socket "$ACCEPTANCE_SOCKET" serve
```

A Python Unix-socket client sent newline-delimited native public requests.
It awaited the terminal correlated result, not only the preliminary `ack`.
The final successful run was background task `180268g9t7`, 11.45 seconds,
exit zero. Responses below contain only non-sensitive selected fields.

| Request | Actual terminal result |
| --- | --- |
| `subscribe` with scratch working directory and `selfdev:false` | `ack` |
| `set_model`, model `gpt-6.1-sol` | `model_changed`, model `gpt-6.1-sol`, no error |
| `runtime_command`, command `fast`, args `["ultra"]` | `runtime_command_result`, provider `OpenAI`, service_tier `ultrafast` |
| `runtime_command`, command `fast`, args `["status"]` | `runtime_command_result`, service_tier `ultrafast` |
| `runtime_command`, command `fast`, args `["off"]` | `runtime_command_result`, service_tier `null` |
| `get_model_catalog` | `history` with actual available models, catalog not reproduced |
| `set_model`, model `gpt-5.6-luna` selected from that catalog | `model_changed`, model `gpt-5.6-luna`, no error |
| `runtime_command`, command `fast`, args `["ultra"]` | `error`: `Ultrafast is not supported for OpenAI model 'gpt-5.6-luna'` |

Exact example of the real command and selected result:

```json
{"type":"runtime_command","id":3,"command":"fast","args":["ultra"]}
{"type":"runtime_command_result","id":3,"command":"fast","data":{"provider":"OpenAI","service_tier":"ultrafast"}}
```

An earlier client attempt incorrectly treated a `set_model` acknowledgment as
its final result. The client was corrected to await `model_changed`. Another
attempt proved Sol ultra/status/off but failed to select the assumed
`gpt-5.5`, which was not available to the actual account. The final client used
the real account catalog and successfully selected `gpt-5.6-luna` for the
unsupported-tier check. No catalog fixture or credential bypass was introduced.

Every acceptance-owned daemon was terminated in `finally` and waited for.
No binary was installed, no shared daemon was restarted, and no push occurred.
The normal session store may retain the empty acceptance sessions. No shared
session store or user session was deleted.

## Remaining limits

This verifies the real OpenAI session-control path only. API-key and OAuth
wire payload serialization and request-model gating are covered by regression
tests, but no live paid API completion or OAuth entitlement request was made.
Ultrafast badge/hotkey ladders have model-gate regression coverage, not an
interactive screenshot acceptance check here. The documented API price and
Codex plan restrictions still apply.
