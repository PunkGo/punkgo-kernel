use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub event_count: i64,
    pub hash: String,
    pub generated_at: String,
}

impl SnapshotInfo {
    pub fn empty(generated_at: String) -> Self {
        Self {
            event_count: 0,
            hash: String::new(),
            generated_at,
        }
    }
}
