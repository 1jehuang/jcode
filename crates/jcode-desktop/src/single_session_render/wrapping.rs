//! Body line wrapping algorithms for single_session_render.
//! Operates on SingleSessionStyledLine to produce explicit visual rows so
//! scroll metrics, selection hit-testing, and the rendered viewport agree.

use super::*;

pub(super) fn single_session_wrapped_body_lines(
    lines: Vec<SingleSessionStyledLine>,
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> Vec<SingleSessionStyledLine> {
    // Glyphon also wraps, but explicit visual rows keep scroll metrics,
    // selection hit-testing, and the rendered text viewport in agreement.
    let max_columns = single_session_body_max_columns(size, text_scale);
    if should_parallel_wrap_body_lines(lines.len()) {
        return parallel_wrap_body_lines(&lines, max_columns);
    }

    let mut wrapped = Vec::with_capacity(lines.len());

    for line in lines {
        push_wrapped_body_line_owned(&mut wrapped, line, max_columns);
    }

    wrapped
}

pub(super) fn single_session_wrapped_body_line_count(
    lines: Vec<SingleSessionStyledLine>,
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> usize {
    let max_columns = single_session_body_max_columns(size, text_scale);
    lines
        .iter()
        .map(|line| wrapped_body_line_count(line, max_columns))
        .sum()
}

pub(super) fn single_session_wrapped_body_lines_ref(
    lines: &[SingleSessionStyledLine],
    size: PhysicalSize<u32>,
    text_scale: f32,
) -> Vec<SingleSessionStyledLine> {
    // Glyphon also wraps, but explicit visual rows keep scroll metrics,
    // selection hit-testing, and the rendered text viewport in agreement.
    let max_columns = single_session_body_max_columns(size, text_scale);
    if should_parallel_wrap_body_lines(lines.len()) {
        return parallel_wrap_body_lines(lines, max_columns);
    }

    wrap_body_lines_slice(lines, max_columns)
}

pub(super) fn should_parallel_wrap_body_lines(line_count: usize) -> bool {
    line_count >= 512
        && std::thread::available_parallelism()
            .map(|parallelism| parallelism.get() > 1)
            .unwrap_or(false)
}

pub(super) fn parallel_wrap_body_lines(
    lines: &[SingleSessionStyledLine],
    max_columns: usize,
) -> Vec<SingleSessionStyledLine> {
    let available_parallelism = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    let worker_count = available_parallelism
        .min(lines.len().div_ceil(256).max(1))
        .max(1);
    if worker_count <= 1 {
        return wrap_body_lines_slice(lines, max_columns);
    }

    let chunk_size = lines.len().div_ceil(worker_count).max(1);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for chunk in lines.chunks(chunk_size) {
            handles.push(scope.spawn(move || wrap_body_lines_slice(chunk, max_columns)));
        }
        let mut wrapped = Vec::with_capacity(lines.len());
        for handle in handles {
            wrapped.extend(
                handle
                    .join()
                    .expect("desktop body wrap worker panicked unexpectedly"),
            );
        }
        wrapped
    })
}

pub(super) fn wrap_body_lines_slice(
    lines: &[SingleSessionStyledLine],
    max_columns: usize,
) -> Vec<SingleSessionStyledLine> {
    let mut wrapped = Vec::with_capacity(lines.len());
    for line in lines {
        push_wrapped_body_line_ref(&mut wrapped, line, max_columns);
    }
    wrapped
}

pub(super) fn push_wrapped_body_line_owned(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    line: SingleSessionStyledLine,
    max_columns: usize,
) {
    if line.text.is_empty() {
        wrapped.push(line);
        return;
    }
    if line.inline_spans.is_empty() && line.text.is_ascii() {
        if line.text.len() <= max_columns.max(1) {
            wrapped.push(line);
        } else {
            push_wrapped_ascii_body_line_parts(
                wrapped,
                &line.text,
                line.style,
                line.tool.as_ref(),
                max_columns,
            );
        }
        return;
    }
    if !text_exceeds_columns(&line.text, max_columns) {
        wrapped.push(line);
        return;
    }
    push_wrapped_body_line_parts(
        wrapped,
        &line.text,
        &line.inline_spans,
        line.style,
        line.tool.as_ref(),
        max_columns,
    );
}

pub(super) fn push_wrapped_body_line_ref(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    line: &SingleSessionStyledLine,
    max_columns: usize,
) {
    if line.text.is_empty() {
        wrapped.push(line.clone());
        return;
    }
    if line.inline_spans.is_empty() && line.text.is_ascii() {
        if line.text.len() <= max_columns.max(1) {
            wrapped.push(line.clone());
        } else {
            push_wrapped_ascii_body_line_parts(
                wrapped,
                &line.text,
                line.style,
                line.tool.as_ref(),
                max_columns,
            );
        }
        return;
    }
    if !text_exceeds_columns(&line.text, max_columns) {
        wrapped.push(line.clone());
        return;
    }
    push_wrapped_body_line_parts(
        wrapped,
        &line.text,
        &line.inline_spans,
        line.style,
        line.tool.as_ref(),
        max_columns,
    );
}

pub(super) fn wrapped_body_line_count(line: &SingleSessionStyledLine, max_columns: usize) -> usize {
    if line.text.is_empty() {
        return 1;
    }
    if line.inline_spans.is_empty() && line.text.is_ascii() {
        return wrapped_ascii_body_line_count(&line.text, max_columns);
    }
    if !text_exceeds_columns(&line.text, max_columns) {
        return 1;
    }
    wrapped_body_line_text_count(&line.text, &line.inline_spans, max_columns)
}

