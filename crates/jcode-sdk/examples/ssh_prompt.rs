//! Create a session over SSH, send one prompt, print the reply.
//! Usage: ssh_prompt <host> <user> <identity> <known_hosts> [prompt]
use jcode_harness_api::ApiEvent;
use jcode_sdk::{JcodeClient, SshConnectOptions};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let prompt = a
        .get(4)
        .cloned()
        .unwrap_or_else(|| "Reply with exactly: SDK_OK".into());
    let client = JcodeClient::connect_ssh(SshConnectOptions {
        user: Some(a[1].clone()),
        identity_file: Some(a[2].clone().into()),
        known_hosts_file: Some(a[3].clone().into()),
        isolated_config: true,
        connect_timeout: Duration::from_secs(60),
        ..SshConnectOptions::new(a[0].clone())
    })?;
    let session = client.create_session(None)?;
    eprintln!("session {}", session.session_id);
    let events = client.events(Some(&session.session_id));
    client.send_message(&session.session_id, &prompt, vec![], None)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let mut text = String::new();
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match events.next_timeout(left) {
            Some(ApiEvent::TextDelta { text: t, .. }) => text.push_str(&t),
            Some(ApiEvent::TurnDone { .. }) => break,
            Some(ApiEvent::TurnStopped {
                message, reason, ..
            }) => {
                eprintln!("turn stopped: {reason:?} {message}");
                break;
            }
            Some(ApiEvent::Error { message, .. }) => {
                eprintln!("error: {message}");
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    println!("{text}");
    Ok(())
}
