use super::cascade::complete_via_named_openai_compatible;
use super::*;
use serde_json::{Map, Value, json};

#[test]
fn distribute_promotes_top_candidate_when_meeting_threshold() {
    let candidates = vec![
        ScoredSuggestion {
            text: "Run cargo test".to_string(),
            confidence: 0.42,
        },
        ScoredSuggestion {
            text: "Fix HTTP 402 fallback".to_string(),
            confidence: 0.81,
        },
        ScoredSuggestion {
            text: "Inspect git diff".to_string(),
            confidence: 0.63,
        },
    ];
    let result = distribute_suggestions("debugging", candidates, 0.55, 5);
    assert_eq!(result.branch, "debugging");
    assert_eq!(
        result.default_action.as_deref(),
        Some("Fix HTTP 402 fallback")
    );
    assert_eq!(
        result.secondary_suggestions,
        vec!["Inspect git diff".to_string(), "Run cargo test".to_string()]
    );
    assert_eq!(
        result.confidence_for_text("Fix HTTP 402 fallback"),
        Some(0.81)
    );
    assert_eq!(result.confidence_for_text("Inspect git diff"), Some(0.63));

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["defaultAction"], "Fix HTTP 402 fallback");
    assert_eq!(
        json["secondarySuggestions"],
        json!(["Inspect git diff", "Run cargo test"])
    );
    assert_eq!(json["branch"], "debugging");
}

#[test]
fn distribute_leaves_default_action_null_when_below_threshold() {
    let candidates = vec![
        ScoredSuggestion {
            text: "Option A".to_string(),
            confidence: 0.54,
        },
        ScoredSuggestion {
            text: "Option B".to_string(),
            confidence: 0.30,
        },
    ];
    let result = distribute_suggestions("exploration", candidates, 0.55, 5);
    assert_eq!(result.default_action, None);
    assert_eq!(
        result.secondary_suggestions,
        vec!["Option A".to_string(), "Option B".to_string()]
    );

    let json = serde_json::to_value(&result).unwrap();
    assert!(json["defaultAction"].is_null());
}

#[test]
fn distribute_respects_max_secondary_clamping_and_filters_nan() {
    let candidates = (0..8)
        .map(|i| ScoredSuggestion {
            text: format!("Step {i}"),
            confidence: 0.90 - (i as f32) * 0.05,
        })
        .chain([ScoredSuggestion {
            text: "Bad NaN".to_string(),
            confidence: f32::NAN,
        }])
        .collect();

    let result = distribute_suggestions("implementation", candidates, 0.55, 3);
    assert_eq!(result.default_action.as_deref(), Some("Step 0"));
    assert_eq!(
        result.secondary_suggestions,
        vec!["Step 1", "Step 2", "Step 3"]
    );
}

#[test]
fn cascade_outputs_exact_configured_suggestion_counts_for_1_3_and_20() {
    let transcript = "[user]: Проверь тестами, что при настройках 1, 3 и 20 каскад выдаёт нужное число подсказок.\n\n[assistant]: Готово! Что проверим дальше?\n1. Запустить тесты каскада\n2. Проверить TUI рендеринг";
    let ctx = SuggestedRepliesContext::from_transcript(transcript)
        .with_pending_todos(vec!["Проверить лимиты 1, 3 и 20".to_string()]);
    assert!(ctx.language_is_cyrillic);
    assert!(ctx.assistant_options.len() >= 2);

    for expected_count in [1usize, 3usize, 20usize] {
        let resp = score_locally_with_context(
            &ctx,
            None,
            &["Мой постоянный вопрос: проверь статус git".to_string()],
            None,
            0.55,
            expected_count,
        );
        assert!(
            resp.default_action.is_some(),
            "expected default_action for count={expected_count}"
        );
        assert_eq!(
            resp.secondary_suggestions.len(),
            expected_count,
            "cascade must emit exactly {expected_count} secondary suggestions"
        );
        // Every emitted row (default_action and all secondary_suggestions) must have a valid confidence score.
        let def_text = resp.default_action.as_deref().unwrap();
        assert!(resp.confidence_for_text(def_text).is_some());
        for sec in &resp.secondary_suggestions {
            assert!(
                resp.confidence_for_text(sec).is_some(),
                "missing confidence for secondary suggestion '{sec}'"
            );
        }
    }
}

