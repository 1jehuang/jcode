fn delta_test_messages() -> Vec<crate::protocol::HistoryMessage> {
    ["one", "two", "three", "four"]
        .into_iter()
        .enumerate()
        .map(|(index, content)| crate::protocol::HistoryMessage {
            response_stats: None,
            role: if index % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_data: None,
        })
        .collect()
}

fn delta_test_base(
    messages: &[crate::protocol::HistoryMessage],
    count: usize,
) -> crate::protocol::HistoryBase {
    crate::protocol::HistoryBase {
        count,
        fingerprint: crate::protocol::history_prefix_fingerprint(
            messages[..count]
                .iter()
                .map(crate::protocol::HistoryMessageRef::from),
        ),
        images_fingerprint: None,
    }
}

#[test]
fn history_base_match_sends_only_the_tail() {
    let messages = delta_test_messages();
    let base = delta_test_base(&messages, 3);
    let (tail, base_count) = super::super::apply_history_base(messages, Some(base));
    assert_eq!(base_count, Some(3));
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].content, "four");
}

#[test]
fn history_base_full_match_sends_empty_tail() {
    let messages = delta_test_messages();
    let base = delta_test_base(&messages, 4);
    let (tail, base_count) = super::super::apply_history_base(messages, Some(base));
    assert_eq!(base_count, Some(4));
    assert!(tail.is_empty());
}

#[test]
fn history_base_mismatch_or_overlong_sends_full_transcript() {
    let messages = delta_test_messages();
    let mut edited = delta_test_messages();
    edited[1].content = "rewound".to_string();
    let stale = delta_test_base(&edited, 3);
    let (full, base_count) = super::super::apply_history_base(messages.clone(), Some(stale));
    assert_eq!((full.len(), base_count), (4, None));

    let mut longer = delta_test_messages();
    longer.push(longer[0].clone());
    let overlong = delta_test_base(&longer, 5);
    let (full, base_count) = super::super::apply_history_base(messages.clone(), Some(overlong));
    assert_eq!((full.len(), base_count), (4, None));

    let (full, base_count) = super::super::apply_history_base(messages, None);
    assert_eq!((full.len(), base_count), (4, None));
}

#[test]
fn history_images_base_omits_only_matching_image_sets() {
    let image = |data: &str| crate::session::RenderedImage {
        media_type: "image/png".to_string(),
        data: data.to_string(),
        label: None,
        source: crate::session::RenderedImageSource::UserInput,
        anchor: None,
        history_message_index: None,
    };
    let images = vec![image("a"), image("b")];
    let base = |fingerprint| crate::protocol::HistoryBase {
        count: 1,
        fingerprint: 0,
        images_fingerprint: Some(fingerprint),
    };
    let matching = crate::protocol::history_images_fingerprint(&images);
    let (sent, unchanged) =
        super::super::apply_history_images_base(images.clone(), Some(base(matching)));
    assert!(unchanged && sent.is_empty());
    let other = crate::protocol::history_images_fingerprint(&[image("a")]);
    let (sent, unchanged) =
        super::super::apply_history_images_base(images.clone(), Some(base(other)));
    assert!(!unchanged && sent.len() == 2);
    let (sent, unchanged) = super::super::apply_history_images_base(images, None);
    assert!(!unchanged && sent.len() == 2);
}
