use super::*;

#[test]
fn strip_all_images_replaces_every_image_with_a_note() {
    let mut a = vec![
        ContentBlock::Text {
            text: "keep".into(),
            cache_control: None,
        },
        ContentBlock::Image {
            media_type: "image/bmp".into(),
            data: "Qk0=".into(),
        },
    ];
    let mut b = vec![ContentBlock::Image {
        media_type: "image/png".into(),
        data: "iVBO".into(),
    }];
    let mut contents = vec![&mut a, &mut b];
    assert_eq!(strip_all_images_in_contents(&mut contents), 2);
    assert!(matches!(&a[0], ContentBlock::Text { text, .. } if text == "keep"));
    assert!(
        matches!(&a[1], ContentBlock::Text { text, .. } if text.contains("media_type=image/bmp"))
    );
    assert!(matches!(&b[0], ContentBlock::Text { .. }));
}
