#[test]
fn cloud_record_ids_are_local_and_only_proven_lineage_keeps_them() {
    for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
        for scenario in ["identity", "same-grant", "baseline", "unproven"] {
            let home = tempfile::tempdir().unwrap();
            let mut envelope: Value =
                serde_json::from_slice(&cloud_fixture(provider, "selected")).unwrap();
            if scenario != "identity" {
                for key in ["account_id", "email", "id_token"] {
                    envelope["credential"].as_object_mut().unwrap().remove(key);
                }
            }
            import_cloud_at(
                home.path(),
                provider,
                "selected",
                &serde_json::to_vec(&envelope).unwrap(),
            )
            .unwrap();
            let path = home.path().join(provider.store_name());
            let (accounts, _, access, refresh) = cloud_keys(provider);
            let mut store: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let local_id = uuid::Uuid::new_v4().to_string();
            store[accounts][0]["record_id"] = json!(local_id);
            fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
            let original =
                export_cloud_account_at(home.path(), provider, Some("selected")).unwrap();
            let exported: Value = serde_json::from_slice(original.as_bytes()).unwrap();
            assert!(
                exported["credential"].get("record_id").is_none(),
                "local row identity must not be exported"
            );
            envelope["credential"]["record_id"] = json!(local_id);
            if scenario != "same-grant" {
                envelope["credential"][access] = json!("replacement-access");
                envelope["credential"][refresh] = json!("replacement-refresh");
            }
            let returned = serde_json::to_vec(&envelope).unwrap();
            if scenario == "baseline" {
                import_cloud_return_at(home.path(), provider, "selected", &returned, &original)
                    .unwrap();
            } else {
                import_cloud_handoff_at(home.path(), provider, "selected", &returned).unwrap();
            }
            let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if scenario == "unproven" {
                assert!(stored[accounts][0].get("record_id").is_none());
            } else {
                assert_eq!(
                    stored[accounts][0]["record_id"].as_str(),
                    Some(local_id.as_str())
                );
            }
        }
    }
}

#[test]
fn new_cloud_accounts_do_not_accept_a_foreign_record_id() {
    for provider in [TransferProvider::OpenAi, TransferProvider::Claude] {
        let home = tempfile::tempdir().unwrap();
        let bytes = revised(
            &cloud_fixture(provider, "selected"),
            "record_id",
            json!(uuid::Uuid::new_v4().to_string()),
        );
        import_cloud_at(home.path(), provider, "selected", &bytes).unwrap();
        let stored: Value =
            serde_json::from_slice(&fs::read(home.path().join(provider.store_name())).unwrap())
                .unwrap();
        assert!(stored[cloud_keys(provider).0][0].get("record_id").is_none());
    }
}