pub(super) fn wrapped_ascii_body_line_count(text: &str, max_columns: usize) -> usize {
    let max_columns = max_columns.max(1);
    let trimmed_end = text.trim_end().len();
    let mut remaining = &text[..trimmed_end];
    let mut count = 1usize;

    while remaining.len() > max_columns {
        let split = ascii_word_wrap_split_index(remaining, max_columns);
        remaining = remaining[split..].trim_start();
        count += 1;
    }

    count
}

pub(super) fn push_wrapped_body_line_parts(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    style: SingleSessionLineStyle,
    tool: Option<&SingleSessionToolLineMetadata>,
    max_columns: usize,
) {
    for (text, inline_spans) in wrap_body_line_text_with_spans(text, inline_spans, max_columns) {
        let mut line = SingleSessionStyledLine::with_inline_spans(text, style, inline_spans);
        line.tool = tool.cloned();
        wrapped.push(line);
    }
}

pub(super) fn push_wrapped_ascii_body_line_parts(
    wrapped: &mut Vec<SingleSessionStyledLine>,
    text: &str,
    style: SingleSessionLineStyle,
    tool: Option<&SingleSessionToolLineMetadata>,
    max_columns: usize,
) {
    let max_columns = max_columns.max(1);
    let trimmed_end = text.trim_end().len();
    let mut remaining = &text[..trimmed_end];

    while remaining.len() > max_columns {
        let split = ascii_word_wrap_split_index(remaining, max_columns);
        let line = remaining[..split].trim_end();
        let mut wrapped_line = SingleSessionStyledLine::new(line.to_string(), style);
        wrapped_line.tool = tool.cloned();
        wrapped.push(wrapped_line);

        remaining = remaining[split..].trim_start();
    }

    let mut wrapped_line = SingleSessionStyledLine::new(remaining.to_string(), style);
    wrapped_line.tool = tool.cloned();
    wrapped.push(wrapped_line);
}

pub(super) fn single_session_body_max_columns(size: PhysicalSize<u32>, text_scale: f32) -> usize {
    let content_width = single_session_content_width(size);
    (content_width / single_session_body_char_width_for_scale(text_scale))
        .floor()
        .max(20.0) as usize
}

pub(super) fn wrap_body_line_text_with_spans(
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    max_columns: usize,
) -> Vec<(String, Vec<SingleSessionInlineSpan>)> {
    let max_columns = max_columns.max(1);
    let trimmed_end =
        single_session_trimmed_line_end_preserving_inline_code_whitespace(text, inline_spans);
    let mut remaining = &text[..trimmed_end];
    let mut lines = Vec::new();
    let mut base_byte = 0usize;

    while text_exceeds_columns(remaining, max_columns) {
        let split = word_wrap_split_index(remaining, max_columns);
        let (line, rest) = remaining.split_at(split);
        let line = line.trim_end();
        let start = base_byte;
        let end = start + line.len();
        lines.push((
            line.to_string(),
            inline_spans_for_wrapped_range(inline_spans, start, end),
        ));

        let trimmed_rest = rest.trim_start();
        base_byte += split + rest.len().saturating_sub(trimmed_rest.len());
        remaining = trimmed_rest;
    }

    let start = base_byte;
    let end = start + remaining.len();
    lines.push((
        remaining.to_string(),
        inline_spans_for_wrapped_range(inline_spans, start, end),
    ));
    lines
}

pub(super) fn wrapped_body_line_text_count(
    text: &str,
    inline_spans: &[SingleSessionInlineSpan],
    max_columns: usize,
) -> usize {
    let max_columns = max_columns.max(1);
    let trimmed_end =
        single_session_trimmed_line_end_preserving_inline_code_whitespace(text, inline_spans);
    let mut remaining = &text[..trimmed_end];
    let mut count = 1usize;

    while text_exceeds_columns(remaining, max_columns) {
        let split = word_wrap_split_index(remaining, max_columns);
        let (_, rest) = remaining.split_at(split);
        remaining = rest.trim_start();
        count += 1;
    }

    count
}

pub(super) fn inline_spans_for_wrapped_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpan> {
    if inline_spans.is_empty() {
        return Vec::new();
    }

    inline_spans
        .iter()
        .filter_map(|span| {
            let span_start = span.start.max(start);
            let span_end = span.end.min(end);
            (span_start < span_end).then(|| SingleSessionInlineSpan {
                start: span_start - start,
                end: span_end - start,
                kind: span.kind,
            })
        })
        .collect()
}

pub(super) fn text_exceeds_columns(text: &str, max_columns: usize) -> bool {
    if text.is_ascii() {
        return text.len() > max_columns.max(1);
    }

    text.chars().nth(max_columns.max(1)).is_some()
}

pub(super) fn word_wrap_split_index(text: &str, max_columns: usize) -> usize {
    let max_columns = max_columns.max(1);
    if text.is_ascii() {
        return ascii_word_wrap_split_index(text, max_columns);
    }

    let hard_split = byte_index_at_char_limit(text, max_columns);
    text[..hard_split]
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .filter(|index| *index > 0)
        .unwrap_or(hard_split)
}

pub(super) fn ascii_word_wrap_split_index(text: &str, max_columns: usize) -> usize {
    let hard_split = text.len().min(max_columns.max(1));
    text.as_bytes()[..hard_split]
        .iter()
        .rposition(u8::is_ascii_whitespace)
        .filter(|index| *index > 0)
        .unwrap_or(hard_split)
}

pub(super) fn byte_index_at_char_limit(text: &str, max_columns: usize) -> usize {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .nth(max_columns)
        .unwrap_or(text.len())
}
