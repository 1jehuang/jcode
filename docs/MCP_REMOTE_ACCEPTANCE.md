# Native MCP acceptance replay

## Scope and identity

The implementation under test is commit `c049568fb5cdebf5894199e04c41b1e760601599`,
including native HTTP/SSE and OAuth from `5cabc521e` and the total remote
handshake deadline from `909ea0509`. Verification ran on October 13, 2026,
using a clean detached scratch worktree, not the mutable active checkout or
shared daemon. The daemon was not restarted. User MCP configuration and
credentials were not changed. The actual Expo check used an existing isolated
validation credential store, not the user's active MCP configuration.

The final binary was built from that pinned tree before the negative controls.
Its SHA-256 is:

```text
0e9004a88fc4f3f6fad00613d6f5d37604e83267448b82f43ce86ac386a38bcd
```

All mapped checks below ran on the pinned result. The public CLI fixture also
ran again after packaging as `scripts/tests/mcp_cli_acceptance.py`, with stronger
unauthenticated-response assertions. Its SHA-256 for this replay is:

```text
75b853a3d6764bc3e5ba9c6c7467cd2a275d6521b54a9f06ad7ffb3872007009
```

## Requirement-to-observation matrix

Suite labels B, M, A, C and S refer to the execution ledger below. F refers to
the ten-case public CLI fixture. These are observed passing assertions, not
conclusions from source inspection alone.

| ID | Requirement or changed public output | Executed check | Observed result |
|---|---|---|---|
| R1 | Native Streamable HTTP initialize, list, call, refresh and shutdown | B `public_json_connect_list_call_refresh_and_shutdown`, `public_post_sse_fragmentation_notifications_and_multiline_json`; F authenticated HTTP | Passed JSON and fragmented POST-SSE correlation and echo results. CLI received two sorted tools, propagated session/protocol headers and sent session DELETE. Real Expo HTTPS initialize/list/shutdown returned 39 tools. |
| R2 | Legacy SSE and HTTP-to-SSE compatibility | B `public_legacy_and_http_fallback_workflows`; F explicit legacy and HTTP405 fallback | Both CLI cases passed through native SSE, with initialize, initialized notification and tools/list in order. F observed the 405 HTTP probe before the SSE GET. No stdio wrapper was used. |
| R3 | Concurrent calls, bounded queues and cancellation | B concurrent-call, backpressure, caller-cancellation, shutdown/drop and endpoint-timeout tests | Passed response correlation, queue timeout and stream/waiter cancellation assertions over real TCP. |
| R4 | Invalid-session recovery without replaying side effects | B session-404, owned-manager and shared-manager/pool replacement tests | Passed: invalidated requests failed, subsequent calls used fresh connections, failed tool calls were not replayed. |
| R5 | Remote configuration and static header precedence | B protocol tests, C CLI tests, F static-header case | Passed transport/URL/header parsing. A case-insensitive static Authorization header worked despite a malformed OAuth store, which remained unchanged. Offline list reported static authorization. |
| R6 | Explicit OAuth discovery, DCR, PKCE, callback, storage and logout | B OAuth suite; F login, stored-token, authenticated-test and logout cases | Public CLI followed real metadata/DCR/authorization/token/callback HTTP requests. Fixture verified S256 verifier, resource, client, callback and grant binding. Token file was mode 0600. Authenticated test worked, logout removed the token and next test stopped at initialize 401. |
| R7 | No interactive authentication during ordinary connect | B OAuth/error tests; F before/after-login-denial cases | CLI exited nonzero with actionable login guidance after exactly one unauthenticated initialize POST receiving 401. No discovery, authorization, token or guarded browser command occurred. Measured 34.3ms before login and 27.9ms after logout. |
| R8 | OAuth refresh security and credential races | Full B OAuth suite | Passed expiry/rejection, resource binding, corrupt-store, refresh serialization and logout-generation assertions. Concurrent refresh/login did not resurrect logged-out credentials. |
| R9 | Session registration does not wait for network | M/A `remote_startup_does_not_wait_and_publishes_fast_tools_before_slow_peer`, owned and shared | Registration completed while slow initialize remained gated. Pinned replay cold registration measured 133us owned and 132us shared. Fast proxy became usable before slow release. |
| R10 | Warm cached schemas and empty live-catalog reconciliation | M/A warm-startup portion and `remote_empty_catalog_replaces_obsolete_cached_schemas` | Cached schemas were immediately available before fresh handshakes. Unchanged explicit-eager warm snapshot stayed byte-equivalent. Empty live catalog replaced the stale disk schema, so next startup could not advertise removed echo. |
| R11 | Total remote handshake deadline independent of tool timeout | B `remote_handshake_deadline_is_independent_of_long_tool_timeout` | With a real TCP peer and paused clock, `timeout_secs=3600` still failed at the total 30-second connection deadline and closed the socket. Same check failed when the cap was removed, then passed when restored. |
| R12 | Agent tool wait isolation | M/A gated slow/fast test plus management dispatch tests | Management list, MCP search and actual fast echo each completed within one second while slow initialize remained gated. Echo output matched its argument. |
| R13 | Native/fallback deferred prompt-prefix and cache stability | M/A `remote_connect_disconnect_reconnect_preserves_prompt_and_cache_baselines`, native/fallback provider x auto/deferred; broader A suite | All four combinations passed real HTTP connect/disconnect/reconnect and echo. Eager schemas, static prompt, existing messages, cache tracker, KV monitor and eager-prefix telemetry stayed unchanged. |
| R14 | KV telemetry represents the cached eager prefix | M/A `remote_deferred_tool_changes_do_not_change_cache_telemetry` | Deferred add/refresh/remove did not change tool hash/count/JSON size. An actual eager-schema edit still changed the signature. Counterfactual all-tools hashing failed with count 2 to 3 and size 174 to 330. |
| R15 | Headless wait excludes disabled servers and uses exact identity | C three `mcp_startup_tests` invoking actual cold-wait helper | Disabled-only and registered normalized/collision-suffixed server cases returned before 100ms. Cold/changed fingerprints remained cold. Both readiness checks timed out when disabled filtering and exact identity were removed. |
| R16 | Management connect, refresh, disconnect, reconnect and aliases | Full M remote-management/collision tests | Real HTTP management connect/discover/call/disconnect passed. URL/transport/header fields preserved, rejected ambiguous options did not silently connect, aliases preserved original server identity, disconnected tools disappeared and reconnect worked. |
| R17 | Redirect, cross-origin, protocol and response safety | Full B remote/OAuth/protocol suites | Passed credential-bearing redirect and cross-origin endpoint rejection, unsupported protocol rejection, malformed/oversized reply failure and initialization/notification error handling. |
| R18 | Existing stdio compatibility | B stdio/manager/protocol tests and S real stdio collision integrations | Local stdio clients still connected, dispatched exact collision-safe identities and disconnected. The integration parent and isolated child passed, exercising two actual stdio server processes. |

