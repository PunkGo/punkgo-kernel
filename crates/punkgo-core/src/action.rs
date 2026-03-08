use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::errors::KernelResult;

/// The four atomic action types defined in whitepaper §2.
///
/// Energy cost = action_cost + append_cost (see [`quote_cost`]).
///
/// **Action cost** (the operation itself):
/// - `Observe` — lowest-privilege recording, exempt from boundary checks (action cost: 0)
/// - `Create` — creates new actors or envelopes (action cost: 10)
/// - `Mutate` — modifies state, triggers lifecycle operations (action cost: 15)
/// - `Execute` — actor-submitted execution result (action cost: 25 + output_bytes/256)
///
/// **Append cost** (recording to log, universal): 1 + payload_bytes/1024
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Observe,
    Create,
    Mutate,
    Execute,
}

impl ActionType {
    /// Returns `true` for all types except `Observe`.
    pub fn is_state_changing(&self) -> bool {
        !matches!(self, Self::Observe)
    }

    /// Returns the lowercase string representation used in serialization and event logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Create => "create",
            Self::Mutate => "mutate",
            Self::Execute => "execute",
        }
    }
}

/// An atomic action submitted by an actor to the kernel.
///
/// Each action targets a specific path and carries an optional JSON payload.
/// The kernel validates, quotes, reserves energy, executes, settles, and appends
/// the resulting event to the log (the 7-step pipeline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub actor_id: String,
    pub action_type: ActionType,
    pub target: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// Computes the energy cost for an action (PIP-001 §4, PIP-002 §4).
///
/// Total cost = action_cost + append_cost.
///
/// **Action cost** (the operation itself):
/// - Observe: 0 (observing is free)
/// - Create: 10 (new actor or envelope)
/// - Mutate: 15 (state change)
/// - Execute: 25 + output_bytes / 256 (actor-submitted execution result)
///
/// **Append cost** (recording to the log, shared by all actions):
/// - 1 + payload_bytes / 1024
///
/// Every action, including observe, appends to the event log and updates the
/// Merkle tree. This append has physical cost (Landauer: recording ≥ kT ln2).
/// The append cost scales with payload size, creating a natural economic
/// incentive to externalize large content to a blob store.
pub fn quote_cost(action: &Action) -> u64 {
    let action_cost = match action.action_type {
        ActionType::Observe => 0,
        ActionType::Create => 10,
        ActionType::Mutate => 15,
        ActionType::Execute => {
            let output_bytes = action
                .payload
                .get("output_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            25 + output_bytes.saturating_div(256)
        }
    };
    action_cost + append_cost(&action.payload)
}

/// The cost of appending an event to the log (Merkle tree update, SQLite write,
/// SHA-256 hash). Applied to every action regardless of type.
///
/// Formula: 1 (pipeline overhead) + payload_bytes / 1024 (storage cost).
fn append_cost(payload: &serde_json::Value) -> u64 {
    let payload_bytes = serde_json::to_vec(payload)
        .map(|v| v.len() as u64)
        .unwrap_or(0);
    1 + payload_bytes / 1024
}

/// Computes the SHA-256 hash of the action's payload as a hex string.
pub fn payload_hash_hex(action: &Action) -> KernelResult<String> {
    let payload_bytes = serde_json::to_vec(&action.payload)?;
    let mut hasher = Sha256::new();
    hasher.update(payload_bytes);
    let digest = hasher.finalize();
    Ok(bytes_to_hex(&digest))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize] as char);
        out.push(LUT[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Action, ActionType, payload_hash_hex, quote_cost};

    fn make_action(action_type: ActionType, payload: serde_json::Value) -> Action {
        Action {
            actor_id: "root".to_string(),
            action_type,
            target: "workspace/a".to_string(),
            payload,
            timestamp: None,
        }
    }

    #[test]
    fn quote_observe_scales_with_payload() {
        // Empty payload: base cost only.
        let empty = make_action(ActionType::Observe, json!({}));
        assert_eq!(quote_cost(&empty), 1);

        // Small payload (~200 bytes): still 1 (200/1024 = 0).
        let small = make_action(ActionType::Observe, json!({ "msg": "hello" }));
        assert_eq!(quote_cost(&small), 1);

        // 2KB payload: 1 + 2 = 3.
        let big_value = "x".repeat(2048);
        let medium = make_action(ActionType::Observe, json!({ "data": big_value }));
        let cost = quote_cost(&medium);
        assert!(cost >= 3, "2KB payload should cost >= 3, got {cost}");
    }

    #[test]
    fn quote_execute_uses_output_bytes() {
        // Execute cost = 25 + output_bytes/256 (action) + 1 + payload_bytes/1024 (append).
        let no_io = make_action(ActionType::Execute, json!({}));
        // action: 25, append: 1 (empty payload < 1KB) → 26
        assert_eq!(quote_cost(&no_io), 26);

        let small_io = make_action(ActionType::Execute, json!({ "output_bytes": 256 }));
        // action: 25 + 1 = 26, append: 1 ({"output_bytes":256} < 1KB) → 27
        assert_eq!(quote_cost(&small_io), 27);

        let large_io = make_action(ActionType::Execute, json!({ "output_bytes": 2560 }));
        // action: 25 + 10 = 35, append: 1 ({"output_bytes":2560} < 1KB) → 36
        assert_eq!(quote_cost(&large_io), 36);
    }

    #[test]
    fn payload_hash_is_deterministic() {
        let action = make_action(ActionType::Mutate, json!({ "k": "v" }));
        let h1 = payload_hash_hex(&action).expect("hash should succeed");
        let h2 = payload_hash_hex(&action).expect("hash should succeed");
        assert_eq!(h1, h2);
    }
}