#[test]
fn joint_choice_and_noul_questions_and_structured_state_respect_invariants() {
    let ctx = SuggestedRepliesContext::from_transcript(
        "[user]: Добавь две опции в настройки\n\n[assistant]: Выберите вариант:\n1. Показывать уверенность модели\n2. Добавить список постоянных вопросов",
    );
    let cands = contextual_candidates_for_branch("implementation", &ctx);
    assert!(cands.len() >= 20);

    let state = build_structured_systemone_state(Some("implementation"), &ctx, &cands[..4]);
    assert_eq!(state["workflow_branch"], "implementation");
    assert_eq!(state["language"], "ru");
    assert_eq!(state["candidates"]["cand_0"], cands[0]);

    // With >= 2 candidates and include_choice = true, best_candidate (choice) + cand_0..3 (noul) are present.
    let joint_q = build_micro_scoring_questions("implementation", &cands[..4], true).unwrap();
    assert_eq!(joint_q["best_candidate"]["type"], "choice");
    assert_eq!(joint_q["cand_0"]["type"], "noul");
    assert_eq!(joint_q.len(), 5);

    // With 1 candidate, best_candidate is omitted so jev.rs choice validation (2..=255) never fails.
    let single_q = build_micro_scoring_questions("implementation", &cands[..1], true).unwrap();
    assert!(!single_q.contains_key("best_candidate"));
    assert_eq!(single_q.len(), 1);

    // When finish_enabled = false, feature_completion is omitted from choice/noul questions
    // and normalized to goal_delivery in score_locally_with_context_and_options.
    let choice_no_finish = build_macro_branch_choice_question_with_options(false);
    assert!(
        choice_no_finish["macro_branch"]["criteria"]
            .get("feature_completion")
            .is_none()
    );
    let noul_no_finish = build_macro_branch_noul_questions_with_options(false);
    assert!(!noul_no_finish.contains_key("branch_feature_completion"));
    let finish_ctx = SuggestedRepliesContext::from_transcript(
        "[user]: Довести фичу до конца\n\n[assistant]: All todos completed. Ready for feature completion.",
    );
    let scored_no_finish =
        score_locally_with_context_and_options(&finish_ctx, None, &[], None, 0.72, 5, false);
    assert_eq!(scored_no_finish.branch, "goal_delivery");
    let def_action = scored_no_finish.default_action.as_deref().unwrap();
    assert_ne!(scored_no_finish.category_for_text(def_action), "finish");
}

#[test]
fn parse_open_gf_extracts_json_from_fenced_response() {
    let raw = "```json\n{\"branch\": \"Verification\", \"candidates\": [\"cargo test\", \"git status\", \"Показать обрезанные данные...\", \"Incomplete reply...\"]}\n```";
    let (branch, cands) = parse_open_gf_response(raw).unwrap();
    assert_eq!(branch, "verification");
    assert_eq!(cands, vec!["cargo test", "git status"]);

    // Verify extract_assistant_options ignores descriptive **Key**: value status bullets
    // and truncated lines while keeping genuine actionable choices.
    let status_report = "- **Config**: enabled=true, jev_provider=\"auto\"\n- **Security (GO)**: секреты санитизируются\n1. **Architecture (GO)**: сквозная передача флага\n- Обрезанные данные в выводе...\n- Запустить финальные тесты";
    let extracted = extract_assistant_options(status_report);
    assert_eq!(extracted, vec!["Запустить финальные тесты"]);

    // Verify truncate_preserving_tail preserves the closing tail without "..." markers.
    let long_text = format!(
        "{}\nФинальная строка ответа без обрезки.",
        "Строка контекста для проверки границ слов.\n".repeat(80)
    );
    let truncated = truncate_preserving_tail(&long_text, 600);
    assert!(truncated.ends_with("Финальная строка ответа без обрезки."));
    assert!(!truncated.contains("..."));
}

#[test]
fn calibrate_micro_score_maps_positive_laya_noul_to_threshold_scale_and_keeps_low_scores_unboosted()
{
    let top = calibrate_micro_score(0.6089, 0, true);
    assert!(
        top >= 0.75,
        "expected calibrated top score >= 0.75, got {top}"
    );
    let top_with_choice = calibrate_micro_score_with_choice(0.8339, Some(0.35), true, 0, true);
    assert!(top_with_choice > top);
    let second = calibrate_micro_score(0.5864, 1, true);
    assert!(top > second);
    assert_eq!(calibrate_micro_score(0.42, 0, true), 0.42);
}

