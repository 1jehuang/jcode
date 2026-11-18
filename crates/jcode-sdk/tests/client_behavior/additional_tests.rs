use super::*;

/// The handshake populates the server identity and capabilities, which is how
/// a client decides what it may depend on.
#[test]
fn the_handshake_reports_the_server_and_its_capabilities() {
    let client = fake_harness(|_, _| {});
    assert_eq!(client.server, "fake-harness/1.0");
    assert!(client.supports("sessions"));
    assert!(
        !client.supports("permissions"),
        "a capability the server did not advertise must not be claimed"
    );
}

/// Replies must go to the request that asked for them, not to whichever one
/// happens to be waiting. Answered out of order on purpose: in-order delivery
/// would pass even with the correlation removed entirely.
#[test]
fn out_of_order_replies_reach_the_right_waiter() {
    let client = fake_harness(|frame, writer| match &frame.request {
        // Delay the first request's reply so the second overtakes it.
        ApiRequest::AttachSession { session_id } => {
            let id = session_id.clone();
            std::thread::sleep(Duration::from_millis(120));
            reply(
                frame,
                ApiEvent::Attached {
                    session: session(&id),
                },
                writer,
            );
        }
        ApiRequest::Ping => reply(frame, ApiEvent::Pong, writer),
        _ => {}
    });

    let slow = {
        let client = client.clone();
        std::thread::spawn(move || client.attach_session("slow-session"))
    };
    std::thread::sleep(Duration::from_millis(20));
    client
        .ping()
        .expect("the fast request must not wait on the slow one");
    let attached = slow.join().expect("thread").expect("attach must succeed");
    assert_eq!(attached.session_id, "slow-session");
}