## Startup and wait measurements

The gated startup check measures registry registration, not total application
startup. On the hosted pinned replay:

| Server ownership | Cold registration | Warm registration |
|---|---:|---:|
| Owned | 133us | 270us |
| Shared | 132us | 241us |

Both owned/shared cases returned while initialize was gated. Warm schemas were
available synchronously. The same test was rerun after restoring the negative
controls, observing owned 133us/237us and shared 96us/247us cold/warm respectively.
These are single-run observations, not a cross-machine speedup claim.

More importantly, the slow peer could not delay fast proxy publication,
management/search or echo execution. Removing incremental publication made the
fast proxy wait exceed the test's two-second bound. Restoring the fix passed
without releasing the slow peer first.

## Counterfactual regression replay

After preserving the final binary, only the relevant safety fixes were removed
in the detached scratch worktree. Tests themselves were unchanged. These are
minimal negative controls, not a claim that the entire historical revision was
rebuilt. The counterfactual tree passed a complete compile check before testing,
so compilation failure cannot masquerade as a successful regression detection.

| Removed fix | Observed negative control | Observed restored final behavior |
|---|---|---|
| Incremental connection publication | Fast proxy publication exceeded two seconds while slow initialize was gated | Owned/shared startup test passed, with fast proxy and echo usable before slow release |
| Total remote handshake cap | Required `connection timed out after 30s` assertion failed with long tool timeout | Same real-TCP paused-clock test passed with total 30-second timeout and socket close |
| Eager-only telemetry projection | Deferred addition changed `(hash,count,size)` from `(17620846556523646203,2,174)` to `(16567676142665456365,3,330)`; real lifecycle signature also changed | Deferred telemetry and all native/fallback auto/deferred lifecycle combinations passed unchanged-prefix assertions |
| Disabled filtering and exact-identity readiness | Cold list incorrectly contained disabled server; disabled-only and collision-suffixed cases exceeded 100ms | All three headless helper regressions passed |
| Empty live-catalog cache update | Removed echo persisted in disk cache and next-startup assertion failed | Empty live catalog replaced stale cached schema and test passed |

Negative test commands exited 101 with the expected assertion failures: one
base deadline test, four app-core regressions and three headless regressions.
All eight identical tests passed after exact restoration. `git diff --exit-code`
then verified the scratch source was byte-for-byte back at the pinned commit.
The active repository's unrelated dirty files were never edited.

## Public CLI workflow observations

The packaged stdlib-only fixture used isolated HOME/JCODE_HOME, a real loopback
MCP/OAuth server, a private temporary directory, and browser-command guards.
It inherited no API keys. It made no LLM call or deployment. All ten cases
passed; no fixture errors were recorded.