#[tokio::test]
async fn complete_via_named_openai_compatible_accepts_literal_sk_in_api_key_env() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req.to_ascii_lowercase()
                    .contains("authorization: bearer sk-test-inline-secret"),
                "missing bearer header in {req}"
            );
            let body = r#"{"choices":[{"message":{"content":"{\"branch\":\"debugging\",\"candidates\":[\"Проверить отладку через /suggestions debug\",\"Запустить тесты\"]}"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let profile = crate::config::NamedProviderConfig {
        base_url: format!("http://{addr}/v1"),
        api_key_env: Some("sk-test-inline-secret".to_string()),
        default_model: Some("gpt-5.6-luna".to_string()),
        ..Default::default()
    };
    let (raw, label) = complete_via_named_openai_compatible("clipapi", &profile, "sys", "usr")
        .await
        .unwrap();
    assert_eq!(label, "profile:clipapi:gpt-5.6-luna");
    let (branch, cands) = parse_open_gf_response(&raw).unwrap();
    assert_eq!(branch, "debugging");
    assert_eq!(cands[0], "Проверить отладку через /suggestions debug");
}

#[test]
fn suggested_replies_env_overrides_apply_count_confidence_and_custom_prompts() {
    let _lock = crate::storage::lock_test_env();
    let keys = [
        "JCODE_SUGGESTED_REPLIES_MAX_SECONDARY",
        "JCODE_MAX_SECONDARY_SUGGESTIONS",
        "JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE",
        "JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS",
    ];
    let saved: Vec<(&str, Option<std::ffi::OsString>)> =
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect();

    crate::env::set_var("JCODE_SUGGESTED_REPLIES_MAX_SECONDARY", "15");
    crate::env::set_var("JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE", "false");
    crate::env::set_var(
        "JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS",
        "Вопрос 1 | Вопрос 2 ; Вопрос 3",
    );

    let mut cfg = crate::config::Config::default();
    cfg.apply_env_overrides();
    assert_eq!(cfg.suggested_replies.max_secondary_suggestions, 15);
    assert!(!cfg.suggested_replies.show_confidence);
    assert_eq!(
        cfg.suggested_replies.sanitized_custom_prompts(),
        vec!["Вопрос 1", "Вопрос 2", "Вопрос 3"]
    );

    // JSON array format + clamping out-of-range counts (99 -> 20, -4 -> 1).
    crate::env::set_var("JCODE_SUGGESTED_REPLIES_MAX_SECONDARY", "99");
    crate::env::set_var("JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE", "1");
    crate::env::set_var(
        "JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS",
        r#"["JSON вопрос A", "JSON вопрос B", ""]"#,
    );
    cfg.apply_env_overrides();
    assert_eq!(cfg.suggested_replies.max_secondary_suggestions, 20);
    assert!(cfg.suggested_replies.show_confidence);
    assert_eq!(
        cfg.suggested_replies.sanitized_custom_prompts(),
        vec!["JSON вопрос A", "JSON вопрос B"]
    );

    crate::env::set_var("JCODE_SUGGESTED_REPLIES_MAX_SECONDARY", "-4");
    cfg.apply_env_overrides();
    assert_eq!(cfg.suggested_replies.max_secondary_suggestions, 1);

    for (k, val) in saved {
        match val {
            Some(v) => crate::env::set_var(k, v),
            None => crate::env::remove_var(k),
        }
    }
}

