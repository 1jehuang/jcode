#[test]
fn borrowed_remote_status_fields_match_legacy_and_retain_sources() {
    for value in [None, Some(String::new()), Some("fixture 🦀".to_string())] {
        let mut provider_name = value.clone();
        let mut connection_type = value.clone();
        let mut remote_transport = value.clone();
        let mut server_version = value;
        let legacy = serde_json::json!({
            "provider_name": provider_name.clone(),
            "connection_type": connection_type.clone(),
            "remote_transport": remote_transport.clone(),
            "server_version": server_version.clone(),
        });
        let evidence = serde_json::json!({
            "provider_name": provider_name,
            "connection_type": connection_type,
            "remote_transport": remote_transport,
            "server_version": server_version,
        });
        assert_eq!(evidence, legacy);
        assert_eq!(evidence.to_string(), legacy.to_string());
        assert_eq!(provider_name.as_deref(), legacy["provider_name"].as_str());
        assert_eq!(
            connection_type.as_deref(),
            legacy["connection_type"].as_str()
        );
        assert_eq!(
            remote_transport.as_deref(),
            legacy["remote_transport"].as_str()
        );
        assert_eq!(server_version.as_deref(), legacy["server_version"].as_str());
        for source in [
            &mut provider_name,
            &mut connection_type,
            &mut remote_transport,
            &mut server_version,
        ] {
            *source = Some("changed after serialization".to_string());
        }
        assert_eq!(evidence, legacy);
    }
}
