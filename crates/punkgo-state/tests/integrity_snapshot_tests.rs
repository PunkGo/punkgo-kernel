use punkgo_state::{EventLog, EventRecord, StateStore};
use sqlx::Row;
use uuid::Uuid;

fn test_dir(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()))
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}

fn event(action_type: &str) -> EventRecord {
    EventRecord {
        id: Uuid::new_v4().to_string(),
        log_index: 0,
        event_hash: String::new(),
        actor_id: "root".to_string(),
        action_type: action_type.to_string(),
        target: "test/target".to_string(),
        payload: serde_json::json!({"test": true}),
        payload_hash: "abc123".to_string(),
        artifact_hash: None,
        reserved_energy: 10,
        settled_energy: 10,
        timestamp: now_millis_string(),
    }
}

#[tokio::test]
async fn integrity_check_rejects_unknown_action_type() {
    let dir = test_dir("punkgo-state-integrity-action");
    let store = StateStore::bootstrap(&dir)
        .await
        .expect("state bootstrap should succeed");
    let event_log = EventLog::new(store.pool());
    event_log
        .append(&mut event("h4ck"))
        .await
        .expect("append should succeed");

    let err = store
        .run_startup_integrity_checks()
        .await
        .expect_err("unknown action must fail integrity check");
    assert!(
        err.to_string().contains("action integrity violation"),
        "unexpected error: {err}"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn integrity_check_detects_snapshot_hash_mismatch() {
    let dir = test_dir("punkgo-state-integrity-snapshot");
    let store = StateStore::bootstrap(&dir)
        .await
        .expect("state bootstrap should succeed");
    let pool = store.pool();
    let event_log = EventLog::new(pool.clone());
    event_log
        .append(&mut event("mutate"))
        .await
        .expect("append should succeed");
    store
        .refresh_snapshot()
        .await
        .expect("snapshot refresh should succeed");

    sqlx::query("UPDATE system_meta SET value = 'tampered_hash' WHERE key = 'snapshot_hash'")
        .execute(&pool)
        .await
        .expect("tamper hash should succeed");

    let err = store
        .run_startup_integrity_checks()
        .await
        .expect_err("tampered snapshot must fail integrity check");
    assert!(
        err.to_string().contains("snapshot hash mismatch"),
        "unexpected error: {err}"
    );

    let row = sqlx::query("SELECT value FROM system_meta WHERE key = 'snapshot_hash'")
        .fetch_one(&pool)
        .await
        .expect("query should succeed");
    let current: String = row.get("value");
    assert_eq!(current, "tampered_hash");

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn refresh_snapshot_writes_snapshot_file() {
    let dir = test_dir("punkgo-state-snapshot-file");
    let store = StateStore::bootstrap(&dir)
        .await
        .expect("state bootstrap should succeed");
    let event_log = EventLog::new(store.pool());
    event_log
        .append(&mut event("create"))
        .await
        .expect("append should succeed");

    let snapshot = store
        .refresh_snapshot()
        .await
        .expect("snapshot refresh should succeed");
    assert_eq!(snapshot.event_count, 1);
    assert!(!snapshot.hash.is_empty());

    let snapshot_file = store.paths().snapshots_root.join("latest.json");
    assert!(snapshot_file.exists(), "snapshot file should exist");

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
