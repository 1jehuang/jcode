use super::*;

#[test]
fn background_timeout_honors_values_above_the_foreground_cap() {
    // Field report: run_in_background with timeout=1800000 (30 min) was
    // recorded as "timed out after 600000ms" and killed at 10 minutes.
    assert_eq!(background_timeout_ms(Some(1_800_000)), Some(1_800_000));
    assert_eq!(background_timeout_ms(Some(100)), Some(100));
    assert_eq!(background_timeout_ms(None), None);
}

#[tokio::test]
async fn background_delivery_preserves_opt_outs_and_explicit_wake() {
    let _env = crate::storage::lock_test_env();
    for (flags, expected) in [
        (json!({"notify": false}), (false, false)),
        (json!({"wake": false}), (true, false)),
        (json!({"notify": false, "wake": true}), (true, true)),
    ] {
        let mut input = json!({"command": "printf delivery", "run_in_background": true});
        input
            .as_object_mut()
            .unwrap()
            .extend(flags.as_object().unwrap().clone());
        let mut ctx = make_ctx(None);
        ctx.session_id = "bash-delivery-opt-outs".into();
        let result = BashTool::new().execute(input, ctx).await.unwrap();
        assert!(
            result
                .output
                .contains(crate::tool::background_delivery_notice(expected.1))
        );
        let task_id = result.metadata.as_ref().unwrap()["task_id"]
            .as_str()
            .unwrap();
        let finished = crate::background::global()
            .wait(task_id, Duration::from_secs(5), false)
            .await
            .expect("background completion");
        assert_eq!((finished.task.notify, finished.task.wake), expected);
    }
}
