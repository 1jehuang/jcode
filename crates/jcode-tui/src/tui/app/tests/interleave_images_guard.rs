/// Every place that consumes a staged interleave message must also carry its
/// staged images.
///
/// This is a source-level guard because the bug it prevents is invisible to
/// ordinary tests: one send path passed `vec![]` instead of the staged
/// attachments, which compiled cleanly, type-checked, and silently dropped every
/// image on an interleaved send. No panic, no error, no failing assertion.
/// `test_interleave_submission_preserves_pending_images` only proves the images
/// reach `app.interleave_images`; it cannot see a consumer that then ignores
/// them, and exercising the real consumer needs a live remote connection.
///
/// So assert the pairing directly: wherever `interleave_message.take()` happens,
/// a matching take of `interleave_images` must happen too, and the taken value
/// must be forwarded rather than replaced by an empty vector.
///
/// Verified by reintroducing the original bug: this fails with a message naming
/// the mismatch (2 paths consume the message, only 1 takes the images).
#[test]
fn every_interleave_send_path_carries_the_staged_images() {
    let source = include_str!("../remote.rs");

    let takes = source.matches("interleave_message.take()").count();
    assert!(
        takes > 0,
        "expected at least one interleave send path in remote.rs; if this moved, \
         move this guard with it rather than deleting it"
    );

    let image_takes = source
        .matches("std::mem::take(&mut app.interleave_images)")
        .count();
    assert_eq!(
        image_takes, takes,
        "every interleave send path must take the staged images ({takes} paths \
         consume interleave_message but only {image_takes} take \
         interleave_images). A path that omits this silently drops the user's \
         attachments while still compiling."
    );

    assert!(
        !source.contains("interleave_msg, vec![], false"),
        "an interleave send is passing an empty image vector instead of the \
         staged attachments, which silently drops them"
    );
}
