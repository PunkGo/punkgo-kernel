use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use punkgo_core::errors::KernelResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub log_index: i64,
    pub event_hash: String,
    pub actor_id: String,
    pub action_type: String,
    pub target: String,
    pub payload: serde_json::Value,
    pub payload_hash: String,
    /// SHA-256(stdout || stderr) for execute actions; None otherwise.
    pub artifact_hash: Option<String>,
    pub reserved_energy: i64,
    pub settled_energy: i64,
    pub timestamp: String,
}

/// Canonical event encoding for RFC 8785 (JCS) — fields MUST be in
/// alphabetical Unicode codepoint order for deterministic hashing.
/// `artifact_hash` is omitted when None (execute outputs absent).
#[derive(Serialize)]
struct CanonicalEvent<'a> {
    action_type: &'a str,
    actor_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_hash: Option<&'a str>,
    event_id: &'a str,
    log_index: i64,
    payload_hash: &'a str,
    reserved_energy: i64,
    settled_energy: i64,
    timestamp: &'a str,
    v: &'static str,
}

/// Computes event_hash = SHA-256(0x00 || canonical_json_bytes)
/// per RFC 6962 leaf domain separation.
fn compute_event_hash(event: &EventRecord) -> KernelResult<String> {
    let canonical = CanonicalEvent {
        action_type: &event.action_type,
        actor_id: &event.actor_id,
        artifact_hash: event.artifact_hash.as_deref(),
        event_id: &event.id,
        log_index: event.log_index,
        payload_hash: &event.payload_hash,
        reserved_energy: event.reserved_energy,
        settled_energy: event.settled_energy,
        timestamp: &event.timestamp,
        v: "punkgo-event-v1",
    };
    let json_bytes = serde_json::to_vec(&canonical)?;
    let mut input = Vec::with_capacity(1 + json_bytes.len());
    input.push(0x00u8);
    input.extend_from_slice(&json_bytes);
    Ok(bytes_to_hex(Sha256::digest(&input).as_slice()))
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

#[derive(Clone)]
pub struct EventLog {
    pool: SqlitePool,
}

impl EventLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Appends an event, assigning a monotonic log_index and computing
    /// event_hash in place. Wraps in an internal transaction for atomicity.
    pub async fn append(&self, event: &mut EventRecord) -> KernelResult<()> {
        let mut tx = self.pool.begin().await?;
        self.assign_and_insert(&mut tx, event).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn append_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        event: &mut EventRecord,
    ) -> KernelResult<()> {
        self.assign_and_insert(tx, event).await
    }

    /// Atomically assigns log_index (MAX+1 within tx), computes event_hash,
    /// then inserts the complete event record.
    async fn assign_and_insert(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        event: &mut EventRecord,
    ) -> KernelResult<()> {
        let row = sqlx::query("SELECT COALESCE(MAX(log_index), -1) + 1 AS next_idx FROM events")
            .fetch_one(&mut **tx)
            .await?;
        event.log_index = row.get::<i64, _>("next_idx");
        event.event_hash = compute_event_hash(event)?;

        let payload_json = serde_json::to_string(&event.payload)?;

        sqlx::query(
            r#"
            INSERT INTO events (
                id,
                log_index,
                event_hash,
                actor_id,
                action_type,
                target,
                payload,
                payload_hash,
                artifact_hash,
                reserved_energy,
                settled_energy,
                timestamp
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
        )
        .bind(&event.id)
        .bind(event.log_index)
        .bind(&event.event_hash)
        .bind(&event.actor_id)
        .bind(&event.action_type)
        .bind(&event.target)
        .bind(&payload_json)
        .bind(&event.payload_hash)
        .bind(event.artifact_hash.as_deref())
        .bind(event.reserved_energy)
        .bind(event.settled_energy)
        .bind(&event.timestamp)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> KernelResult<Vec<EventRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, log_index, event_hash, actor_id, action_type, target, payload,
                   payload_hash, artifact_hash, reserved_energy, settled_energy, timestamp
            FROM events
            ORDER BY log_index DESC
            LIMIT ?1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let events = rows
            .into_iter()
            .map(|row| {
                let payload_str: String = row.get("payload");
                EventRecord {
                    id: row.get("id"),
                    log_index: row.get("log_index"),
                    event_hash: row.get("event_hash"),
                    actor_id: row.get("actor_id"),
                    action_type: row.get("action_type"),
                    target: row.get("target"),
                    payload: serde_json::from_str(&payload_str).unwrap_or_default(),
                    payload_hash: row.get("payload_hash"),
                    artifact_hash: row.get("artifact_hash"),
                    reserved_energy: row.get("reserved_energy"),
                    settled_energy: row.get("settled_energy"),
                    timestamp: row.get("timestamp"),
                }
            })
            .collect();
        Ok(events)
    }

    /// List events filtered by actor_id, ordered by log_index descending.
    pub async fn list_by_actor(
        &self,
        actor_id: &str,
        limit: i64,
    ) -> KernelResult<Vec<EventRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, log_index, event_hash, actor_id, action_type, target, payload,
                   payload_hash, artifact_hash, reserved_energy, settled_energy, timestamp
            FROM events
            WHERE actor_id = ?1
            ORDER BY log_index DESC
            LIMIT ?2
            "#,
        )
        .bind(actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let events = rows
            .into_iter()
            .map(|row| {
                let payload_str: String = row.get("payload");
                EventRecord {
                    id: row.get("id"),
                    log_index: row.get("log_index"),
                    event_hash: row.get("event_hash"),
                    actor_id: row.get("actor_id"),
                    action_type: row.get("action_type"),
                    target: row.get("target"),
                    payload: serde_json::from_str(&payload_str).unwrap_or_default(),
                    payload_hash: row.get("payload_hash"),
                    artifact_hash: row.get("artifact_hash"),
                    reserved_energy: row.get("reserved_energy"),
                    settled_energy: row.get("settled_energy"),
                    timestamp: row.get("timestamp"),
                }
            })
            .collect();
        Ok(events)
    }

    pub async fn count(&self) -> KernelResult<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM events")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("count"))
    }
}
