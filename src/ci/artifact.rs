use std::path::PathBuf;

/// Artifact store for pipeline outputs
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    pub artifact_paths: Vec<PathBuf>,
    pub collected_artifacts: Vec<Artifact>,
}

/// Single build artifact
#[derive(Debug, Clone)]
pub struct Artifact {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub artifact_type: ArtifactType,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Type of artifact
#[derive(Debug, Clone)]
pub enum ArtifactType {
    Binary,
    Library,
    Report,
    Log,
    Config,
    Image,
    Document,
    Other,
}

impl ArtifactStore {
    pub fn new(artifact_paths: Vec<PathBuf>) -> Self {
        ArtifactStore {
            artifact_paths,
            collected_artifacts: vec![],
        }
    }

    pub fn add_artifact(&mut self, name: &str, path: PathBuf, size_bytes: u64, artifact_type: ArtifactType) {
        self.collected_artifacts.push(Artifact {
            name: name.to_string(),
            path,
            size_bytes,
            artifact_type,
            created_at: chrono::Utc::now(),
        });
    }

    pub fn list_artifacts(&self) -> &[Artifact] {
        &self.collected_artifacts
    }

    pub fn total_size(&self) -> u64 {
        self.collected_artifacts.iter().map(|a| a.size_bytes).sum()
    }

    pub fn clear(&mut self) {
        self.collected_artifacts.clear();
    }
}