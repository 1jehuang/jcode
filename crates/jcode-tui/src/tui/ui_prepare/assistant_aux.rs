use super::*;

/// Compute (and cache) the auxiliary raw-line-map data for an assistant message.
/// `cached` is the already-rendered (and possibly cached) display lines for the
/// message; `align` and `centered` describe the current display mode.
pub(super) fn assistant_aux_data(
    msg: &DisplayMessage,
    cached: &[Line<'static>],
    content_width: u16,
    centered: bool,
    diff_mode: crate::config::DiffDisplayMode,
    align: ratatui::layout::Alignment,
) -> std::sync::Arc<AssistantAuxData> {
    let build = || {
        // Ordinary assistant rendering returns only markdown lines, with no
        // appended tool footer. Reuse that authoritative count instead of
        // parsing the same markdown again just to discard its rendered lines.
        // Plan cards rewrite/box their markdown, so retain the legacy count for
        // their selection mapping. Match the renderer's conservative plan gate.
        let content_line_count = if msg.content.contains("```plan") {
            markdown::render_markdown_with_width(&msg.content, Some(content_width as usize))
                .len()
                .min(cached.len())
        } else {
            cached.len()
        };
        let logical_plain_lines: Vec<String> =
            if content_prefers_display_as_logical_lines(&msg.content) {
                cached
                    .iter()
                    .take(content_line_count)
                    .map(ui::line_plain_text)
                    .collect()
            } else {
                markdown::render_markdown(&msg.content)
                    .into_iter()
                    .map(|line| ui::line_plain_text(&align_if_unset(line, align)))
                    .collect()
            };
        AssistantAuxData {
            content_line_count,
            logical_plain_lines,
        }
    };

    if cfg!(test) {
        return std::sync::Arc::new(build());
    }

    let key = AssistantAuxKey {
        message_hash: msg.stable_cache_hash(),
        content_len: msg.content.len(),
        content_width,
        centered,
        diff_mode,
        cached_len: cached.len(),
    };

    {
        let cache = match assistant_aux_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(data) = cache.entries.get(&key) {
            return data.clone();
        }
    }

    let data = std::sync::Arc::new(build());
    let mut cache = match assistant_aux_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    if cache.entries.insert(key.clone(), data.clone()).is_none() {
        cache.order.push_back(key);
        while cache.order.len() > ASSISTANT_AUX_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The previous cold auxiliary builder, retained only as a parity oracle
    /// and benchmark baseline. It deliberately performs both markdown passes.
    fn legacy_aux(
        msg: &DisplayMessage,
        cached: &[Line<'static>],
        width: u16,
        align: ratatui::layout::Alignment,
    ) -> AssistantAuxData {
        let count = markdown::render_markdown_with_width(&msg.content, Some(width as usize))
            .len()
            .min(cached.len());
        let logical_plain_lines = if content_prefers_display_as_logical_lines(&msg.content) {
            cached.iter().take(count).map(ui::line_plain_text).collect()
        } else {
            markdown::render_markdown(&msg.content)
                .into_iter()
                .map(|line| ui::line_plain_text(&align_if_unset(line, align)))
                .collect()
        };
        AssistantAuxData {
            content_line_count: count,
            logical_plain_lines,
        }
    }

    #[test]
    fn assistant_aux_ordinary_skips_count_render_and_preserves_logical_lines() {
        let cases = [
            "",
            "# Heading\n\nA **styled** paragraph with café and 日本語.\n\n- first\n- second",
            "> A quote\n\n```rust\nfn main() { println!(\"hello\"); }\n```",
            "A paragraph\n\n---\n\nAnother paragraph",
        ];
        for centered in [false, true] {
            markdown::with_center_code_blocks(centered, || {
                for width in [0, 1, 40, 96, 120] {
                    for text in cases {
                        let msg = DisplayMessage::assistant(text);
                        let mode = crate::config::DiffDisplayMode::default();
                        let align = default_message_alignment("assistant", centered);
                        let cached = render_assistant_message(&msg, width, mode);
                        let before = markdown::thread_render_count();
                        let aux = assistant_aux_data(&msg, &cached, width, centered, mode, align);
                        assert_eq!(markdown::thread_render_count() - before, 1);
                        assert_eq!(aux.content_line_count, cached.len());
                        let legacy = legacy_aux(&msg, &cached, width, align);
                        // Width zero is normalized to one by the actual
                        // renderer, but not by the legacy count-only render.
                        // A horizontal rule then has 5 actual lines vs 3 in
                        // the old auxiliary count. Trust the actual output.
                        if width > 0 {
                            assert_eq!(
                                aux.content_line_count, legacy.content_line_count,
                                "centered={centered}, width={width}, text={text:?}"
                            );
                            let maps = |data: &AssistantAuxData| {
                                map_display_lines_to_logical_lines(
                                    &cached[..data.content_line_count],
                                    &data.logical_plain_lines,
                                    0,
                                )
                                .map(|maps| {
                                    maps.into_iter()
                                        .map(|map| (map.raw_line, map.start_col, map.end_col))
                                        .collect::<Vec<_>>()
                                })
                            };
                            assert_eq!(maps(&aux), maps(&legacy));
                        }
                        assert_eq!(aux.logical_plain_lines, legacy.logical_plain_lines);
                    }
                }
            });
        }
    }

    #[test]
    fn assistant_aux_tables_reuse_display_without_rendering() {
        let msg = DisplayMessage::assistant(
            "| Name | Details |\n| --- | --- |\n| alpha | A long table cell with many words to fit |",
        );
        for centered in [false, true] {
            markdown::with_center_code_blocks(centered, || {
                for width in [1, 40, 120] {
                    let mode = crate::config::DiffDisplayMode::default();
                    let align = default_message_alignment("assistant", centered);
                    let cached = render_assistant_message(&msg, width, mode);
                    let before = markdown::thread_render_count();
                    let aux = assistant_aux_data(&msg, &cached, width, centered, mode, align);
                    assert_eq!(markdown::thread_render_count(), before);
                    assert_eq!(aux.content_line_count, cached.len());
                    assert_eq!(
                        aux.logical_plain_lines,
                        cached.iter().map(ui::line_plain_text).collect::<Vec<_>>()
                    );
                }
            });
        }
    }

    #[test]
    fn assistant_aux_plan_cards_keep_legacy_mapping() {
        for text in [
            "Before\n\n```plan\n# Plan\n\n- first step\n- second step\n```\n\nAfter",
            "```plan\n# Incomplete plan\n\nA paragraph that wraps within the card",
            "```text\nA literal ```plan marker inside another fence\n```",
        ] {
            let msg = DisplayMessage::assistant(text);
            for centered in [false, true] {
                markdown::with_center_code_blocks(centered, || {
                    for width in [40, 120] {
                        let mode = crate::config::DiffDisplayMode::default();
                        let align = default_message_alignment("assistant", centered);
                        let cached = render_assistant_message(&msg, width, mode);
                        let before = markdown::thread_render_count();
                        let aux = assistant_aux_data(&msg, &cached, width, centered, mode, align);
                        assert_eq!(markdown::thread_render_count() - before, 2);
                        let legacy = legacy_aux(&msg, &cached, width, align);
                        assert_eq!(aux.content_line_count, legacy.content_line_count);
                        assert_eq!(aux.logical_plain_lines, legacy.logical_plain_lines);
                    }
                });
            }
        }
    }

    /// Compares uncached render + auxiliary work at the two scrollbar widths.
    /// This intentionally bypasses the message cache on both sides and uses
    /// cfg(test)'s auxiliary-cache bypass. It is not an end-to-end reload timer.
    #[test]
    #[ignore = "manual cold assistant preparation comparison benchmark"]
    fn assistant_aux_cold_comparison_benchmark() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        let messages: Vec<_> = (0..400)
            .map(|i| {
                DisplayMessage::assistant(format!(
                    "## Message {i}\n\n{}\n\n```rust\nfn example() {{ println!(\"hello\"); }}\n```\n\n- Check the result\n- Keep the mapping\n",
                    "A **styled** paragraph with enough words to wrap in the transcript. ".repeat(12)
                ))
            })
            .collect();
        let run = |legacy: bool| {
            let mode = crate::config::DiffDisplayMode::default();
            let before = markdown::thread_render_count();
            let start = Instant::now();
            for width in [116, 115] {
                for msg in &messages {
                    let cached = render_assistant_message(msg, width, mode);
                    if legacy {
                        black_box(legacy_aux(
                            msg,
                            &cached,
                            width,
                            ratatui::layout::Alignment::Left,
                        ));
                    } else {
                        black_box(assistant_aux_data(
                            msg,
                            &cached,
                            width,
                            false,
                            mode,
                            ratatui::layout::Alignment::Left,
                        ));
                    }
                    black_box(cached);
                }
            }
            (start.elapsed(), markdown::thread_render_count() - before)
        };
        markdown::with_center_code_blocks(false, || {
            // Exclude one-time syntax resources and highlight cache population
            // from both sides. Every timed iteration still parses all messages.
            black_box(run(true));
            black_box(run(false));
            let mut old_times: Vec<Duration> = Vec::new();
            let mut new_times: Vec<Duration> = Vec::new();
            for iteration in 0..7 {
                let order = if iteration % 2 == 0 {
                    [true, false]
                } else {
                    [false, true]
                };
                for legacy in order {
                    let (elapsed, renders) = run(legacy);
                    assert_eq!(renders, messages.len() as u64 * if legacy { 6 } else { 4 });
                    if legacy {
                        old_times.push(elapsed);
                    } else {
                        new_times.push(elapsed);
                    }
                }
            }
            old_times.sort();
            new_times.sort();
            let old = old_times[old_times.len() / 2].as_secs_f64() * 1000.0;
            let new = new_times[new_times.len() / 2].as_secs_f64() * 1000.0;
            eprintln!(
                "assistant_aux cold comparison: 400 messages x 2 widths, 7 samples; legacy median {old:.2}ms / 2400 renders, optimized median {new:.2}ms / 1600 renders, {:.1}% less time (highlight resources warm, auxiliary/message caches bypassed)",
                (1.0 - new / old) * 100.0
            );
        });
    }
}
