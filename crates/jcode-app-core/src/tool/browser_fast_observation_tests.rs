use super::*;

#[tokio::test]
async fn initial_and_predecision_observation_disconnects_are_retried_read_only() {
    let error = json!({"bridge_error":"Receiving end does not exist"});
    let browser = MockBrowser::new(vec![
        error.clone(),
        page("complete"),
        error,
        page("complete"),
    ]);
    let value = result(&browser, &MockTransport::new(&[("done", 0.99)]), &input()).await;
    assert_eq!(value["status"], "done", "{value}");
    assert!(value["action_trace"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn identical_url_and_target_on_new_document_requires_replan() {
    let mut before = page("same");
    before["document_id"] = json!("old-document");
    before["elements"] = json!([{"identity":1,"tag":"a","text":"Docs","selector":"#docs","href":"https://example.test/docs"}]);
    let mut after = before.clone();
    after["document_id"] = json!("new-document");
    let browser = MockBrowser::new(vec![before, after]);
    let transport = MockTransport::new(&[("a0", 0.99), ("done", 0.99)]);
    let value = result(&browser, &transport, &input()).await;
    assert_eq!(value["status"], "done", "{value}");
    assert!(value["action_trace"].as_array().unwrap().is_empty());
    assert_eq!(transport.observed.lock().unwrap().len(), 2);
}
