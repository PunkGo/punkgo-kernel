use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::errors::KernelResult;

/// The four atomic action types defined in whitepaper §2.
///
/// - `Observe` — read-only, zero cost, exempt from boundary checks
/// - `Create` — creates new actors or envelopes (cost: 10)
/// - `Mutate` — modifies state, triggers lifecycle operations (cost: 15)
/// - `Execute` — actor-submitted execution result (cost: 25 + output_bytes/256)
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
/// - Observe: 0
/// - Create: 10
/// - Mutate: 15
/// - Execute: 25 + output_bytes / 256 (output_bytes is actor-reported in payload)
pub fn quote_cost(action: &Action) -> u64 {
    match action.action_type {
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
    }
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
    fn quote_observe_is_zero() {
        let action = make_action(ActionType::Observe, json!({}));
        assert_eq!(quote_cost(&action), 0);
    }

    #[test]
    fn quote_execute_uses_output_bytes() {
        let no_io = make_action(ActionType::Execute, json!({}));
        assert_eq!(quote_cost(&no_io), 25);

        let small_io = make_action(ActionType::Execute, json!({ "output_bytes": 256 }));
        assert_eq!(quote_cost(&small_io), 26);

        let large_io = make_action(ActionType::Execute, json!({ "output_bytes": 2560 }));
        assert_eq!(quote_cost(&large_io), 35);
    }

    #[test]
    fn payload_hash_is_deterministic() {
        let action = make_action(ActionType::Mutate, json!({ "k": "v" }));
        let h1 = payload_hash_hex(&action).expect("hash should succeed");
        let h2 = payload_hash_hex(&action).expect("hash should succeed");
        assert_eq!(h1, h2);
    }
}