#[test]
fn extract_assistant_options_handles_code_fences_bullets_and_slash_commands() {
    let msg = r#"Вот пример кода:
```bash
1. Fake numbered item inside code fence
- Fake bullet inside code fence
`/fake-inside-fence`
```
Что делаем дальше?
- Проверить конфигурацию в `crates/jcode-base/src/config.rs`
• Запустить модульные тесты — убедиться, что всё зелёное
1) Сделать коммит изменений
Также можно вызвать `/opsx:apply` или `/review 6`, но не путь `/usr/local/bin/cargo` и не `https://example.com/test`.
"#;
    let opts = extract_assistant_options(msg);
    assert!(
        !opts.iter().any(|o| o.contains("Fake")),
        "must ignore lines inside fenced code blocks: {opts:?}"
    );
    assert!(
        !opts.iter().any(|o| o.contains("/usr/local")),
        "must ignore file paths: {opts:?}"
    );
    assert!(
        opts.iter().any(|o| o.contains("Проверить конфигурацию")),
        "must extract '-' bullet option: {opts:?}"
    );
    assert!(
        opts.iter().any(|o| o.contains("Запустить модульные тесты")),
        "must extract '•' bullet option and strip em-dash explanation: {opts:?}"
    );
    assert!(
        opts.iter().any(|o| o == "Сделать коммит изменений"),
        "must extract '1)' numbered option: {opts:?}"
    );
    assert!(
        opts.iter().any(|o| o == "/opsx:apply") && opts.iter().any(|o| o == "/review 6"),
        "must extract backticked slash commands: {opts:?}"
    );
}

#[tokio::test]
async fn evaluate_cascade_with_context_executes_two_stage_laya_systemone_and_falls_back_on_http_402()
 {
    use std::io::{Read, Write};
    let _lock = crate::storage::lock_test_env();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = std::thread::spawn(move || {
        // Request 1: Iteration 1 macro_branch choice
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 16384];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req.contains("\"macro_branch\""),
                "expected macro_branch question in iter1: {req}"
            );
            let body = r#"{"id":"dec_1","model":"laya-latest","answers":{"macro_branch":{"type":"choice","choice":"verification","probabilities":{"verification":0.82,"implementation":0.18}}}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }

        // Request 2: Iteration 2 joint best_candidate (choice) + cand_0..cand_N (noul)
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 32768];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let json_start = req.find('{').expect("request JSON body");
            let parsed: Value =
                serde_json::from_str(&req[json_start..]).expect("valid request JSON");
            assert_eq!(parsed["state"]["workflow_branch"], "verification");
            assert_eq!(parsed["questions"]["best_candidate"]["type"], "choice");

            let questions = parsed["questions"].as_object().unwrap();
            let mut answers = Map::new();
            let mut probs = Map::new();
            for key in questions.keys() {
                if key == "best_candidate" {
                    continue;
                }
                let is_winner = key == "cand_1";
                probs.insert(key.clone(), json!(if is_winner { 0.65 } else { 0.05 }));
                answers.insert(
                    key.clone(),
                    json!({ "type": "noul", "noul": if is_winner { 0.84 } else { 0.52 } }),
                );
            }
            answers.insert(
                "best_candidate".to_string(),
                json!({
                    "type": "choice",
                    "choice": "cand_1",
                    "probabilities": probs,
                }),
            );
            let body = serde_json::to_string(&json!({
                "id": "dec_2",
                "model": "laya-latest",
                "answers": answers,
            }))
            .unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }

        // Request 3: Simulate HTTP 402 Payment Required / error to verify local fallback
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 16384];
            let _ = stream.read(&mut buf);
            let body = r#"{"error":{"message":"Insufficient credits"}}"#;
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let saved_laya_url = std::env::var_os("JCODE_LAYA_BASE_URL");
    crate::env::set_var("JCODE_LAYA_BASE_URL", format!("http://{addr}/v1/systemone"));

    let ctx = SuggestedRepliesContext::from_transcript(
        "[user]: Проверь работу каскада через Laya\n\n[assistant]: Выберите шаг:\n1. Первый шаг\n2. Второй шаг (победитель)",
    )
    .with_pending_todos(vec!["Запустить финальные тесты".to_string()]);

    let cfg = crate::config::SuggestedRepliesConfig {
        enabled: true,
        jev_provider: "laya".to_string(),
        use_open_gf: false,
        max_secondary_suggestions: 3,
        confidence_threshold: 0.55,
        show_confidence: true,
        custom_prompts: vec!["Кастомный вопрос".to_string()],
        enable_full_auto_explorer: false,
        six_lens_auto_review: false,
        enable_finish_category: true,
    };

    // 1. Happy path: 2-stage SystemOne evaluation (choice + joint choice+noul)
    let resp = evaluate_cascade_with_context(&ctx, &cfg, None).await;
    assert_eq!(resp.branch, "verification");
    assert_eq!(
        resp.default_action.as_deref(),
        Some("Второй шаг (победитель)"),
        "cand_1 chosen by best_candidate + noul must be promoted to default_action"
    );
    assert_eq!(resp.secondary_suggestions.len(), 3);
    let debug = resp.debug.as_ref().expect("debug info present");
    assert_eq!(debug.source, "fixed+jev");
    assert!(debug.jev_status.contains("mode=choice+noul"));

    // 2. HTTP 402 fallback path: gracefully degrades to local scorer
    let fallback_resp = evaluate_cascade_with_context(&ctx, &cfg, None).await;
    assert!(fallback_resp.default_action.is_some());
    assert_eq!(fallback_resp.secondary_suggestions.len(), 3);
    assert_eq!(fallback_resp.debug.as_ref().unwrap().source, "fixed+local");

    // 3. Six-lens findings short-circuit path
    let six_lens_resp =
        evaluate_cascade_with_context(&ctx, &cfg, Some("Устранить гонку в кэше")).await;
    assert_eq!(six_lens_resp.branch, "review");
    assert_eq!(six_lens_resp.debug.as_ref().unwrap().source, "six_lens");
    assert!(
        six_lens_resp
            .default_action
            .as_deref()
            .unwrap()
            .contains("Устранить гонку в кэше")
    );

    match saved_laya_url {
        Some(v) => crate::env::set_var("JCODE_LAYA_BASE_URL", v),
        None => crate::env::remove_var("JCODE_LAYA_BASE_URL"),
    }
    let _ = server_thread.join();
}

