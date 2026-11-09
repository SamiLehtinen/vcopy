use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SourceMapping {
    pub source_path: String,
    pub destination_prefix: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FileState {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_name: Option<String>,
    pub size: u64,
    pub mtime_ns: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crc32: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub action: String, // "new", "modified", "deleted"
    pub prior_size: Option<u64>,
    pub prior_mtime_ns: Option<i64>,
    pub new_size: Option<u64>,
    pub new_mtime_ns: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub files_copied: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub bytes_transferred: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SnapshotManifest {
    pub version: u32,
    #[serde(default)]
    pub snapshot_name: String,
    pub snapshot_timestamp: DateTime<Utc>,
    pub sources: Vec<SourceMapping>,
    pub summary: RunSummary,
    pub changes: Vec<FileChange>,
    pub state: Vec<FileState>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FlatManifest {
    pub version: u32,
    pub flatten: bool,
    #[serde(default)]
    pub version_counters: BTreeMap<String, u64>,
    pub baseline: SnapshotManifest,
    #[serde(default)]
    pub snapshots: Vec<SnapshotManifest>,
}
