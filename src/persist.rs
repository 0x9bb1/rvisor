use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProgramSnapshot {
    pub name: String,
    pub state: String,
    pub pid: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub programs: Vec<ProgramSnapshot>,
}

/// Derives the state-file path from the socket path by swapping the extension.
/// `/tmp/rvisor.sock` → `/tmp/rvisor.state`
pub fn state_path(sock_path: &Path) -> PathBuf {
    sock_path.with_extension("state")
}

/// Reads, deserializes, and deletes the snapshot in one step.  Returns `None`
/// on any error (missing file, bad JSON) so callers treat absence as a clean
/// start.  Deleting immediately prevents a subsequent crash from replaying it.
pub fn load_and_remove(path: &Path) -> Option<StateSnapshot> {
    let content = std::fs::read_to_string(path).ok()?;
    let snapshot = serde_json::from_str(&content).ok()?;
    let _ = std::fs::remove_file(path);
    Some(snapshot)
}

/// Serializes and writes the snapshot.  Logs a warning on failure rather than
/// propagating the error — a missing snapshot on the next startup is safe.
pub fn save(path: &Path, snapshot: &StateSnapshot) {
    match serde_json::to_string(snapshot) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                tracing::warn!("failed to write state snapshot {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("failed to serialize state snapshot: {e}"),
    }
}