#[test]
fn goal_classification_labels_and_badges_cover_all_categories_bilingually() {
    let custom = vec!["Кастомный вопрос для цели".to_string()];
    assert_eq!(
        classify_suggestion_for_goal("Кастомный вопрос для цели", "implementation", &custom),
        "custom"
    );
    assert_eq!(format_goal_category_badge("custom", true), "[свой]");
    assert_eq!(format_goal_category_badge("custom", false), "[custom]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Закоммитить проверенные изменения и завершить цель",
            "goal_delivery",
            &[]
        ),
        "goal"
    );
    assert_eq!(format_goal_category_badge("goal", true), "[цель]");
    assert_eq!(format_goal_category_badge("goal", false), "[goal]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Выполнить следующую задачу: обновить конфиг",
            "planning",
            &[]
        ),
        "plan"
    );
    assert_eq!(format_goal_category_badge("plan", true), "[план]");
    assert_eq!(format_goal_category_badge("plan", false), "[plan]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Запустить целевые тесты и убедиться, что все проверки проходят",
            "verification",
            &[]
        ),
        "verify"
    );
    assert_eq!(format_goal_category_badge("verify", true), "[тест]");
    assert_eq!(format_goal_category_badge("verify", false), "[verify]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Проанализировать вывод падающего теста и найти корневую причину ошибки",
            "debugging",
            &[]
        ),
        "debug"
    );
    assert_eq!(format_goal_category_badge("debug", true), "[баг]");
    assert_eq!(format_goal_category_badge("debug", false), "[debug]");

    assert_eq!(
        classify_suggestion_for_goal("Запустить /review 6 для аудита", "review", &[]),
        "review"
    );
    assert_eq!(format_goal_category_badge("review", true), "[ревью]");
    assert_eq!(format_goal_category_badge("review", false), "[review]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Удалить неиспользуемый код и лишние импорты",
            "refactoring",
            &[]
        ),
        "refactor"
    );
    assert_eq!(format_goal_category_badge("refactor", true), "[рефактор]");
    assert_eq!(format_goal_category_badge("refactor", false), "[refactor]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Пересобрать и перезагрузить бинарник через selfdev build-reload",
            "implementation",
            &[]
        ),
        "impl"
    );
    assert_eq!(format_goal_category_badge("impl", true), "[код]");
    assert_eq!(format_goal_category_badge("impl", false), "[impl]");

    assert_eq!(
        classify_suggestion_for_goal(
            "Довести фичу до конца и выполнить финальное завершение",
            "implementation",
            &[]
        ),
        "finish"
    );
    assert_eq!(
        classify_suggestion_for_goal(
            "Complete the feature and verify final completion",
            "implementation",
            &[]
        ),
        "finish"
    );
    assert_eq!(format_goal_category_badge("finish", true), "[финал]");
    assert_eq!(format_goal_category_badge("finish", false), "[finish]");
    assert_eq!(
        classify_suggestion_for_goal_with_options(
            "Довести фичу до конца и выполнить финальное завершение",
            "implementation",
            &[],
            false
        ),
        "goal"
    );
    assert_eq!(
        classify_suggestion_for_goal_with_options(
            "Complete the feature and verify final completion",
            "feature_completion",
            &[],
            false
        ),
        "goal"
    );

    assert!(advances_toward_feature_completion(
        "Довести фичу до конца и выполнить финальное завершение",
        "implementation"
    ));
    assert!(advances_toward_feature_completion(
        "Запустить целевые тесты и убедиться, что все проверки проходят",
        "verification"
    ));
    assert!(!advances_toward_feature_completion(
        "Изучить границы модулей и публичные интерфейсы целевых крейтов",
        "architecture"
    ));
}

