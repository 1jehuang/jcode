use super::*;
use tokio::sync::mpsc;

/// A context for a session of its own, routed to `tx` when given.
fn context(tx: Option<mpsc::UnboundedSender<ClientInputRequest>>) -> ToolContext {
    let session_id = crate::id::new_id("ask-user-test");
    if let Some(tx) = tx {
        route::sync(&session_id, vec![(session_id.clone(), tx)]);
    }
    ToolContext {
        session_id,
        message_id: "msg".into(),
        tool_call_id: "call_1".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    }
}

fn question(text: &str, labels: &[&str], multi: bool) -> Value {
    json!({
        "question": text,
        "header": "Pick",
        "options": labels.iter().map(|l| json!({"label": l, "description": "d"})).collect::<Vec<_>>(),
        "multiSelect": multi,
    })
}

/// Run the tool with a client that replies with `reply` (`None` drops the sender).
async fn run_with_reply(
    input: Value,
    reply: Option<Option<Vec<AskUserAnswer>>>,
) -> (String, Vec<AskUserQuestion>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = tokio::spawn(async move {
        let Some(ClientInputRequest::AskUser(request)) = rx.recv().await else {
            panic!("expected an ask_user request");
        };
        assert_eq!(request.tool_call_id, "call_1");
        let questions = request.questions.clone();
        if let Some(reply) = reply {
            let _ = request.response_tx.send(reply);
        }
        questions
    });
    let output = AskUserTool.execute(input, context(Some(tx))).await.unwrap();
    (output.output, client.await.unwrap())
}

fn answer(question: &str, answers: &[&str]) -> AskUserAnswer {
    AskUserAnswer {
        question: question.into(),
        answers: answers.iter().map(|a| a.to_string()).collect(),
    }
}

#[tokio::test]
async fn rejects_invalid_question_sets() {
    let ok = question("A?", &["x", "y"], false);
    for input in [
        json!({"questions": []}),
        json!({"questions": [ok.clone(), question("B?", &["x", "y"], false), question("C?", &["x", "y"], false), question("D?", &["x", "y"], false), question("E?", &["x", "y"], false)]}),
        json!({"questions": [question("A?", &["x"], false)]}),
        json!({"questions": [question("A?", &["a", "b", "c", "d", "e"], false)]}),
        json!({"questions": [ok.clone(), ok.clone()]}),
        json!({"questions": [question("A?", &["x", "x"], false)]}),
        json!({"questions": [ok.clone()], "extra": 1}),
        json!({"questions": [{"question": "A?", "header": "h", "options": [], "bogus": true}]}),
    ] {
        assert!(
            AskUserTool
                .execute(input.clone(), context(None))
                .await
                .is_err(),
            "{input}"
        );
    }
}

#[tokio::test]
async fn without_a_client_tells_the_model_to_proceed() {
    let input = json!({"questions": [question("A?", &["x", "y"], false)], "intent": "Ask"});
    let output = AskUserTool.execute(input, context(None)).await.unwrap();
    assert_eq!(output.output, NO_USER);
}

#[tokio::test]
async fn formats_single_multi_other_and_unanswered() {
    let input = json!({"questions": [
        question("Which db?", &["Postgres (Recommended)", "SQLite"], false),
        question("Which extras?", &["Auth", "Cache", "Queue"], true),
        question("Name?", &["foo", "bar"], false),
        question("Skip?", &["a", "b"], true),
    ]});
    let reply = vec![
        answer("Which db?", &["SQLite"]),
        answer("Which extras?", &["Auth", "Queue", "my own thing"]),
        answer("Name?", &["custom name"]),
        answer("Skip?", &[]),
    ];
    let (output, questions) = run_with_reply(input, Some(Some(reply))).await;
    assert_eq!(questions.len(), 4);
    assert!(questions[1].multi_select && !questions[0].multi_select);
    assert_eq!(
        output,
        r#"The user answered: "Which db?"="SQLite", "Which extras?"="Auth, Queue, my own thing", "Name?"="custom name", "Skip?"=(no answer). Follow what they actually said."#
    );
}

#[tokio::test]
async fn dismissed_and_disconnected_replies() {
    let input = json!({"questions": [question("A?", &["x", "y"], false)]});
    let (output, _) = run_with_reply(input.clone(), Some(None)).await;
    assert_eq!(output, DISMISSED);
    let (output, _) = run_with_reply(input, None).await;
    assert_eq!(output, NO_USER);
}

#[tokio::test]
async fn choices_with_commas_or_quotes_are_json_quoted() {
    let input = json!({"questions": [question("Pick?", &["a, b", "c"], true)]});
    let reply = vec![answer("Pick?", &["a, b", "c", r#"say "hi""#])];
    let (output, _) = run_with_reply(input, Some(Some(reply))).await;
    assert_eq!(
        output,
        r#"The user answered: "Pick?"="\"a, b\", c, \"say \\\"hi\\\"\"". Follow what they actually said."#
    );
}

#[tokio::test]
async fn interrupt_closes_the_dialog_and_frees_the_reply() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let mut ctx = context(Some(tx));
    ctx.graceful_shutdown_signal = Some(signal.clone());
    let input = json!({"questions": [question("A?", &["x", "y"], false)]});
    let call = tokio::spawn(async move { AskUserTool.execute(input, ctx).await.unwrap() });
    let Some(ClientInputRequest::AskUser(request)) = rx.recv().await else {
        panic!("expected an ask_user request");
    };
    signal.fire();
    assert_eq!(call.await.unwrap().output, DISMISSED);
    let Some(ClientInputRequest::AskUserClosed(closed)) = rx.recv().await else {
        panic!("expected the dialog to be closed");
    };
    assert_eq!(closed, request.request_id);
}
