#![cfg(unix)]

use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame,
};
use jcode_sdk::{ConnectOptions, JcodeClient, RunStructuredError, RunStructuredOptions, Transport};
use serde::Deserialize;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct PairTransport(UnixStream);

impl Transport for PairTransport {
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self.0.try_clone().expect("socket pair must clone");
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

fn fake_harness(handle: impl Fn(&ClientFrame, &mut dyn Write) + Send + 'static) -> JcodeClient {
    let (ours, theirs) = UnixStream::pair().expect("socket pair");
    std::thread::spawn(move || {
        let mut reader = BufReader::new(theirs.try_clone().expect("clone"));
        let mut writer = theirs;
        while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
            if let ApiRequest::Hello { .. } = frame.request {
                reply(
                    &frame,
                    ApiEvent::HelloOk {
                        version: API_VERSION_MAJOR,
                        server: "structured-test/1.0".to_string(),
                        capabilities: vec!["sessions".to_string()],
                    },
                    &mut writer,
                );
                continue;
            }
            handle(&frame, &mut writer);
        }
    });
    JcodeClient::connect_with(
        Box::new(PairTransport(ours)),
        ConnectOptions {
            request_timeout: Some(Duration::from_secs(5)),
            ensure_runtime: false,
            ..Default::default()
        },
    )
    .expect("handshake")
}

fn reply(frame: &ClientFrame, event: ApiEvent, writer: &mut dyn Write) {
    write_frame(
        &mut { writer },
        &ServerFrame {
            v: API_VERSION_MAJOR,
            reply_to: Some(frame.id),
            event,
        },
    )
    .expect("reply");
}

fn push(event: ApiEvent, writer: &mut dyn Write) {
    write_frame(
        &mut { writer },
        &ServerFrame {
            v: API_VERSION_MAJOR,
            reply_to: None,
            event,
        },
    )
    .expect("event");
}

fn send_turn(writer: &mut dyn Write, session_id: &str, text: &str) {
    push(
        ApiEvent::MessageAccepted {
            session_id: session_id.to_string(),
        },
        writer,
    );
    push(
        ApiEvent::TextDelta {
            session_id: session_id.to_string(),
            text: text.to_string(),
        },
        writer,
    );
    push(
        ApiEvent::TurnDone {
            session_id: session_id.to_string(),
        },
        writer,
    );
}

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "count"],
        "properties": {
            "summary": { "type": "string" },
            "count": { "type": "integer", "minimum": 0 }
        }
    })
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Summary {
    summary: String,
    count: u64,
}

#[test]
fn validates_fenced_json_and_returns_parsed_data() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&prompts);
    let client = fake_harness(move |frame, writer| {
        if let ApiRequest::SendMessage {
            session_id,
            content,
            ..
        } = &frame.request
        {
            seen.lock().unwrap().push(content.clone());
            send_turn(
                writer,
                session_id,
                "```json\n{\"summary\":\"done\",\"count\":2}\n```",
            );
        }
    });

    let result = client
        .run_structured::<Summary>(
            "s1",
            "Summarize the work",
            RunStructuredOptions::new(schema()),
        )
        .expect("structured turn");

    assert_eq!(
        result.data,
        Summary {
            summary: "done".to_string(),
            count: 2,
        }
    );
    assert_eq!(
        result.text,
        "```json\n{\"summary\":\"done\",\"count\":2}\n```"
    );
    assert_eq!(result.attempts.len(), 1);
    assert!(result.attempts[0].errors.is_empty());
    let prompts = prompts.lock().unwrap();
    assert!(prompts[0].contains("Return the answer as JSON only"));
    assert!(prompts[0].contains("\"additionalProperties\": false"));
}

#[test]
fn retries_with_validation_details_then_returns_the_correction() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&prompts);
    let responses = [
        "{\"summary\":42,\"count\":2}",
        "{\"summary\":\"fixed\",\"count\":2}",
    ];
    let client = fake_harness(move |frame, writer| {
        if let ApiRequest::SendMessage {
            session_id,
            content,
            ..
        } = &frame.request
        {
            let index = {
                let mut prompts = seen.lock().unwrap();
                let index = prompts.len();
                prompts.push(content.clone());
                index
            };
            send_turn(writer, session_id, responses[index]);
        }
    });
    let mut options = RunStructuredOptions::new(schema());
    options.max_retries = 1;

    let result = client
        .run_structured::<Summary>("s1", "Return a summary", options)
        .expect("corrected structured turn");

    assert_eq!(result.data.summary, "fixed");
    assert_eq!(result.attempts.len(), 2);
    assert_eq!(result.attempts[0].attempt, 1);
    assert_eq!(result.attempts[0].errors[0].path, "/summary");
    assert_eq!(result.attempts[0].errors[0].keyword, "type");
    assert_eq!(result.attempts[0].errors[0].message, "must be string");
    assert!(result.attempts[1].errors.is_empty());
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("Validation errors:"));
    assert!(prompts[1].contains("/summary must be string"));
    assert!(prompts[1].contains("\"summary\":42"));
}

#[test]
fn exhaustion_reports_every_bounded_attempt_and_last_parse_error() {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&prompts);
    let client = fake_harness(move |frame, writer| {
        if let ApiRequest::SendMessage {
            session_id,
            content,
            ..
        } = &frame.request
        {
            seen.lock().unwrap().push(content.clone());
            send_turn(writer, session_id, "not json");
        }
    });
    let mut options = RunStructuredOptions::new(schema());
    options.max_retries = 1;

    let error = client
        .run_structured::<Summary>("s1", "Return a summary", options)
        .expect_err("invalid output must exhaust retries");

    assert_eq!(error.code(), "structured_output_invalid");
    let RunStructuredError::InvalidOutput(error) = error else {
        panic!("expected invalid output error");
    };
    assert_eq!(error.attempts.len(), 2);
    assert_eq!(error.validation_errors[0].keyword, "parse");
    assert_eq!(error.last_text, "not json");
    assert!(error.to_string().contains("after 2 attempts"));
    assert_eq!(prompts.lock().unwrap().len(), 2);
}

#[test]
fn invalid_schema_fails_before_any_model_turn() {
    let requests = Arc::new(Mutex::new(0usize));
    let seen = Arc::clone(&requests);
    let client = fake_harness(move |_, _| *seen.lock().unwrap() += 1);

    let error = client
        .run_structured::<serde_json::Value>(
            "s1",
            "Return anything",
            RunStructuredOptions::new(json!({ "type": "not-a-json-schema-type" })),
        )
        .expect_err("invalid schema");

    assert_eq!(error.code(), "structured_schema_invalid");
    assert!(matches!(error, RunStructuredError::InvalidSchema(_)));
    assert_eq!(*requests.lock().unwrap(), 0);
}
