//! `ask_user`: Claude's `AskUserQuestion` input compiled into one ACP flat form.
//!
//! The broker validates each field independently, so this module owns the
//! cross-field "Other" normalisation and the escaped tool-result formatting.
use crate::elicitation::ElicitOutcome;
use crate::protocol::elicitation::FormSchema;
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

/// Root and per-field layout tag. Clients must ignore keys they do not know.
const TAG: &str = "jcode.dev/askUser";
/// Reserved choice that stands in for free-form "Other" text.
const OTHER: &str = "__other__";
/// Maximum code points for the optional `q{i}_other` and `q{i}_notes` fields.
const MAX_TEXT: usize = 8192;
const TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

pub struct AskUserTool;

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user one to four structured questions and block until they answer. \
         Each question has 2-4 options and is single-select or multi-select. The user \
         can always choose Other and add free-form text and notes. Use this when you \
         need a decision, preference or missing detail before continuing."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question to ask the user. Clear, specific, and ending with a question mark."
                            },
                            "header": {
                                "type": "string",
                                "description": "Very short label shown as the question tab (at most 12 display characters)."
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Display text for the option (1-5 words)."
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Explanation of what this option means or what happens if it is chosen."
                                        },
                                        "preview": {
                                            "type": "string",
                                            "description": "Optional preview content shown while the option is highlighted."
                                        }
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Allow the user to select more than one option."
                            }
                        },
                        "required": ["question", "header", "options", "multiSelect"]
                    }
                },
                "answers": {
                    "type": "object",
                    "description": "Ignored. Accepted for Claude AskUserQuestion compatibility."
                },
                "annotations": {
                    "type": "object",
                    "description": "Ignored. Accepted for Claude AskUserQuestion compatibility."
                },
                "metadata": {
                    "type": "object",
                    "description": "Ignored. Accepted for Claude AskUserQuestion compatibility."
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let questions = parse_questions(&input)?;
        let schema = build_form(&questions)?;
        let message = if questions.len() == 1 {
            "The agent has a question.".to_string()
        } else {
            format!("The agent has {} questions.", questions.len())
        };
        let Some(ticket) = crate::elicitation::global()
            .start(
                &ctx.session_id,
                Some(ctx.tool_call_id.clone()),
                message,
                schema,
            )
            .map_err(|error| anyhow!(error))?
        else {
            bail!(
                "No attached client can display questions; ask the user in your normal reply instead."
            );
        };
        let outcome = ticket.wait(interrupted(&ctx), TIMEOUT).await;
        format_outcome(&questions, outcome).map(ToolOutput::new)
    }
}

struct OptionItem {
    label: String,
    description: String,
    preview: Option<String>,
}

struct Question {
    question: String,
    header: String,
    options: Vec<OptionItem>,
    multi_select: bool,
}

/// Validate the model-facing shape before anything reaches the broker.
fn parse_questions(input: &Value) -> Result<Vec<Question>> {
    let list = input
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("`questions` must be an array"))?;
    if list.is_empty() || list.len() > 4 {
        bail!("ask_user needs 1-4 questions");
    }
    let mut question_texts = HashSet::new();
    let mut questions = Vec::with_capacity(list.len());
    for value in list {
        let question = text_field(value, "question")?;
        let header = text_field(value, "header")?;
        if UnicodeWidthStr::width(header.as_str()) > 12 {
            bail!("question header `{header}` exceeds 12 display cells");
        }
        if !question_texts.insert(question.clone()) {
            bail!("duplicate question text `{question}`");
        }
        let options = value
            .get("options")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("`options` must be an array"))?;
        if options.len() < 2 || options.len() > 4 {
            bail!("each question needs 2-4 options");
        }
        let mut labels = HashSet::new();
        let mut parsed = Vec::with_capacity(options.len());
        for option in options {
            let label = text_field(option, "label")?;
            if label == "Other" || label == OTHER {
                bail!("`{label}` is a reserved option label");
            }
            if !labels.insert(label.clone()) {
                bail!("duplicate option label `{label}`");
            }
            let description = option
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let preview = option
                .get("preview")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(preview) = &preview
                && preview.len() > MAX_TEXT
            {
                bail!("option preview exceeds {MAX_TEXT} bytes");
            }
            parsed.push(OptionItem {
                label,
                description,
                preview,
            });
        }
        let multi_select = value
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        questions.push(Question {
            question,
            header,
            options: parsed,
            multi_select,
        });
    }
    Ok(questions)
}

