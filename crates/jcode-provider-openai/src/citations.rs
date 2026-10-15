//! Render Responses URL annotations without guessing a URL from a search ID.
//!
//! The completed output item is authoritative: annotation events may arrive
//! after the text deltas. Indices are character offsets, not UTF-8 byte offsets.

use serde_json::Value;

pub(super) fn render_annotated_text(text: &str, annotations: Option<&Value>) -> String {
    let Some(annotations) = annotations.and_then(Value::as_array) else {
        return text.to_string();
    };
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut citations = Vec::new();
    for annotation in annotations {
        if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
            continue;
        }
        let Some((start, end, link, destination)) = (|| {
            let start = annotation.get("start_index")?.as_u64()?;
            let end = annotation.get("end_index")?.as_u64()?;
            // Validate against the actual character table before narrowing offsets.
            if start >= boundaries.len() as u64 || end >= boundaries.len() as u64 {
                return None;
            }
            let start = boundaries[start as usize];
            let end = boundaries[end as usize];
            if start > end {
                return None;
            }
            let url = annotation.get("url")?.as_str()?;
            let parsed = match reqwest::Url::parse(url) {
                Ok(parsed) => parsed,
                Err(_) => {
                    eprintln!("Ignoring citation annotation with an invalid URL");
                    return None;
                }
            };
            // Never make file:, javascript:, or terminal-control payloads clickable.
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || url.chars().any(char::is_control)
            {
                return None;
            }
            let title = annotation
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or(url);
            let mut label = String::new();
            for ch in title.chars() {
                if ch.is_control() {
                    label.push(' ');
                } else {
                    if ch.is_ascii_punctuation() {
                        label.push('\\');
                    }
                    label.push(ch);
                }
            }
            // Angle-bracket destinations avoid unbalanced URL parentheses. Only
            // Markdown delimiters are encoded, the URL still comes from OpenAI.
            let destination = url
                .replace(' ', "%20")
                .replace('<', "%3C")
                .replace('>', "%3E")
                .replace('\\', "%5C");
            Some((
                start,
                end,
                format!("[{label}](<{destination}>)"),
                destination,
            ))
        })() else {
            continue;
        };
        // The OAuth backend can already return a Markdown citation covering
        // the annotation's URL. Keep that source rather than appending a
        // second link to the same destination (observed in a live smoke test).
        let cited = &text[start..end];
        if cited.contains(&format!("]({destination})"))
            || cited.contains(&format!("](<{destination}>)"))
        {
            continue;
        }
        citations.push((start, end, link));
    }
    // Stable ordering preserves multiple sources sharing the same citation span.
    citations.sort_by_key(|(start, end, _)| (*start, *end));
    let mut rendered = String::new();
    let mut cursor = 0;
    let mut index = 0;
    while index < citations.len() {
        let (start, end, _) = &citations[index];
        let (start, end) = (*start, *end);
        let mut links = Vec::new();
        while index < citations.len() && citations[index].0 == start && citations[index].1 == end {
            let link = &citations[index].2;
            if !links.contains(&link) {
                links.push(link);
            }
            index += 1;
        }
        if start < cursor {
            // Ambiguous overlapping spans must not eat already-rendered prose.
            continue;
        }
        rendered.push_str(&text[cursor..start]);
        let cited = &text[start..end];
        let trimmed = cited.trim();
        let is_marker = trimmed.starts_with("\u{e200}cite\u{e202}")
            && trimmed.ends_with('\u{e201}')
            && !trimmed[..trimmed.len() - '\u{e201}'.len_utf8()].contains('\u{e201}');
        if is_marker {
            // Preserve whitespace surrounding a marker covered by the annotation.
            rendered.push_str(&cited[..cited.len() - cited.trim_start().len()]);
        } else {
            // Some APIs annotate ordinary prose rather than an opaque marker.
            // Keep that prose and attach its source instead of replacing a claim.
            rendered.push_str(cited);
            if !rendered.ends_with(char::is_whitespace) {
                rendered.push(' ');
            }
        }
        for (index, link) in links.iter().enumerate() {
            if index > 0 {
                rendered.push(' ');
            }
            rendered.push_str(link);
        }
        if is_marker {
            rendered.push_str(&cited[cited.trim_end().len()..]);
        }
        cursor = end;
    }
    rendered.push_str(&text[cursor..]);
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn citation(text: &str, marker: &str, url: &str, title: &str) -> Value {
        let start = text[..text.find(marker).unwrap()].chars().count();
        json!({"type":"url_citation", "start_index":start,
            "end_index":start + marker.chars().count(), "url":url, "title":title})
    }

    #[test]
    fn native_marker_uses_real_annotation_even_after_multibyte_prose() {
        let marker = "\u{e200}cite\u{e202}turn0search12\u{e201}";
        let text = format!("许可 🦀. {marker}\n\n");
        let annotations = json!([citation(
            &text,
            marker,
            "https://www.apple.com/legal/sla/docs/macOSTahoe.pdf",
            "macOS license"
        )]);
        assert_eq!(
            render_annotated_text(&text, Some(&annotations)),
            "许可 🦀. [macOS license](<https://www.apple.com/legal/sla/docs/macOSTahoe.pdf>)\n\n"
        );
    }

    #[test]
    fn shared_span_preserves_multiple_sources_and_deduplicates() {
        let marker = "\u{e200}cite\u{e202}turn0search1\u{e202}turn0search2\u{e201}";
        let a = citation(marker, marker, "https://a.example", "A");
        let b = citation(marker, marker, "https://b.example", "B");
        assert_eq!(
            render_annotated_text(marker, Some(&json!([a.clone(), b, a]))),
            "[A](<https://a.example>) [B](<https://b.example>)"
        );
    }

    #[test]
    fn ordinary_annotated_prose_is_preserved() {
        let text = "A sourced claim. Next sentence.";
        let annotations = json!([citation(
            text,
            "A sourced claim.",
            "https://a.example",
            "Source"
        )]);
        assert_eq!(
            render_annotated_text(text, Some(&annotations)),
            "A sourced claim. [Source](<https://a.example>) Next sentence."
        );
    }

    #[test]
    fn provider_rendered_markdown_citations_are_not_duplicated() {
        for source in [
            "([rust-lang.org](https://rust-lang.org/tools/install/))",
            "[Install Rust](<https://rust-lang.org/tools/install/>)",
        ] {
            let text = format!("A sourced claim. {source}");
            let annotations = json!([citation(
                &text,
                source,
                "https://rust-lang.org/tools/install/",
                "Install Rust"
            )]);
            assert_eq!(render_annotated_text(&text, Some(&annotations)), text);
        }
    }

    #[test]
    fn missing_or_invalid_metadata_never_hides_or_invents_sources() {
        let marker = "\u{e200}cite\u{e202}turn0search12\u{e201}";
        assert_eq!(render_annotated_text(marker, None), marker);
        for bad in [
            json!({"type":"url_citation", "start_index":0, "end_index":999, "url":"https://a.example"}),
            json!({"type":"url_citation", "start_index":9, "end_index":2, "url":"https://a.example"}),
            citation(marker, marker, "javascript:alert(1)", "Unsafe"),
            citation(marker, marker, "file:///etc/passwd", "Unsafe"),
            citation(marker, marker, "https://a.example/\u{1b}]8", "Unsafe"),
            json!({"type":"file_citation", "start_index":0, "end_index":marker.chars().count(), "url":"https://a.example"}),
            json!({"type":"url_citation", "start_index":0, "end_index":marker.chars().count()}),
        ] {
            assert_eq!(render_annotated_text(marker, Some(&json!([bad]))), marker);
        }
    }

    #[test]
    fn source_title_and_destination_cannot_inject_markdown() {
        let marker = "\u{e200}cite\u{e202}turn0search12\u{e201}";
        let annotations = json!([citation(
            marker,
            marker,
            "https://a.example/a_(b)?x=<c>",
            "A [title] *bold*\\\nnext"
        )]);
        assert_eq!(
            render_annotated_text(marker, Some(&annotations)),
            "[A \\[title\\] \\*bold\\*\\\\ next](<https://a.example/a_(b)?x=%3Cc%3E>)"
        );
    }

    #[test]
    fn unordered_annotations_are_rendered_in_text_order() {
        let first = "\u{e200}cite\u{e202}turn0search1\u{e201}";
        let second = "\u{e200}cite\u{e202}turn0search2\u{e201}";
        let text = format!("One {first}. Two {second}.");
        assert_eq!(
            render_annotated_text(
                &text,
                Some(&json!([
                    citation(&text, second, "https://b.example", "B"),
                    citation(&text, first, "https://a.example", "A"),
                ]))
            ),
            "One [A](<https://a.example>). Two [B](<https://b.example>)."
        );
    }
}
