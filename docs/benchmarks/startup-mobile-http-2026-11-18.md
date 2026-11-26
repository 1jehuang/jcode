# Startup mobile HTTP client sharing, 2026-11-18

## Change and scope

The daemon's `mobile_sync::background_sync` and `mobile_runner::local::spawn_local_runner` previously each constructed an unmodified `reqwest::Client::new()`. They now each obtain an owned clone of one process-local `OnceLock<reqwest::Client>` in `mobile_sync::daemon_http_client`.

Both loops still initialize their client at their original eager startup location, before checking whether mobile sync is enabled. This does not defer TLS setup or change readiness. The cloud runner, interactive discovery client, provider clients, request code, polling cadence, notification handling and module wiring are unchanged.

The shared initializer is precisely `reqwest::Client::new`, not the provider shared client. Default proxy discovery, TLS roots and verification, redirect policy and client defaults are preserved. Existing per-request 30-second timeouts, bearer credentials, JSON bodies and headers remain at their original call sites. Neither loop installs default authorization headers or a cookie store. Pooling is now shared, but bearer credentials remain request-local. The static keeps one owner for the daemon's lifetime.

## Optimized measurements

Built with hosted justrust, then ran the downloaded native Linux executable on the local XPS 14 / Intel Core Ultra X9 388H. Used the repository's optimized `release` profile (`opt-level = 1`), without the unrelated optional AWS Bedrock SDK. This does not remove or change the mobile reqwest dependency settings. Pinned each measurement process and its test thread to CPU 0.

For each of two separate passes, ran 21 fresh-process pairs, alternating old/new ordering. The CPU pass left allocation counters disabled. The allocation pass enabled an instrumented `System` allocator only around the measured region. CPU is `CLOCK_THREAD_CPUTIME_ID`, not wall time, an allocation estimate or a debug-build measurement. Tokio runtime creation is outside the measurement.

The standalone probe faithfully compares two default constructors against one default constructor plus two cheap clones, retaining one handle to model the static owner. The baseline also retains one extra clone for symmetric ownership. It measures the construction primitive, not whole-daemon startup or the `OnceLock` synchronization instruction itself. It performs no network calls and does not touch user session storage.

Medians:

| Metric | Two default clients | One default client plus clones | Difference |
| --- | ---: | ---: | ---: |
| Startup thread CPU, counter disabled | 6.247 ms | 3.286 ms | -47.4% |
| Startup wall time, counter disabled | 6.259 ms | 3.295 ms | -47.4% |
| Startup cumulative allocated bytes, separate counted pass | 9,723,030 | 4,861,515 | -50.0%, 4.64 MiB avoided |
| Startup allocation/reallocation calls | 7,042 | 3,521 | -50.0% |
| Warm request-build thread CPU, counter disabled | 14.105 ms | 13.928 ms | -1.3%, treated as noise |
| Warm request-build cumulative allocated bytes | 28,750,000 | 28,750,000 | unchanged |
| Warm request-build allocation/reallocation calls | 280,000 | 280,000 | unchanged |

The warm region builds 10,000 authenticated GET/JSON POST request pairs on the already-retained clients, preserving their per-request timeout. No warm regression was observed. These are cumulative allocations, not live retained bytes, RSS or PSS. The earlier daemon sample's roughly 4.5 MiB second default-client initialization is consistent with the independently counted 4,861,515-byte saving here, but this report does not claim a measured whole-daemon PSS reduction.

Build record: `20261118-053849065-2644362`, hosted, successful, selected executable `target/release/deps/mobile_http_startup_probe-7f1ca1fcc1d725fd`. Raw local measurements: `$JCODE_SCRATCH_DIR/mobile-http/results.json`, 84 fresh processes total.

## Behavior coverage

`mobile_http_tests` compares the exact local HTTP requests emitted by the existing cloud-session listing and local-runner claim functions, first with two independent default clients and then with shared clones:

- Identical method, path/query, headers and JSON bytes, excluding only the fixture's varying ephemeral Host port.
- Different sync and runner bearer credentials never cross requests.
- The fixture waits for both concurrent requests before returning either response, proving neither client serializes the other across an await.
- Repeated warmed rounds still produce identical requests and do not echo server `Set-Cookie` values.
- Built plain requests contain neither default authorization nor cookies, have no per-request timeout unless explicitly set, and retain an explicit 30-second override.
- A second fixture accepts exactly one keep-alive TCP connection and serves successive requests from both clones, followed by an unauthenticated request. This checks actual pool sharing, no credential inheritance, and no response-cookie persistence on the same connection and origin. Returning independently built clients cannot pass this fixture.

## Reproduction and validation

```bash
justrust check -p jcode-base --tests
justrust test -p jcode-base --lib mobile_http_tests -- --nocapture
justrust test -p jcode-base --lib mobile_sync::http_tests -- --nocapture
justrust test -p jcode-base --lib mobile_runner::local::tests -- --nocapture
justrust clippy -p jcode-base --lib --test mobile_http_startup_probe -- -D warnings
justrust test -p jcode-base --release --no-default-features \
  --test mobile_http_startup_probe --no-run
# Run the executable printed by justrust, using a fresh process per sample:
MOBILE_HTTP_MODE=two taskset -c 0 "$PROBE" --ignored --exact mobile_http_startup_probe --nocapture
MOBILE_HTTP_MODE=shared taskset -c 0 "$PROBE" --ignored --exact mobile_http_startup_probe --nocapture
# Separate allocation pass:
MOBILE_HTTP_COUNT=1 MOBILE_HTTP_MODE=two taskset -c 0 "$PROBE" --ignored --exact mobile_http_startup_probe --nocapture
MOBILE_HTTP_COUNT=1 MOBILE_HTTP_MODE=shared taskset -c 0 "$PROBE" --ignored --exact mobile_http_startup_probe --nocapture
```

Hosted compile check passed (`20261118-053705052-2633341`). Strengthened wire and shared-pool regressions passed, 2 tests (`20261118-054240502-2656432`). Existing upload/body/keep-alive regressions passed, 6 tests with 1 benchmark ignored (`20261118-054400647-2660398`), and existing local-runner regressions passed, 10 tests (`20261118-054405703-2660766`). Strict production-library and dedicated-probe lint passed (`20261118-054149212-2654122`). Broader strict test lint was also attempted, but is blocked by unrelated existing test diagnostics, including `runtime_activity`'s `async_yields_async`, existing test field reassignments, inconsistent digit grouping and constant assertions. No mobile-sharing diagnostic was emitted. Unrelated files were not changed to silence them.