#[test]
fn extract_assistant_options_tracks_code_fences_across_prefix_and_extracts_review_default_action() {
    let mut lines = vec![
        "Вот длинный пример в блоке кода:".to_string(),
        "```rust".to_string(),
    ];
    for i in 1..=30 {
        lines.push(format!("{i}. Fake code option inside long fence {i}"));
    }
    lines.push("`/fake-command`".to_string());
    lines.push("```".to_string());
    lines.push("Выберите реальное следующее действие:".to_string());
    lines.push("1. Запустить реальные интеграционные тесты".to_string());
    lines.push("2. Проверить сборку и отправить PR".to_string());
    let msg = lines.join("\n");

    let opts = extract_assistant_options(&msg);
    assert!(
        !opts.iter().any(|o| o.contains("Fake")),
        "must not extract numbered lines inside a code block that started >28 lines ago: {opts:?}"
    );
    assert_eq!(
        opts,
        vec![
            "Запустить реальные интеграционные тесты".to_string(),
            "Проверить сборку и отправить PR".to_string()
        ]
    );

    assert_eq!(
        extract_six_lens_review_findings(
            "Overall Verdict: CONDITIONAL GO\n- DEFAULT_ACTION: `Fix session boundary check in poll_suggested_replies_tick`"
        ),
        Some("Fix session boundary check in poll_suggested_replies_tick".to_string())
    );
    assert_eq!(
        extract_six_lens_review_findings(
            "You are the 6-Lens Subagent Self-Review coordinator... prefixed with DEFAULT_ACTION: describing the exact fix"
        ),
        None
    );
}

#[tokio::test]
async fn named_profile_honors_custom_auth_header_extra_body_and_laya_fixed_selector() {
    use std::io::{Read, Write};
    let _lock = crate::storage::lock_test_env();

    let saved_prov = std::env::var_os("JCODE_SUGGESTIONS_JEV_PROVIDER");
    crate::env::set_var("JCODE_SUGGESTIONS_JEV_PROVIDER", "auto");
    let fixed_laya = crate::jev::JevClient::for_suggestions_fixed_selector("laya")
        .expect("fixed laya selector must succeed even when JCODE_SUGGESTIONS_JEV_PROVIDER=auto");
    assert_eq!(fixed_laya.provider_name(), "laya");
    match saved_prov {
        Some(v) => crate::env::set_var("JCODE_SUGGESTIONS_JEV_PROVIDER", v),
        None => crate::env::remove_var("JCODE_SUGGESTIONS_JEV_PROVIDER"),
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req.to_ascii_lowercase()
                    .contains("x-custom-key: sk-custom-header-token"),
                "expected custom auth header in request: {req}"
            );
            assert!(
                req.contains("\"custom_flag\":true"),
                "expected extra_body field in request: {req}"
            );
            let body = r#"{"choices":[{"message":{"content":"{\"branch\":\"verification\",\"candidates\":[\"Run cargo test\"]}"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let profile = crate::config::NamedProviderConfig {
        base_url: format!("http://{addr}/v1"),
        auth: crate::config::NamedProviderAuth::Header,
        auth_header: Some("X-Custom-Key".to_string()),
        api_key: Some("sk-custom-header-token".to_string()),
        extra_body: Some(json!({ "custom_flag": true })),
        ..Default::default()
    };
    let (raw, _) = complete_via_named_openai_compatible("custom-prof", &profile, "sys", "usr")
        .await
        .unwrap();
    let (branch, cands) = parse_open_gf_response(&raw).unwrap();
    assert_eq!(branch, "verification");
    assert_eq!(cands, vec!["Run cargo test"]);
}
