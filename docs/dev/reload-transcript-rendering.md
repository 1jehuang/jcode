# Cold transcript rendering after reload

## Changes

- Before the first layout, count only rows the transcript renderer must emit (user, meta, error, explicit spacer rows, and applicable inter-message separators). When this lower bound exceeds the available viewport height, prepare the scrollbar-reserved width directly rather than building and discarding the full-width transcript first. Hidden tool results are not counted. The normal overflow check still validates the resulting layout.
- Ordinary assistant messages already contain only rendered Markdown lines, so reuse their length for selection metadata. Previously the auxiliary builder rendered the same Markdown again just to count lines. Preserve the existing special handling for plan cards.

## Evidence

Observed local reload logs had first-frame preparation times of approximately 0.6–1.2 seconds for some longer histories. These frames performed two complete preparations, one per scrollbar width. This is diagnostic evidence, not a controlled before/after timing comparison.

Controlled assistant preparation comparison on justrust hosted, selfdev profile:

| Workload | Before | After |
| --- | ---: | ---: |
| 400 Markdown messages at two widths, median of 7 alternating samples | 215.29 ms | 144.61 ms |
| Markdown render calls for the same workload | 2,400 | 1,600 |

This is 32.8% less time for the measured Markdown-plus-selection-metadata work. The benchmark bypasses message/auxiliary caches and warms syntax definitions/highlighting equally on both sides. It does not measure process startup, history transport, the full frame, or the separate scrollbar optimization.

Reproduce:

```sh
justrust test -p jcode-tui --lib --profile selfdev assistant_aux_cold_comparison_benchmark -- --ignored --nocapture
# justrust retains passing-test output in its run log:
justrust log --grep 'assistant_aux cold'
```

`reload_long_transcript_prepares_once_and_preserves_frame` checks an 80-message cold transcript uses exactly one preparation and matches its subsequent frame. Additional regressions cover conservative bounds, empty/hidden messages, multiple widths, Unicode, tables, plan cards, and text-selection metadata.

The broader UI group passes with explicit truecolor support, which its pre-existing shimmer-color test requires on hosted runners:

```sh
justrust test -p jcode-tui --lib --profile selfdev \
  --config 'env.COLORTERM.value="truecolor"' \
  --config 'env.COLORTERM.force=true' tui::ui::tests -- --test-threads=1
```

A rebuilt native TUI tester rendered a 437-line Markdown transcript with 60 Rust code blocks without visual anomalies. Its first captured preparation took 30.45 ms. This is a runtime smoke check, not a comparison with the earlier reload logs. The compiled scrolling smoke test passed all 14 normal/paused viewport checks after deferred diagrams were ready.
