//! Serde round-trip smoke tests for the public data types.
//!
//! These exercise the JSON shape that `gen-evals` and `run` will read
//! and write, and they fail loudly if anyone changes a field's
//! `snake_case` name without updating consumers.

use jcode_jbench::types::{
    EvalCommit, FileDiff, FileDiffStatus, JudgingResult,
};

#[test]
fn eval_commit_round_trips_through_json() {
    let original = EvalCommit {
        id: "abc1234-add-readme".to_string(),
        sha: "abc1234deadbeef".to_string(),
        parent_sha: "0011223344556677".to_string(),
        spec: "Add a README describing the project.".to_string(),
        prompt: "Please add a README.md at the repo root.".to_string(),
        supplemental_files: vec!["Cargo.toml".to_string(), "src/lib.rs".to_string()],
        file_diffs: vec![FileDiff {
            path: "README.md".to_string(),
            status: FileDiffStatus::Added,
            old_path: None,
            diff: "+++ b/README.md\n@@ -0,0 +1 @@\n+hello\n".to_string(),
        }],
    };

    let json = serde_json::to_string(&original).expect("serialize EvalCommit");
    // Sanity-check the wire format is snake_case as documented.
    assert!(json.contains("\"parent_sha\""));
    assert!(json.contains("\"supplemental_files\""));
    assert!(json.contains("\"file_diffs\""));

    let decoded: EvalCommit = serde_json::from_str(&json).expect("deserialize EvalCommit");
    assert_eq!(decoded.id, original.id);
    assert_eq!(decoded.sha, original.sha);
    assert_eq!(decoded.parent_sha, original.parent_sha);
    assert_eq!(decoded.spec, original.spec);
    assert_eq!(decoded.prompt, original.prompt);
    assert_eq!(decoded.supplemental_files, original.supplemental_files);
    assert_eq!(decoded.file_diffs.len(), 1);
    assert_eq!(decoded.file_diffs[0].path, "README.md");
    assert!(matches!(
        decoded.file_diffs[0].status,
        FileDiffStatus::Added
    ));
}

#[test]
fn file_diff_round_trips_renamed_with_old_path() {
    let original = FileDiff {
        path: "src/new_name.rs".to_string(),
        status: FileDiffStatus::Renamed,
        old_path: Some("src/old_name.rs".to_string()),
        diff: "rename from src/old_name.rs\nrename to src/new_name.rs\n".to_string(),
    };

    let json = serde_json::to_string(&original).expect("serialize FileDiff");
    assert!(json.contains("\"status\":\"renamed\""));
    assert!(json.contains("\"old_path\":\"src/old_name.rs\""));

    let decoded: FileDiff = serde_json::from_str(&json).expect("deserialize FileDiff");
    assert_eq!(decoded.path, original.path);
    assert!(matches!(decoded.status, FileDiffStatus::Renamed));
    assert_eq!(decoded.old_path.as_deref(), Some("src/old_name.rs"));
    assert_eq!(decoded.diff, original.diff);

    // And a Modified entry should omit `old_path` from the JSON.
    let modified = FileDiff {
        path: "src/lib.rs".to_string(),
        status: FileDiffStatus::Modified,
        old_path: None,
        diff: "@@ -1 +1 @@\n-old\n+new\n".to_string(),
    };
    let modified_json = serde_json::to_string(&modified).expect("serialize Modified FileDiff");
    assert!(
        !modified_json.contains("old_path"),
        "old_path should be skipped when None, got: {modified_json}"
    );
}

#[test]
fn judging_result_round_trips_through_json() {
    let original = JudgingResult {
        analysis: "The agent addressed the prompt and produced clean code.".to_string(),
        strengths: vec![
            "Followed existing module structure.".to_string(),
            "Added a passing test.".to_string(),
        ],
        weaknesses: vec!["Missed an edge case in error handling.".to_string()],
        completion_score: 8.5,
        code_quality_score: 7.0,
        overall_score: 7.75,
    };

    let json = serde_json::to_string(&original).expect("serialize JudgingResult");
    assert!(json.contains("\"completion_score\""));
    assert!(json.contains("\"code_quality_score\""));
    assert!(json.contains("\"overall_score\""));

    let decoded: JudgingResult = serde_json::from_str(&json).expect("deserialize JudgingResult");
    assert_eq!(decoded.analysis, original.analysis);
    assert_eq!(decoded.strengths, original.strengths);
    assert_eq!(decoded.weaknesses, original.weaknesses);
    assert!((decoded.completion_score - original.completion_score).abs() < f64::EPSILON);
    assert!((decoded.code_quality_score - original.code_quality_score).abs() < f64::EPSILON);
    assert!((decoded.overall_score - original.overall_score).abs() < f64::EPSILON);
}