fn text_field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("`{key}` must be a string"))
}

/// Questions become `q1..q4`, each with optional `q{i}_other` and `q{i}_notes`.
fn build_form(questions: &[Question]) -> Result<FormSchema> {
    let mut properties = Map::new();
    let mut required = Vec::with_capacity(questions.len());
    for (index, question) in questions.iter().enumerate() {
        let field = format!("q{}", index + 1);
        let other_field = format!("{field}_other");
        let notes_field = format!("{field}_notes");
        let mut choices: Vec<Value> = question
            .options
            .iter()
            .map(|option| {
                let mut choice = json!({"const": option.label, "title": option.label});
                if !option.description.is_empty() {
                    choice["description"] = json!(option.description);
                }
                if let Some(preview) = &option.preview {
                    choice["_meta"] = json!({"preview": preview});
                }
                choice
            })
            .collect();
        choices.push(json!({"const": OTHER, "title": "Other"}));
        let mut property = if question.multi_select {
            json!({
                "type": "array",
                "minItems": 1,
                "title": question.header,
                "description": question.question,
                "items": {"anyOf": choices}
            })
        } else {
            json!({
                "type": "string",
                "title": question.header,
                "description": question.question,
                "oneOf": choices
            })
        };
        let mut field_meta = Map::new();
        field_meta.insert(
            TAG.into(),
            json!({
                "header": question.header,
                "otherField": other_field,
                "notesField": notes_field
            }),
        );
        property["_meta"] = Value::Object(field_meta);
        properties.insert(field.clone(), property);
        properties.insert(
            other_field,
            json!({"type": "string", "title": "Other", "maxLength": MAX_TEXT}),
        );
        properties.insert(
            notes_field,
            json!({"type": "string", "title": "Notes", "maxLength": MAX_TEXT}),
        );
        required.push(Value::String(field));
    }
    let mut meta = Map::new();
    meta.insert(TAG.into(), json!({"version": 1}));
    let schema = FormSchema(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "_meta": meta
    }));
    schema.validate().map_err(|error| anyhow!(error))?;
    Ok(schema)
}

fn format_outcome(questions: &[Question], outcome: ElicitOutcome) -> Result<String> {
    match outcome {
        ElicitOutcome::Accepted(content) => format_answers(questions, &content),
        ElicitOutcome::Declined => {
            Ok("User declined to answer your questions; proceed with best judgment.".into())
        }
        ElicitOutcome::Cancelled => {
            Ok("The question was cancelled before the user answered.".into())
        }
        ElicitOutcome::Expired => Ok("The question expired before the user answered.".into()),
        ElicitOutcome::Unsupported => Err(anyhow!(
            "No attached client can display questions; ask the user in your normal reply instead."
        )),
    }
}

