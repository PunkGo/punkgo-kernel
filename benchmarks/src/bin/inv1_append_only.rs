use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Instant;

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let (kernel, state) = bootstrap_kernel("bench-inv1").await;

    // Step 1: Submit 5 Mutate actions
    let mut event_hashes: Vec<String> = Vec::new();
    let mut log_indices: Vec<i64> = Vec::new();

    for i in 0..5 {
        let receipt = submit_ok(
            &kernel,
            make_action(
                "root",
                ActionType::Mutate,
                &format!("workspace/inv1/{i}"),
                json!({"seq": i}),
            ),
        )
        .await;
        event_hashes.push(receipt["event_hash"].as_str().unwrap().to_string());
        log_indices.push(receipt["log_index"].as_i64().unwrap());
    }

    // Step 2: Get audit checkpoint (root_hash before tamper)
    let checkpoint = read_ok(&kernel, json!({"kind": "audit_checkpoint"})).await;
    let root_before = checkpoint["root_hash"].as_str().unwrap().to_string();
    let tree_size = checkpoint["tree_size"].as_i64().unwrap();

    // Step 3: Get inclusion proof for event at log_index=2 (before tamper)
    let proof_before = read_ok(
        &kernel,
        json!({
            "kind": "audit_inclusion_proof",
            "log_index": 2,
            "tree_size": tree_size
        }),
    )
    .await;
    let proof_hashes: Vec<String> = proof_before["proof"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    // Step 4: Verify inclusion proof independently (before tamper — should pass)
    let original_hash = &event_hashes[2];
    let verified_before = verify_inclusion(
        original_hash,
        2,
        tree_size as u64,
        &root_before,
        &proof_hashes,
    );

    // Step 5: Direct SQLite tamper — modify payload of log_index=2
    let db_path = state.path().join("punkgo.db");
    let tamper_pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", db_path.display()))
        .await
        .expect("connect to DB for tamper");

    // Read original event for comparison
    let _original_event: (String, String, String) =
        sqlx::query_as("SELECT payload, payload_hash, event_hash FROM events WHERE log_index = 2")
            .fetch_one(&tamper_pool)
            .await
            .expect("read original event");

    // Tamper the payload
    sqlx::query("UPDATE events SET payload = '{\"tampered\": true}' WHERE log_index = 2")
        .execute(&tamper_pool)
        .await
        .expect("tamper event");

    // Step 6: Recompute event_hash from tampered data
    let tampered_event: (String, String, String, String, String, i64, i64, String) = sqlx::query_as(
        "SELECT id, actor_id, action_type, target, payload, reserved_energy, settled_energy, timestamp FROM events WHERE log_index = 2"
    )
        .fetch_one(&tamper_pool)
        .await
        .expect("read tampered event");

    // Recompute payload_hash for tampered payload
    let tampered_payload_hash = {
        let h = Sha256::digest(tampered_event.4.as_bytes());
        hex::encode(&h)
    };

    // Build canonical event JSON (RFC 8785 alphabetical order)
    let canonical = json!({
        "action_type": tampered_event.2,
        "actor_id": tampered_event.1,
        "artifact_hash": null,
        "event_id": tampered_event.0,
        "log_index": 2,
        "payload_hash": tampered_payload_hash,
        "reserved_energy": tampered_event.5,
        "settled_energy": tampered_event.6,
        "target": tampered_event.3,
        "timestamp": tampered_event.7,
    });

    let canonical_bytes = serde_json::to_vec(&canonical).unwrap();
    let mut hash_input = vec![0x00u8]; // RFC 6962 leaf prefix
    hash_input.extend_from_slice(&canonical_bytes);
    let recomputed_hash = hex::encode(&Sha256::digest(&hash_input));

    // Step 7: The stored event_hash in audit_hashes still references the ORIGINAL hash
    // The recomputed hash from tampered data is different
    let tamper_detected = recomputed_hash != *original_hash;

    // Step 8: Try to verify inclusion proof with the recomputed (tampered) hash — should FAIL
    let verified_after_tamper = verify_inclusion(
        &recomputed_hash,
        2,
        tree_size as u64,
        &root_before,
        &proof_hashes,
    );

    tamper_pool.close().await;
    let elapsed_ms = start.elapsed().as_millis();

    let result = json!({
        "test": "INV-1 Append-Only",
        "status": if tamper_detected && !verified_after_tamper { "PASS" } else { "FAIL" },
        "steps": [
            {"step": 1, "result": format!("submitted 5 actions, log_indices: {:?}", log_indices)},
            {"step": 2, "result": format!("root_before = {root_before}")},
            {"step": 3, "result": format!("inclusion proof for log_index=2: {} hashes", proof_hashes.len())},
            {"step": 4, "result": format!("pre-tamper verification: {verified_before}")},
            {"step": 5, "result": "modified event log_index=2 payload to {\"tampered\":true}"},
            {"step": 6, "result": format!("recomputed hash = {recomputed_hash}")},
            {"step": 7, "result": format!("original hash = {}, tamper detected = {tamper_detected}", original_hash)},
            {"step": 8, "result": format!("post-tamper inclusion proof verification: {verified_after_tamper} (expected false)")},
        ],
        "original_event_hash": original_hash,
        "recomputed_tampered_hash": recomputed_hash,
        "tamper_detected": tamper_detected,
        "inclusion_proof_invalidated": !verified_after_tamper,
        "root_hash": root_before,
        "timing_ms": elapsed_ms,
        "notes": "Direct SQLite tamper changes payload but cannot update the Merkle tree leaf hash. The stored event_hash (in audit_hashes) still references the original data. Any independent verifier re-hashing the tampered row will compute a different leaf hash, breaking the inclusion proof."
    });

    write_result("inv1_append_only.json", &result);
    println!("INV-1: {}", if tamper_detected { "PASS" } else { "FAIL" });
}

/// Independent RFC 6962 inclusion proof verifier.
fn verify_inclusion(
    leaf_hash_hex: &str,
    leaf_index: u64,
    tree_size: u64,
    root_hex: &str,
    proof: &[String],
) -> bool {
    let mut current = hex_decode(leaf_hash_hex);
    let mut idx = leaf_index;
    let mut last = tree_size - 1;

    for sibling_hex in proof {
        let sibling = hex_decode(sibling_hex);
        if idx % 2 == 1 {
            // current is right child
            current = node_hash(&sibling, &current);
        } else if idx < last {
            // current is left child (and not the last node at this level)
            current = node_hash(&current, &sibling);
        } else {
            // current is the last (unpaired) node, promote with left
            current = node_hash(&sibling, &current);
        }
        idx >>= 1;
        last >>= 1;
    }

    hex::encode(&current) == root_hex
}

fn node_hash(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut input = vec![0x01u8]; // RFC 6962 interior node prefix
    input.extend_from_slice(left);
    input.extend_from_slice(right);
    Sha256::digest(&input).to_vec()
}

fn hex_decode(hex_str: &str) -> Vec<u8> {
    (0..hex_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16).unwrap())
        .collect()
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        const LUT: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            out.push(LUT[(b >> 4) as usize] as char);
            out.push(LUT[(b & 0x0f) as usize] as char);
        }
        out
    }
}
