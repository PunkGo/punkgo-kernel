use punkgo_state::{EnergyLedger, EventLog, EventRecord, StateStore};
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

#[tokio::test]
async fn reserve_and_settle_updates_balance() {
    let dir = test_dir("punkgo-state-ledger");
    let store = StateStore::bootstrap(&dir)
        .await
        .expect("state bootstrap should succeed");
    let pool = store.pool();

    sqlx::query(
        r#"
        INSERT INTO energy_ledger (actor_id, energy_balance, reserved_energy, updated_at)
        VALUES ('alice', 100, 0, ?1)
        "#,
    )
    .bind(now_millis_string())
    .execute(&pool)
    .await
    .expect("seed actor should succeed");

    let ledger = EnergyLedger::new(pool.clone());
    let reservation = ledger
        .reserve("alice", 30)
        .await
        .expect("reserve should succeed");
    assert_eq!(reservation.reserved, 30);

    ledger
        .settle("alice", 30, 25)
        .await
        .expect("settle should succeed");

    let (balance, reserved) = ledger
        .balance_view("alice")
        .await
        .expect("balance should exist");
    assert_eq!(balance, 75);
    assert_eq!(reserved, 0);

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn append_and_list_events() {
    let dir = test_dir("punkgo-state-events");
    let store = StateStore::bootstrap(&dir)
        .await
        .expect("state bootstrap should succeed");
    let event_log = EventLog::new(store.pool());

    let mut event = EventRecord {
        id: Uuid::new_v4().to_string(),
        log_index: 0,
        event_hash: String::new(),
        actor_id: "root".to_string(),
        action_type: "mutate".to_string(),
        target: "test/target".to_string(),
        payload: serde_json::json!({"test": true}),
        payload_hash: "abc123".to_string(),
        artifact_hash: None,
        reserved_energy: 15,
        settled_energy: 15,
        timestamp: now_millis_string(),
    };
    event_log
        .append(&mut event)
        .await
        .expect("append event should succeed");

    assert_eq!(event.log_index, 0, "first event should get log_index 0");
    assert!(
        !event.event_hash.is_empty(),
        "event_hash should be computed"
    );

    let events = event_log
        .list_recent(1)
        .await
        .expect("list recent should succeed");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action_type, "mutate");
    assert_eq!(events[0].log_index, 0);
    assert!(!events[0].event_hash.is_empty());

    let pool = store.pool();
    let row = sqlx::query("SELECT COUNT(*) AS count FROM events")
        .fetch_one(&pool)
        .await
        .expect("count query should succeed");
    let count: i64 = row.get("count");
    assert_eq!(count, 1);

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
