# Durable history response statistics

Harness API minor version 3 adds optional `HistoryMessage.response_stats`, also
exported as `ResponseStats` by the Rust and TypeScript SDKs. Old history messages
still deserialize, and absent metrics are omitted from JSON.

Fields:

- `duration_secs: Option<f64>`: whole-turn wall-clock seconds, including tools.
  **Currently absent in restored history.** Stored assistant messages contain no
  response duration. `tool_duration_ms` belongs to tool results and is not a
  substitute. Timestamps are not used to invent elapsed time.
- `input_tokens`, `output_tokens`: optional `u64` sums of persisted assistant
  provider-call usage. Inputs are raw provider-reported counts, not normalized:
  OpenAI includes cache reads in input while Anthropic reports them separately.
- `cache_read_tokens`, `cache_creation_tokens`: optional `u64` sums of the
  respective persisted cache-input counters. Do not blindly add them to input.

Aggregation runs over the complete stored transcript, including tool-only
assistant rounds that have no visible text and history hidden by compaction.
Real user prompts delimit turns. Tool results, internal reminders, scheduled
instructions, and automatic continuation prompts do not create new boundaries.
Totals appear once, on the last assistant message of the turn, and only if that
message has a rendered row and no pending tool calls. This terminal-message rule
is an inference from the transcript, not a persisted `done` event. The API bridge
also suppresses the current turn's footer when the history activity snapshot says
it is still running. Intermediate assistant rows never receive partial totals.

Each token field is independently unknown if any contributing assistant round
lacks that field, or its sum overflows `u64`. Unknown is not zero. Entirely unknown
statistics are omitted. Older sessions and text-only `peek_session` previews can
therefore have no statistics. Existing storage sometimes records zero usage when
a provider supplies no telemetry; the history layer preserves those stored zeros
and cannot retrospectively distinguish them from reported zero usage.

These are per-user-turn provider-call totals, not unique context size, session
usage, billing estimates, or tokens per second. The same prompt/context can be
counted again in each tool round. Per-call provider identity is not stored, so a
turn spanning providers cannot be retrospectively normalized reliably.
