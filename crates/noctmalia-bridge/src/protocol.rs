//! Wire types for bridge protocol v1 (`docs/bridge-protocol.md`).
//!
//! One JSON object per line in each direction. We send requests and receive replies and events;
//! the shim's own `{"shim": ...}` notices go to the extension, never to us.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(crate) struct Request<'a> {
    pub id: u64,
    pub method: &'a str,
    pub params: &'a Value,
}

/// A line from the bridge: either a reply (`id` plus `result` or `error`) or an event.
#[derive(Debug, Deserialize)]
pub(crate) struct Incoming {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RemoteError>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub data: Option<Value>,
}

/// An error raised inside Thunderbird, or by the shim.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteError {
    #[serde(default = "unnamed")]
    pub name: String,
    #[serde(default)]
    pub message: String,
}

fn unnamed() -> String {
    "Error".to_string()
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}
