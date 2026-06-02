#[test]
fn test_protocol_request_roundtrip_randomized_samples() -> Result<()> {
    use rand::{Rng, SeedableRng};

    fn sample_ascii(rng: &mut rand::rngs::StdRng, max_len: usize) -> String {
        let len = rng.random_range(0..=max_len);
        (0..len)
            .map(|_| char::from(rng.random_range(b'a'..=b'z')))
            .collect()
    }

    let mut rng = rand::rngs::StdRng::seed_from_u64(0xC0DEC0DE);

    for id in 0..32u64 {
        let content = sample_ascii(&mut rng, 24);
        let images = if rng.random_bool(0.5) {
            vec![("image/png".to_string(), sample_ascii(&mut rng, 12))]
        } else {
            Vec::new()
        };
        let system_reminder = if rng.random_bool(0.5) {
            Some(sample_ascii(&mut rng, 20))
        } else {
            None
        };
        let req = Request::Message {
            id,
            content: content.clone(),
            images: images.clone(),
            system_reminder: system_reminder.clone(),
        };
        let decoded = parse_request_json(&serde_json::to_string(&req)?)?;
        let Request::Message {
            id: decoded_id,
            content: decoded_content,
            images: decoded_images,
            system_reminder: decoded_system_reminder,
        } = decoded
        else {
            return Err(anyhow!("expected randomized Message"));
        };
        assert_eq!(decoded_id, id);
        assert_eq!(decoded_content, content);
        assert_eq!(decoded_images, images);
        assert_eq!(decoded_system_reminder, system_reminder);
    }

    for id in 100..132u64 {
        let working_dir = rng
            .random_bool(0.5)
            .then(|| format!("/tmp/{}", sample_ascii(&mut rng, 12)));
        let selfdev = rng.random_bool(0.5).then(|| rng.random_bool(0.5));
        let target_session_id = rng.random_bool(0.5).then(|| format!("sess_{}", id));
        let client_instance_id = rng.random_bool(0.5).then(|| format!("client-{}", id));
        let client_has_local_history = rng.random_bool(0.5);
        let allow_session_takeover = rng.random_bool(0.5);
        let req = Request::Subscribe {
            id,
            working_dir: working_dir.clone(),
            selfdev,
            target_session_id: target_session_id.clone(),
            client_instance_id: client_instance_id.clone(),
            client_has_local_history,
            allow_session_takeover,
        };
        let decoded = parse_request_json(&serde_json::to_string(&req)?)?;
        let Request::Subscribe {
            id: decoded_id,
            working_dir: decoded_working_dir,
            selfdev: decoded_selfdev,
            target_session_id: decoded_target_session_id,
            client_instance_id: decoded_client_instance_id,
            client_has_local_history: decoded_client_has_local_history,
            allow_session_takeover: decoded_allow_session_takeover,
        } = decoded
        else {
            return Err(anyhow!("expected randomized Subscribe"));
        };
        assert_eq!(decoded_id, id);
        assert_eq!(decoded_working_dir, working_dir);
        assert_eq!(decoded_selfdev, selfdev);
        assert_eq!(decoded_target_session_id, target_session_id);
        assert_eq!(decoded_client_instance_id, client_instance_id);
        assert_eq!(decoded_client_has_local_history, client_has_local_history);
        assert_eq!(decoded_allow_session_takeover, allow_session_takeover);
    }

    Ok(())
}

#[test]
fn test_resume_session_roundtrip_preserves_client_sync_flags() -> Result<()> {
    let req = Request::ResumeSession {
        id: 90,
        session_id: "sess_resume".to_string(),
        client_instance_id: Some("client-456".to_string()),
        client_has_local_history: true,
        allow_session_takeover: true,
    };
    let json = serde_json::to_string(&req)?;
    assert!(json.contains("\"type\":\"resume_session\""));
    let decoded = parse_request_json(&json)?;
    let Request::ResumeSession {
        id,
        session_id,
        client_instance_id,
        client_has_local_history,
        allow_session_takeover,
    } = decoded
    else {
        return Err(anyhow!("expected ResumeSession"));
    };
    assert_eq!(id, 90);
    assert_eq!(session_id, "sess_resume");
    assert_eq!(client_instance_id.as_deref(), Some("client-456"));
    assert!(client_has_local_history);
    assert!(allow_session_takeover);
    Ok(())
}
