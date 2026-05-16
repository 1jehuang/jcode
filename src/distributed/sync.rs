use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrdtType {
    GCounter,       // Grow-only Counter
    PNCounter,      // Positive-Negative Counter
    GSet,           // Grow-only Set
    ORSet,          // Observed-Remove Set
    LWWRegister,     // Last-Writer-Wins Register
    MVRegister,     // Multi-Value Register
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSync {
    pub sync_id: String,
    pub crdt_type: CrdtType,
    pub data: Vec<u8>,
    pub version: u64,
    pub source_node: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl StateSync {
    pub fn new(crdt_type: CrdtType, source: &str) -> Self {
        StateSync {
            sync_id: uuid::Uuid::new_v4().to_string(),
            crdt_type,
            data: vec![],
            version: 0,
            source_node: source.to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn merge(&mut self, other: &StateSync) -> Result<(), String> {
        match (&self.crdt_type, &other.crdt_type) {
            (CrdtType::GCounter, _) => { self.merge_gcounter(other)?; }
            _ => return Err("Unsupported CRDT type for merge".to_string()),
        }
        Ok(())
    }

    fn merge_gcounter(&mut self, other: &StateSync) -> Result<(), String> {
        if other.version > self.version {
            self.data = other.data.clone();
            self.version = other.version;
        }
        Ok(())
    }

    pub fn increment(&mut self) { self.version += 1; }
    pub fn get_version(&self) -> u64 { self.version }
}
