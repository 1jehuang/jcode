use super::*;

#[test]
fn soft_interrupt_with_images_preserves_attachments_and_legacy_helper() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&requests);
    let client = fake_harness(move |frame, writer| {
        seen.lock()
            .expect("request log")
            .push(frame.request.clone());
        reply(frame, ApiEvent::Ok, writer);
    });

    client
        .soft_interrupt_with_images(
            "s1",
            "look",
            vec![("image/png".into(), "aW1hZ2U=".into())],
            true,
        )
        .expect("image interrupt");
    client
        .soft_interrupt("s1", "text only", false)
        .expect("legacy helper");

    let requests = requests.lock().expect("request log");
    assert!(matches!(
        &requests[0],
        ApiRequest::SoftInterrupt {
            session_id,
            content,
            images,
            urgent: true,
            ..
        } if session_id == "s1"
            && content == "look"
            && images == &vec![("image/png".into(), "aW1hZ2U=".into())]
    ));
    assert!(matches!(
        &requests[1],
        ApiRequest::SoftInterrupt {
            images,
            urgent: false,
            ..
        } if images.is_empty()
    ));
}
