//! Portable native-provider response projections. No I/O or UI runtime.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Serialize)]
pub enum SessionState {
    Connecting,
    Connected,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct RemoteSession {
    pub id: String,
    pub peer: String,
    pub status: SessionState,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct RemoteSnapshot {
    pub instance: String,
    pub generation: u64,
    pub endpoint: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub selected: Option<String>,
    pub sessions: Vec<RemoteSession>,
}