/// One escaped line per question, with JSON arrays preserved for multi-select.
fn format_answers(questions: &[Question], content: &Map<String, Value>) -> Result<String> {
    let mut lines = Vec::with_capacity(questions.len());
    for (index, question) in questions.iter().enumerate() {
        let field = format!("q{}", index + 1);
        let other = content
            .get(&format!("{field}_other"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let notes = content
            .get(&format!("{field}_notes"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let selected = content.get(&field);
        let (answer, also_wrote) = if question.multi_select {
            let labels = selected
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("answer for `{field}` must be an array"))?;
            let mut values = Vec::with_capacity(labels.len() + 1);
            let mut substituted = false;
            for label in labels.iter().filter_map(Value::as_str) {
                if label == OTHER {
                    substituted = true;
                    values.push(Value::String(
                        other
                            .clone()
                            .unwrap_or_else(|| "Other (no text given)".into()),
                    ));
                } else {
                    values.push(Value::String(label.into()));
                }
            }
            if let Some(text) = &other
                && !substituted
            {
                values.push(Value::String(text.clone()));
            }
            (Value::Array(values), None)
        } else {
            let label = selected
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("answer for `{field}` must be a string"))?;
            if label == OTHER {
                (
                    Value::String(
                        other
                            .clone()
                            .unwrap_or_else(|| "Other (no text given)".into()),
                    ),
                    None,
                )
            } else {
                (Value::String(label.into()), other.clone())
            }
        };
        let mut line = format!(
            "{}={}",
            serde_json::to_string(&question.question)?,
            serde_json::to_string(&answer)?
        );
        if let Some(text) = &also_wrote {
            line.push_str(&format!(
                " (user also wrote: {})",
                serde_json::to_string(text)?
            ));
        }
        if let Some(text) = &notes {
            line.push_str(&format!(" (user notes: {})", serde_json::to_string(text)?));
            let previews = selected_previews(question, selected);
            if !previews.is_empty() {
                line.push_str(&format!(
                    " (previews for {})",
                    serde_json::to_string(&previews)?
                ));
            }
        }
        lines.push(line);
    }
    let mut output = String::from("User has answered your questions:\n");
    for line in lines {
        output.push_str(&line);
        output.push('\n');
    }
    output.push_str("You can now continue with the user's answers in mind.");
    Ok(output)
}

/// Labels among the selection whose options carry a preview, in selection order.
fn selected_previews(question: &Question, selected: Option<&Value>) -> Vec<String> {
    let labels: Vec<&str> = match selected {
        Some(Value::String(label)) => vec![label.as_str()],
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    labels
        .into_iter()
        .filter(|label| *label != OTHER)
        .filter(|label| {
            question
                .options
                .iter()
                .any(|option| option.label == *label && option.preview.is_some())
        })
        .map(str::to_string)
        .collect()
}

async fn interrupted(ctx: &ToolContext) {
    match &ctx.graceful_shutdown_signal {
        Some(signal) => {
            if !signal.is_set() {
                signal.notified().await;
            }
        }
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ServerEvent;
    use crate::protocol::elicitation::ElicitationResponse;
    use serde_json::json;
    use tokio::sync::mpsc as tokio_mpsc;

    fn option(label: &str, description: &str) -> Value {
        json!({"label": label, "description": description})
    }

    fn question(text: &str, header: &str, options: Value, multi: bool) -> Value {
        json!({
            "question": text,
            "header": header,
            "options": options,
            "multiSelect": multi
        })
    }

    fn input(questions: Value) -> Value {
        json!({"questions": questions})
    }

    fn parse(value: &Value) -> Vec<Question> {
        parse_questions(value).unwrap()
    }

    fn content(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn answer_line(output: &str, index: usize) -> &str {
        output.lines().nth(index + 1).unwrap()
    }

    fn decode_answer(line: &str) -> Value {
        let rest = &line[line.find('=').unwrap() + 1..];
        serde_json::Deserializer::from_str(rest)
            .into_iter::<Value>()
            .next()
            .unwrap()
            .unwrap()
    }

    fn ctx(session: &str) -> ToolContext {
        ToolContext {
            session_id: session.into(),
            message_id: "message".into(),
            tool_call_id: "call".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
        }
    }

    #[test]
    fn form_schema_uses_flat_fields_with_meta_tags() {
        let questions = parse(&input(json!([
            question(
                "Pick one",
                "Pick",
                json!([option("A", "first"), option("B", "second")]),
                false
            ),
            question(
                "Pick many",
                "Many",
                json!([option("C", "third"), option("D", "fourth")]),
                true
            )
        ])));
        let schema = build_form(&questions).unwrap();
        let value = &schema.0;
        assert_eq!(value["type"], "object");
        assert_eq!(value["_meta"][TAG]["version"], 1);
        assert_eq!(value["required"], json!(["q1", "q2"]));

        let single = &value["properties"]["q1"];
        assert_eq!(single["type"], "string");
        assert_eq!(single["title"], "Pick");
        assert_eq!(single["description"], "Pick one");
        assert_eq!(single["oneOf"][0]["const"], "A");
        assert_eq!(single["oneOf"][0]["description"], "first");
        assert_eq!(single["oneOf"][2]["const"], OTHER);
        assert_eq!(single["oneOf"][2]["title"], "Other");
        assert_eq!(single["_meta"][TAG]["header"], "Pick");
        assert_eq!(single["_meta"][TAG]["otherField"], "q1_other");
        assert_eq!(single["_meta"][TAG]["notesField"], "q1_notes");

        let multi = &value["properties"]["q2"];
        assert_eq!(multi["type"], "array");
        assert_eq!(multi["minItems"], 1);
        assert_eq!(multi["items"]["anyOf"][1]["const"], "D");
        assert_eq!(multi["items"]["anyOf"][2]["const"], OTHER);

        assert_eq!(value["properties"]["q1_other"]["maxLength"], 8192);
        assert_eq!(value["properties"]["q1_notes"]["maxLength"], 8192);
        assert_eq!(value["properties"]["q2_other"]["maxLength"], 8192);
        assert_eq!(value["properties"]["q2_notes"]["maxLength"], 8192);
    }

    #[test]
    fn previews_land_in_option_meta() {
        let questions = parse(&input(json!([question(
            "Pick one",
            "Pick",
            json!([
                json!({"label":"A","description":"a","preview":"full text"}),
                option("B", "b")
            ]),
            false
        )])));
        let schema = build_form(&questions).unwrap();
        assert_eq!(
            schema.0["properties"]["q1"]["oneOf"][0]["_meta"]["preview"],
            "full text"
        );
        assert!(
            schema.0["properties"]["q1"]["oneOf"][1]
                .get("_meta")
                .is_none()
        );
    }

    #[test]
    fn input_checks_reject_bad_shapes() {
        let good = question(
            "Pick",
            "Pick",
            json!([option("A", "a"), option("B", "b")]),
            false,
        );
        for bad in [
            input(json!([])),
            input(json!([good.clone(), good.clone()])),
            input(json!([question(
                "Pick",
                "Pick",
                json!([option("A", "a")]),
                false
            )])),
            input(json!([question(
                "Pick",
                "Pick",
                json!([
                    option("A", "a"),
                    option("B", "b"),
                    option("C", "c"),
                    option("D", "d"),
                    option("E", "e")
                ]),
                false
            )])),
            input(json!([question(
                "Pick",
                "Pick",
                json!([option("A", "a"), option("A", "b")]),
                false
            )])),
            input(json!([question(
                "Pick",
                "Pick",
                json!([option("Other", "a"), option("B", "b")]),
                false
            )])),
            input(json!([question(
                "Pick",
                "Pick",
                json!([option(OTHER, "a"), option("B", "b")]),
                false
            )])),
            input(json!([question(
                "Pick",
                "Unicode width is not bytes",
                json!([option("A", "a"), option("B", "b")]),
                false
            )])),
            input(json!([question(
                "Pick",
                "Pick",
                json!([
                    json!({"label":"A","description":"a","preview":"x".repeat(8193)}),
                    option("B", "b")
                ]),
                false
            )])),
        ] {
            assert!(parse_questions(&bad).is_err(), "{bad}");
        }
        assert!(parse_questions(&input(json!([good]))).is_ok());
    }

    #[test]
    fn header_limit_counts_display_cells() {
        let twelve = "界".repeat(6);
        let thirteen = "界".repeat(7);
        let build = |header: &str| {
            input(json!([question(
                "Pick",
                header,
                json!([option("A", "a"), option("B", "b")]),
                false
            )]))
        };
        assert!(parse_questions(&build(&twelve)).is_ok());
        assert!(parse_questions(&build(&thirteen)).is_err());
    }

    #[test]
    fn over_limit_schema_strings_are_rejected_by_validation() {
        let questions = parse(&input(json!([question(
            "Pick",
            "Pick",
            json!([option("A", &"x".repeat(9000)), option("B", "b")]),
            false
        )])));
        assert!(build_form(&questions).is_err());
    }

    #[test]
    fn other_rules_table_is_normalised() {
        let one = parse(&input(json!([question(
            "One",
            "One",
            json!([option("A", "a"), option("B", "b")]),
            false
        )])));
        let many = parse(&input(json!([question(
            "Many",
            "Many",
            json!([option("A", "a"), option("B", "b")]),
            true
        )])));

        let single = |value: Value| format_answers(&one, &content(value)).unwrap();
        let multi = |value: Value| format_answers(&many, &content(value)).unwrap();
        let first = |output: String| decode_answer(answer_line(&output, 0)).clone();

        assert_eq!(first(single(json!({"q1": "A"}))), json!("A"));

        let with_text = single(json!({"q1": "A", "q1_other": "  typed  "}));
        assert_eq!(decode_answer(answer_line(&with_text, 0)), json!("A"));
        assert!(answer_line(&with_text, 0).contains("(user also wrote: \"typed\")"));

        assert_eq!(
            decode_answer(answer_line(
                &single(json!({"q1": OTHER, "q1_other": "typed"})),
                0
            )),
            json!("typed")
        );
        assert_eq!(
            decode_answer(answer_line(&single(json!({"q1": OTHER})), 0)),
            json!("Other (no text given)")
        );

        let multi_text = multi(json!({"q1": ["A", "B"], "q1_other": "typed"}));
        assert_eq!(
            decode_answer(answer_line(&multi_text, 0)),
            json!(["A", "B", "typed"])
        );
        assert!(!answer_line(&multi_text, 0).contains("user also wrote"));

        assert_eq!(
            decode_answer(answer_line(
                &multi(json!({"q1": ["A", OTHER], "q1_other": "typed"})),
                0
            )),
            json!(["A", "typed"])
        );
        assert_eq!(
            decode_answer(answer_line(&multi(json!({"q1": [OTHER]})), 0)),
            json!(["Other (no text given)"])
        );
    }

    #[test]
    fn result_escapes_values_and_keeps_selection_boundaries() {
        let question_text = "Pick, \"one\"\nなに 🎉";
        let questions = parse(&input(json!([question(
            question_text,
            "Pick",
            json!([option("B, C", "x"), option("D\"E", "y")]),
            true
        )])));
        let notes = "keep line 1\nthen line 2";
        let output = format_answers(
            &questions,
            &content(json!({"q1": ["B, C", "D\"E"], "q1_notes": notes})),
        )
        .unwrap();
        let line = answer_line(&output, 0);
        assert!(line.starts_with(&serde_json::to_string(question_text).unwrap()));
        assert_eq!(decode_answer(line), json!(["B, C", "D\"E"]));
        assert!(line.contains(&format!(
            "user notes: {}",
            serde_json::to_string(notes).unwrap()
        )));
        assert!(output.starts_with("User has answered your questions:\n"));
        assert!(output.ends_with("You can now continue with the user's answers in mind."));
    }

    #[test]
    fn notes_name_previewed_options() {
        let questions = parse(&input(json!([question(
            "Pick",
            "Pick",
            json!([
                json!({"label":"A","description":"a","preview":"preview A"}),
                json!({"label":"B","description":"b","preview":"preview B"})
            ]),
            true
        )])));
        let output = format_answers(
            &questions,
            &content(json!({"q1": ["B", "A"], "q1_notes": "because"})),
        )
        .unwrap();
        let line = answer_line(&output, 0);
        assert!(line.contains("user notes: \"because\""));
        assert!(line.contains("(previews for [\"B\",\"A\"])"));
    }

    #[test]
    fn max_length_boundary_counts_code_points_not_bytes() {
        let questions = parse(&input(json!([question(
            "Pick",
            "Pick",
            json!([option("A", "a"), option("B", "b")]),
            false
        )])));
        let schema = build_form(&questions).unwrap();
        let exactly = "界".repeat(MAX_TEXT);
        assert!(exactly.len() > MAX_TEXT);
        assert!(
            schema
                .check_content(&content(json!({"q1": "A", "q1_other": exactly})))
                .is_ok()
        );
        let over = "界".repeat(MAX_TEXT + 1);
        assert!(
            schema
                .check_content(&content(json!({"q1": "A", "q1_other": over})))
                .is_err()
        );
        assert!(
            schema
                .check_content(&content(
                    json!({"q1": "A", "q1_notes": "界".repeat(MAX_TEXT + 1)})
                ))
                .is_err()
        );
    }

    #[test]
    fn terminal_outcomes_say_what_happened() {
        let questions = parse(&input(json!([question(
            "Pick",
            "Pick",
            json!([option("A", "a"), option("B", "b")]),
            false
        )])));
        assert!(
            format_outcome(&questions, ElicitOutcome::Declined)
                .unwrap()
                .contains("declined")
        );
        assert!(
            format_outcome(&questions, ElicitOutcome::Cancelled)
                .unwrap()
                .contains("cancelled")
        );
        assert!(
            format_outcome(&questions, ElicitOutcome::Expired)
                .unwrap()
                .contains("expired")
        );
        assert!(format_outcome(&questions, ElicitOutcome::Unsupported).is_err());
    }

    #[tokio::test]
    async fn broker_round_trip_with_fake_connection() {
        const SESSION: &str = "ask-user-round-trip-session";
        const CONNECTION: &str = "ask-user-round-trip-connection";
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        crate::elicitation::global().attach(CONNECTION, SESSION, true, false, false, tx);

        let input = input(json!([
            question(
                "Which one?",
                "Pick",
                json!([option("A", "a"), option("B", "b")]),
                false
            ),
            question(
                "Which many?",
                "Many",
                json!([option("C", "c"), option("D", "d")]),
                true
            )
        ]));
        let handle = tokio::spawn(async move { AskUserTool.execute(input, ctx(SESSION)).await });
        let Some(ServerEvent::ElicitationRequest(request)) = rx.recv().await else {
            panic!("expected an elicitation request");
        };
        assert_eq!(request.message, "The agent has 2 questions.");
        assert_eq!(request.session_id, SESSION);
        assert_eq!(request.tool_call_id.as_deref(), Some("call"));
        assert_eq!(
            request.requested_schema.0["properties"]["q1"]["type"],
            "string"
        );
        assert_eq!(
            request.requested_schema.0["properties"]["q2"]["type"],
            "array"
        );

        crate::elicitation::global()
            .respond(
                CONNECTION,
                SESSION,
                &request.elicitation_id,
                ElicitationResponse::Accept {
                    content: Some(content(json!({"q1": "A", "q2": ["C"], "q1_notes": "ok"}))),
                },
            )
            .unwrap();
        let output = handle.await.unwrap().unwrap().output;
        assert_eq!(decode_answer(answer_line(&output, 0)), json!("A"));
        assert!(answer_line(&output, 0).contains("user notes: \"ok\""));
        assert_eq!(decode_answer(answer_line(&output, 1)), json!(["C"]));

        crate::elicitation::global().detach(CONNECTION);
    }

    #[tokio::test]
    async fn no_form_client_is_an_error_telling_the_model_to_ask_directly() {
        const SESSION: &str = "ask-user-headless-session";
        let input = input(json!([question(
            "Pick",
            "Pick",
            json!([option("A", "a"), option("B", "b")]),
            false
        )]));
        let error = AskUserTool
            .execute(input, ctx(SESSION))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("normal reply"), "{error}");
    }
}
