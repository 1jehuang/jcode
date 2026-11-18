# Streaming debug event queries (2026-11-18)

`scripts/probe_event_query.sh` runs two hosted release tests: uninstrumented timing and a separate counting allocator. The script extracts the production event types and formatter and freezes `age_secs` at 42 in both paths. Pretty output is compared byte for byte for reversed recent lists and ascending `id > threshold` filters, including empty lists, escaped strings and absent optional metadata. Production still samples elapsed time per event.

Thread CPU per query in two runs (old → streamed), 10 alternating batches per run:

|History / query|First run|Second run|
|---|---:|---:|
|Empty recent|28 → 19 ns|28 → 20 ns|
|One recent|679 → 667 ns|690 → 681 ns|
|50 recent|44.1 → 28.7 µs|44.1 → 29.6 µs|
|50 since (24 returned)|18.0 → 14.2 µs|18.1 → 14.6 µs|
|5,000 recent|6.77 → 2.94 ms|6.62 → 3.03 ms|
|5,000 since (2,499 returned)|3.22 → 1.46 ms|3.04 → 1.51 ms|

Uncontended read-guard query wall time followed thread CPU (5,000 recent: 6.77 → 2.94 and 6.62 → 3.03 ms). Both production paths hold the history read guard through serialization; this probe models the protected work, not Tokio writer scheduling. Requested allocation bytes (including reallocations) dropped from 11,543,066 to 11,383,066 for 5,000 recent and 5,952,602 to 5,690,586 for since. Calls dropped by one and eleven respectively. Empty output stayed at one 128-byte allocation. The output string is still buffered, but only one temporary payload tree is live at a time. These are formatter measurements, not daemon RSS or socket latency.
