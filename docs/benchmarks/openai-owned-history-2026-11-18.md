# OpenAI owned-history request construction

The foreground completion path already owns the `Vec<Value>` produced by
`build_responses_input`. The previous request builder serialized a borrowed
slice through `json!`, recursively copying the history. The foreground task
then retained both arrays until its WebSocket continuation attempt completed.

The builder now accepts borrowed or owned histories through `Cow<[Value]>`.
It inserts `Value::Array(input.into_owned())` directly into the request through
`get_mut`, avoiding `Value` indexing's allocation of a replacement key.
Passing a nonempty owned vector transfers its existing allocation and nested
values, while borrowed warmup and test callers still receive an independent
copy. Empty histories use their final empty-array representation immediately,
avoiding even the replacement lookup.
`complete` borrows the request's input for canonical logging and continuation.
The immutable full request remains available to every fresh-connection retry.
Canonical logging now borrows the twelve historically selected fields in sorted
order, retaining explicit nulls for absent settings instead of cloning a second
history-sized JSON object. Dedicated tests freeze that selection, exact JSON
bytes and hash, excluded transport fields, and original field pointer identity.

## Correctness checks

`openai_owned_request_tests.rs` keeps the original request builder as a frozen
oracle. Eighty combinations cover empty, tiny, large, nested, and native
reasoning/search histories, four model IDs, both authentication modes, and
configured/default request settings. The checks compare complete JSON values
and exact serialized bytes, verify that owned history storage was transferred,
and compare continuation item hashes. Existing runtime tests cover real local
HTTP/WebSocket retry, cancellation, continuation, and warmup paths.

## Measurement method

Run the ignored probe explicitly:

```sh
justrust test --release -p jcode-provider-openai-runtime --lib alternating -- --ignored --nocapture --test-threads=1
```

Both old and new paths start with an owned input vector. Input preparation is
outside each measured region. Twelve rounds alternate old/new execution order.
Allocation calls and requested bytes are counted per thread in a separate
pass from CPU timing. CPU uses Linux `CLOCK_THREAD_CPUTIME_ID`, not wall time.
Both measured paths construct and drop the complete request, and the old path
also drops its now-redundant input vector. Medians are reported per fixture.
The test rejects allocation increases and CPU increases beyond a 10% noise
allowance. The test-only allocator's disabled counter branch remains present
in CPU runs, so these are instrumented comparisons rather than end-to-end
production latency claims.

## Results, November 18, 2026 (optimized release)

Hosted Linux optimized run `20261118-045147905-2483302` passed both alternating
probes with the single `alternating` filter and `--release`. This supersedes the
earlier default opt-level-0 CPU evidence (`20261118-044634996-2443610`).
The frozen canonical selection/bytes/hash/borrowing oracle separately passed
in `20261118-045302638-2487063`. Values below are median thread CPU ratios and
measured allocation calls/requested bytes per request, excluding prepared input.

| History | CPU new/old | Old calls / bytes | New calls / bytes |
| --- | ---: | ---: | ---: |
| Empty | 1.000 | 20 / 828 | 20 / 828 |
| Tiny | 0.813 | 26 / 1,509 | 20 / 828 |
| Large | 0.272 | 1,301 / 1,224,398 | 20 / 828 |
| Nested | 0.278 | 19,029 / 3,070,706 | 20 / 828 |
| Native reasoning/search | 0.201 | 4,117 / 608,224 | 20 / 828 |

Empty-history CPU is effectively unchanged within small measurement noise.
The nonempty fixtures reduce measured CPU by 19% to 80% and remove all
history-sized allocations from request construction. This does not imply
those percentages for full provider latency or process RSS.

### Persistent replay, same optimized run

| History | Delta CPU new/old | Full replay CPU new/old | Full old calls / bytes | Full new calls / bytes |
| --- | ---: | ---: | ---: | ---: |
| Empty | 1.007 | 0.270 | 11 / 1,356 | 4 / 656 |
| Tiny | 1.014 | 0.426 | 136 / 17,223 | 63 / 9,519 |
| Malformed optional fields | 1.013 | 0.409 | 31 / 3,613 | 13 / 1,654 |
| Large | 1.010 | 0.256 | 2,676 / 4,558,047 | 698 / 1,144,725 |

Delta allocations are exactly unchanged. A second optimized run
`20261118-045334143-2489085` again passed both probes: owned-history CPU ratios
were 1.019, 0.826, 0.270, 0.277, and 0.200 in table order. Delta ratios were
0.971, 1.011, 1.008, and 1.006; full replay ratios were 0.261, 0.425, 0.396,
and 0.261. There was no allocation regression, and normal/tiny delta differences
remain around 1%, within the stated noise allowance, rather than a material
CPU regression. These probes isolate request construction/replay, not canonical
logging or end-to-end provider latency.

The serial full runtime suite passed 244 tests with 5 ignored in hosted run
`20261118-044332141-2434239`. An earlier parallel suite run exposed an existing
unlocked API-base environment test race, so the full suite was rerun with
`--test-threads=1`. Production hosted check and clippy also passed.
