use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub build_date: chrono::DateTime<chrono::Utc>,
    pub commit_hash: Option<String>,
    pub changelog: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackPoint {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub description: String,
    pub version: String,
    pub backup_path: PathBuf,
}

pub struct VersionManager {
    versions_dir: PathBuf,
    current_version: VersionInfo,
    rollback_points: Vec<RollbackPoint>,
}

impl VersionManager {
    pub fn new(versions_dir: PathBuf) -> Self {
        fs::create_dir_all(&versions_dir).ok();
        let current = Self::load_current_version(&versions_dir);
        let rollbacks = Self::load_rollback_points(&versions_dir);

        VersionManager {
            versions_dir,
            current_version: current,
            rollback_points: rollbacks,
        }
    }

    fn load_current_version(dir: &PathBuf) -> VersionInfo {
        let version_file = dir.join("current.json");
        if version_file.exists() {
            if let Ok(content) = fs::read_to_string(&version_file) {
                if let Ok(info) = serde_json::from_str::<VersionInfo>(&content) {
                    return info;
                }
            }
        }

        VersionInfo {
            version: "0.1.0".to_string(),
            build_date: chrono::Utc::now(),
            commit_hash: None,
            changelog: vec!["Initial version".to_string()],
        }
    }

    fn load_rollback_points(dir: &PathBuf) -> Vec<RollbackPoint> {
        let mut points = vec![];
        let rollback_file = dir.join("rollbacks.json");
        if rollback_file.exists() {
            if let Ok(content) = fs::read_to_string(&rollback_file) {
                if let Ok(data) = serde_json::from_str::<Vec<RollbackPoint>>(&content) {
                    points = data;
                }
            }
        }
        points
    }

    fn save_state(&self) -> Result<(), String> {
        let version_file = self.versions_dir.join("current.json");
        let content = serde_json::to_string_pretty(&self.current_version)
            .map_err(|e| format!("Serialization error: {}", e))?;
        fs::write(&version_file, &content)
            .map_err(|e| format!("Write error: {}", e))?;

        let rollback_file = self.versions_dir.join("rollbacks.json");
        let rb_content = serde_json::to_string_pretty(&self.rollback_points)
            .map_err(|e| format!("Serialization error: {}", e))?;
        fs::write(&rollback_file, &rb_content)
            .map_err(|e| format!("Write error: {}", e))?;

        Ok(())
    }

    pub fn get_current_version(&self) -> &VersionInfo { &self.current_version }
    pub fn get_version_string(&self) -> String { self.current_version.version.clone() }

    pub fn install_version(&mut self, version: &str, changelog: Vec<String>) -> Result<String, String> {
        let old_version = self.current_version.version.clone();

        let backup_id = format!("rb-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        let backup_path = self.versions_dir.join(&backup_id);
        fs::create_dir_all(&backup_path)
            .map_err(|e| format!("Failed to create backup dir: {}", e))?;

        self.rollback_points.push(RollbackPoint {
            id: backup_id.clone(),
            timestamp: chrono::Utc::now(),
            description: format!("Before upgrade to {}", version),
            version: old_version.clone(),
            backup_path: backup_path,
        });

        self.current_version = VersionInfo {
            version: version.to_string(),
            build_date: chrono::Utc::now(),
            commit_hash: None,
            changelog,
        };

        self.save_state()?;
        Ok(format!(
            "✓ Installed version {} (was {})\nRollback point: {}",
            version, old_version, backup_id
        ))
    }

    pub fn create_rollback_point(&mut self, description: &str) -> Result<String, String> {
        let id = format!("rb-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        let path = self.versions_dir.join(&id);

        fs::create_dir_all(&path)
            .map_err(|e| format!("Failed to create rollback dir: {}", e))?;

        let point = RollbackPoint {
            id: id.clone(),
            timestamp: chrono::Utc::now(),
            description: description.to_string(),
            version: self.current_version.version.clone(),
            backup_path: path,
        };

        self.rollback_points.push(point);
        self.save_state()?;

        Ok(format!("✓ Created rollback point: {}", id))
    }

    pub fn list_rollback_points(&self) -> String {
        if self.rollback_points.is_empty() {
            return "No rollback points available.".to_string();
        }

        let mut output = format!("Rollback Points ({} total):\n\n", self.rollback_points.len());
        for (i, point) in self.rollback_points.iter().enumerate().rev() {
            output.push_str(&format!(
                "  [{}] {} ({})\n      Version: {}\n      Created: {}\n      Description: {}\n\n",
                i + 1,
                point.id,
                if i == 0 { "latest" } else { "" },
                point.version,
                point.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                point.description
            ));
        }
        output
    }

    pub fn rollback(&mut self, target: &str) -> Result<String, String> {
        let point = if target == "latest" || target.is_empty() {
            self.rollback_points.last()
                .cloned()
                .ok_or_else(|| "No rollback points available".to_string())?
        } else {
            self.rollback_points.iter()
                .find(|p| p.id == target || p.version == target)
                .cloned()
                .ok_or_else(|| format!("Rollback point '{}' not found", target))?
        };

        let current_ver = self.current_version.version.clone();

        self.current_version = VersionInfo {
            version: point.version.clone(),
            build_date: point.timestamp,
            commit_hash: None,
            changelog: vec![format!("Rolled back from {}", current_ver)],
        };

        self.rollback_points.retain(|p| p.id != point.id);
        self.save_state()?;

        Ok(format!(
            "✓ Rolled back to {} (was {})\nRollback point '{}' has been consumed.",
            point.version, current_ver, point.id
        ))
    }

    pub fn get_changelog(&self, count: usize) -> String {
        let entries: Vec<&String> = self.current_version.changelog
            .iter()
            .rev()
            .take(count)
            .collect();

        format!(
            "Changelog for v{}:\n{}",
            self.current_version.version,
            entries.iter().map(|e| format!("  - {}", e)).collect::<Vec<_>>().join("\n")
        )
    }
}
