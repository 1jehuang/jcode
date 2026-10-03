# Opt-in parallel native tool calls

Jcode can overlap adjacent read-only native tool calls from a single model
response. This is **disabled by default**. The initial rollout opts in only
`read` for text files, `ls`, `jcode_docs`, and `webfetch`. Image/PDF reads, all
Bash commands, `agentgrep`, `websearch`, mutations, SDK callbacks, and unknown
tools remain sequential.

## Configuration and rollback

To opt in, add this to your Jcode configuration:

```toml
[tools]
parallel = true
```

`JCODE_PARALLEL_TOOLS=1` also enables the feature. An explicit environment
setting takes precedence over configuration: `JCODE_PARALLEL_TOOLS=0` disables
it even when the file enables it. Missing configuration retains sequential
execution. Set `parallel = false` or use the environment override to roll back.
The explicit `batch` tool is independent and retains its existing behavior.

The setting also controls OpenAI Responses `parallel_tool_calls`. This grants
permission to emit multiple calls, not permission to overlap arbitrary tools.
The local scheduler checks each call even when the provider emits several.
Initial requests, WebSocket continuations, and prewarm matching carry the same
request policy. Unsupported mixed hosted/function model combinations remain
conservatively disabled.

## Executor contract

1. Ordinary local tool calls begin after the response has been collected. This
   is not speculative execution of partially streamed arguments. The separate
   provider/SDK-native streaming path is unchanged.
2. At most ten adjacent eligible calls start together. This is a fixed batch,
   not a rolling worker pool. A single eligible call follows the normal path.
3. An ineligible call is a barrier: all earlier work finishes before it begins,
   and later work waits for it. Calls outside the session tool policy are never
   executed by prefetch. Registry permission checks still apply at execution.
4. Results are consumed and stored in original call order, even if completion
   order differs. Each call's duration ends when its own work finishes, not
   when earlier results finish being consumed.
5. Prefetched work stays owned until completion or an explicit background
   handoff. Dropping the turn aborts the active prefetched task and its pending
   peers. An intentional background transfer retains ownership independently.
6. Interruption/reload drains completed successes and failures in call order.
   Unstarted or actually cancelled calls get terminal skip results, rather
   than completed outputs being replaced with synthetic skips.
7. Prefetched output is admitted serially against history-aware context usage
   and results appended since the provider snapshot. Concurrent calls cannot
   independently spend the same remaining output allowance. Normal registry,
   SDK and explicit batch callers retain their existing output guards.

## Why the first rollout is narrow

`Tool::is_concurrency_safe(input)` defaults to `false`. Opting in requires both
safe overlap and cancellation ownership. A read-oriented name is not proof:
Git can refresh its index, shell helpers can have side effects, and a linked
blocking search can outlive cancellation of the async caller. This change does
not add a shell classifier or redesign Bash/search process ownership.
Image reads can render directly to the terminal and launch a converter. PDF
reads run synchronous parsing without a cancellation boundary. Both remain
sequential, using the same case-insensitive extension checks as execution.
These paths can be reviewed independently before extending eligibility.

The predicate is a scheduling contract, not a sandbox or a replacement for
permissions. In particular, an HTTP GET may have effects at the remote server.
`webfetch` eligibility assumes the requested URLs are independent reads. Local
read-only operations do not guarantee snapshots against unrelated processes
modifying the same resources.

Configured pre-tool policy gates and input transformers disable automatic
prefetch, since their own side effects or input rewriting are not classified.
SDK custom callbacks do not opt in. Tools using background threads or child
processes must not opt in merely because dropping their outer future is cheap.

Existing non-prefetched tool cancellation and explicit background mechanisms
are not presented as newly solved by this feature. Provider-native hosted
search and tools executed inside provider streams are separate mechanisms.

## Validation

Run the configuration, provider and real Agent regressions:

```sh
cargo test --offline -p jcode-base --lib parallel -- --test-threads=1
cargo test --offline -p jcode-provider-openai-runtime --lib -- --test-threads=1
cargo test --offline -p jcode-app-core --lib parallel_tools -- --test-threads=1
cargo test --offline -p jcode-app-core --lib tool::tests -- --test-threads=1
cargo test --offline -p jcode-app-core --lib tool_streaming -- --test-threads=1
cargo test --offline -p jcode-app-core --lib turn_streaming_mpsc::tests -- --test-threads=1
```

Build and exercise the actual binary without contacting a paid provider or
attaching to the shared daemon:

```sh
scripts/dev_cargo.sh build --offline --profile selfdev -p jcode --bin jcode
python3 scripts/test_parallel_tools_cli.py --binary target/selfdev/jcode
```

The fixture uses isolated homes, fake credentials, a minimal environment and
loopback HTTP endpoints for both the scripted Responses provider and actual
`webfetch` calls. It exercises JSON/blocking and NDJSON/streaming modes. The
checks cover default-off behavior, configuration and environment precedence,
observed request overlap, call timing, ordered and exactly-once stored results,
read/write and shell barriers, and error replay. The fixture is an integration
check, not a production latency benchmark or live-provider certification.

Lifecycle and context-budget regressions drive the real Agent loops with
instrumented local tools. These cover turn cancellation, interruption/reload,
background handoff and cumulative output admission independently of the HTTP
fixture. External-provider availability is not required for these tests.