| CLI case | Observed time | HTTP requests |
|---|---:|---:|
| Offline list before login | 8.7ms | 0 |
| Unauthenticated initialize stops at 401 | 34.3ms | 1 |
| Discovery, DCR, PKCE and real callback login | 225.5ms | 6 |
| Mode-0600 token file and offline auth status | 7.0ms | 0 |
| Authenticated initialize/list/session DELETE | 75.6ms | 4 |
| Logout and offline status | 16.9ms | 0 |
| Initialize stops at 401 after logout | 27.9ms | 1 |
| Static Authorization overrides corrupt OAuth store | 78.4ms | 4 |
| Explicit legacy SSE | 34.4ms | 4 |
| HTTP405-to-legacy-SSE fallback | 47.5ms | 5 |

An additional fresh-binary public CLI check against Expo's actual HTTPS MCP
service initialized, listed **39 tools**, and disconnected cleanly in 2.7s wall
time. This was discovery only, not a deployment or a call to any listed tool.

## Execution ledger

Every Rust command below used hosted justrust. `justrust show` recorded
`where remote on hosted`, with exit zero for final-tree runs.

| Label | Command after `justrust` | Run ID | Observed result |
|---|---|---|---|
| Check | `check --remote -p jcode-base -p jcode-app-core -p jcode --lib --tests` | `20261013-080005474-857733` | Passed |
| B | `test --remote -p jcode-base --lib mcp::` | `20261013-080056187-863499` | 113 passed, 0 failed |
| M | `test --remote -p jcode-app-core --lib mcp -- --nocapture` | `20261013-080129343-866464` | 54 passed across two isolated runs, 0 failed |
| A | `test --remote -p jcode-app-core --lib agent::tests::` | `20261013-080158147-869510` | 79 passed, 0 failed |
| C | `test --remote -p jcode --lib mcp` | `20261013-080212305-871126` | 11 passed, 0 failed |
| Build | `build --remote --profile selfdev --bin jcode` | `20261013-080235863-873381` | Passed; pinned binary preserved before mutations |
| S | `test --remote -p jcode-base --test mcp_stdio_collision_integration` | `20261013-080941290-912893` | 1 integration passed, reported by parent and isolated child |
| Negative compile | Same check command as Check | `20261013-081053803-920453` | Passed before negative tests |
| Negative deadline | Base filtered deadline test | `20261013-081104975-921505` | Expected 1 assertion failure, exit 101 |
| Negative app-core | App-core `mcp_remote_cache` filter | `20261013-081116561-922377` | Expected 4 assertion failures, exit 101 |
| Negative headless | Root library `mcp_startup_tests` filter | `20261013-081133649-924110` | Expected 3 assertion failures, exit 101 |
| Restored deadline | Same base filtered deadline test | `20261013-081220153-929046` | 1 passed |
| Restored app-core | Same app-core `mcp_remote_cache` filter | `20261013-081222149-929109` | 4 passed |
| Restored headless | Same root library `mcp_startup_tests` filter | `20261013-081223827-929366` | 3 passed |

Suite counts overlap. They should not be summed into a count of distinct tests.

## Reproducing the public CLI acceptance

Requires Python 3.9+ and an explicitly selected freshly built Jcode binary.
The fixture uses only Python's standard library. No real Apple/Expo account,
credentials, browser login, paid model or deployment is needed.

```bash
justrust build --remote --profile selfdev --bin jcode
scratch="${JCODE_SCRATCH_DIR:-$HOME/.jcode/scratch}"
python3 scripts/tests/mcp_cli_acceptance.py \
  --binary target/selfdev/jcode \
  --scratch-root "$scratch" \
  --output "$scratch/mcp-acceptance-replay/results.json"
```

The report contains only request-boundary metadata and fixed assertion errors.
It does not persist token material, authorization URLs or raw CLI output.
Fixture state is temporary and removed on completion.

## Explicit limits and remaining intentional behavior

- Registry measurements do not quantify total TUI/server application startup.
- Default `jcode run` retains an intentional cold-schema wait of up to five
  seconds. `JCODE_RUN_MCP_WAIT_MS=0` disables it. Disabled servers and already
  registered exact identities no longer consume that wait.
- Cold or changed explicit-eager schemas may intentionally rebuild the tool
  snapshot and miss the prompt cache. Default auto/deferred mode keeps the eager
  prefix stable through the tested connection lifecycle.
- Prefix equality and KV telemetry tests do not prove a paid provider's actual
  cache-hit billing rate. No paid model request was made for verification.
- The CLI fixture checks a product-created mode-0600 token file inside an
  already private fixture-created directory. It does not independently prove
  permissions for every preexisting user directory.
- Public CLI legacy-SSE checks prove the protocol and clean-disconnect CLI
  result, not independent socket closure detection. Actual stream cancellation
  and closure are covered by the base real-TCP tests.
- Public CLI OAuth replay covers the successful login/logout lifecycle. Negative
  OAuth, refresh and race cases are covered by the base suite, not this fixture.
- The freshly built pinned binary was exercised directly. This report does not
  claim the shared daemon or launcher was restarted onto that binary.
