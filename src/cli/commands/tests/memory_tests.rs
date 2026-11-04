use super::*;

#[tokio::test]
async fn memory_cli_project_import_uses_explicit_directory_and_persists() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME"]);
    let temp = tempfile::tempdir().expect("temp dir");
    crate::env::set_var("JCODE_HOME", temp.path().join("home"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");

    let entry = crate::memory::MemoryEntry::new(
        crate::memory::MemoryCategory::Fact,
        "persisted project memory".to_string(),
    );
    let input = temp.path().join("memories.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&vec![entry.clone()]).expect("serialize memory"),
    )
    .expect("write import");

    run_memory_command_for_dir(
        MemorySubcommand::Import {
            input: input.display().to_string(),
            scope: "project".to_string(),
            overwrite: false,
        },
        Some(project.clone()),
    )
    .await
    .expect("import project memory");

    let reloaded = crate::memory::MemoryManager::new()
        .with_project_dir(project)
        .load_project_graph()
        .expect("reload project graph");
    assert_eq!(
        reloaded
            .get_memory(&entry.id)
            .map(|memory| memory.content.as_str()),
        Some("persisted project memory")
    );
}

#[tokio::test]
async fn memory_cli_project_import_fails_without_durable_project_store() {
    let temp = tempfile::tempdir().expect("temp dir");
    let input = temp.path().join("memories.json");
    std::fs::write(&input, "[]").expect("write import");

    let error = run_memory_command_for_dir(
        MemorySubcommand::Import {
            input: input.display().to_string(),
            scope: "project".to_string(),
            overwrite: false,
        },
        None,
    )
    .await
    .expect_err("project import must require a durable store");

    assert!(error.to_string().contains("without a project directory"));
}

#[tokio::test]
async fn memory_cli_semantic_requires_jev_but_keyword_search_remains_local() {
    let _guard = crate::storage::lock_test_env();
    let keys = [
        "JCODE_HOME",
        "JCODE_API_KEY",
        "OPENROUTER_API_KEY",
        "TYPESAFE_API_KEY",
        "AIMLAPI_API_KEY",
        "JCODE_MEMORY_JEV_PROVIDER",
    ];
    let _saved = SavedEnv::capture(&keys);
    let temp = tempfile::tempdir().expect("temp dir");
    crate::env::set_var("JCODE_HOME", temp.path().join("home"));
    for key in &keys[1..] {
        crate::env::remove_var(key);
    }
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let manager = crate::memory::MemoryManager::new().with_project_dir(&project);
    manager
        .remember_project(crate::memory::MemoryEntry::new(
            crate::memory::MemoryCategory::Fact,
            "cli-jev-probe without embedding",
        ))
        .unwrap();
    assert!(
        manager
            .list_all()
            .unwrap()
            .iter()
            .all(|entry| entry.embedding.is_none())
    );

    let command = |semantic| MemorySubcommand::Search {
        query: "cli-jev-probe".into(),
        semantic,
    };
    run_memory_command_for_dir(command(false), Some(project.clone()))
        .await
        .expect("local keyword search must remain available without credentials");
    let error = run_memory_command_for_dir(command(true), Some(project))
        .await
        .expect_err("--semantic must report missing Jev access, not silently use embeddings");
    assert!(error.to_string().contains("Jev memory search failed"));
    assert!(!format!("{error:#}").contains("cli-jev-probe"));

    // Scope still comes from the explicit directory, never the process cwd.
    run_memory_command_for_dir(command(true), Some(temp.path().join("empty-project")))
        .await
        .expect("an empty project must not leak another project's candidates");
}
