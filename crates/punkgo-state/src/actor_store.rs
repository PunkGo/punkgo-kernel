//! Actor persistence layer — CRUD operations on the `actors` table.
//!
//! Covers: PIP-001 §5 (actor types), §7 (agent conditional existence).

use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use punkgo_core::actor::{ActorRecord, ActorStatus, ActorType, CreateActorSpec, WritableTarget};
use punkgo_core::errors::{KernelError, KernelResult};

#[derive(Clone)]
pub struct ActorStore {
    pool: SqlitePool,
}

impl ActorStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Look up an actor by ID. Returns None if not found.
    pub async fn get(&self, actor_id: &str) -> KernelResult<Option<ActorRecord>> {
        let row = sqlx::query(
            r#"
            SELECT actor_id, actor_type, creator_id, lineage, purpose, status,
                   writable_targets, energy_share, reduction_policy, created_at, updated_at
            FROM actors
            WHERE actor_id = ?1
            "#,
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_actor_record(&row)?)),
            None => Ok(None),
        }
    }

    /// Check if an actor exists in the actors table.
    pub async fn exists(&self, actor_id: &str) -> KernelResult<bool> {
        let row = sqlx::query("SELECT actor_id FROM actors WHERE actor_id = ?1")
            .bind(actor_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// Check if an actor is active (exists and status = 'active').
    pub async fn is_active(&self, actor_id: &str) -> KernelResult<bool> {
        let row = sqlx::query("SELECT status FROM actors WHERE actor_id = ?1")
            .bind(actor_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                let status: String = row.get("status");
                Ok(status == "active")
            }
            None => Ok(false),
        }
    }

    /// Insert a new actor within an existing transaction.
    pub async fn create_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        spec: &CreateActorSpec,
    ) -> KernelResult<()> {
        if spec.actor_id.trim().is_empty() {
            return Err(KernelError::PolicyViolation(
                "actor_id cannot be empty".to_string(),
            ));
        }

        // Check uniqueness
        let existing = sqlx::query("SELECT actor_id FROM actors WHERE actor_id = ?1")
            .bind(&spec.actor_id)
            .fetch_optional(&mut **tx)
            .await?;
        if existing.is_some() {
            return Err(KernelError::PolicyViolation(format!(
                "actor already exists: {}",
                spec.actor_id
            )));
        }

        let lineage_json = serde_json::to_string(&spec.lineage).map_err(|e| {
            KernelError::PolicyViolation(format!("failed to serialize lineage: {e}"))
        })?;
        let targets_json = serde_json::to_string(&spec.writable_targets).map_err(|e| {
            KernelError::PolicyViolation(format!("failed to serialize writable_targets: {e}"))
        })?;
        let now = now_millis_string();

        sqlx::query(
            r#"
            INSERT INTO actors (
                actor_id, actor_type, creator_id, lineage, purpose, status,
                writable_targets, energy_share, reduction_policy, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?8, ?9, ?9)
            "#,
        )
        .bind(&spec.actor_id)
        .bind(spec.actor_type.as_str())
        .bind(&spec.creator_id)
        .bind(&lineage_json)
        .bind(&spec.purpose)
        .bind(&targets_json)
        .bind(spec.energy_share)
        .bind(&spec.reduction_policy)
        .bind(&now)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Update an actor's status within an existing transaction.
    pub async fn set_status_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        actor_id: &str,
        status: &ActorStatus,
    ) -> KernelResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE actors SET status = ?1, updated_at = ?2 WHERE actor_id = ?3
            "#,
        )
        .bind(status.as_str())
        .bind(now_millis_string())
        .bind(actor_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(KernelError::ActorNotFound(actor_id.to_string()));
        }
        Ok(())
    }

    /// Get the next sequence number for a given creator + purpose pair.
    /// Used for derived identity (agent ID generation).
    pub async fn next_sequence(&self, creator_id: &str, purpose: &str) -> KernelResult<i64> {
        // Count existing actors created by this creator with matching purpose prefix
        let pattern = format!("{creator_id}/{purpose}/%");
        let row = sqlx::query("SELECT COUNT(*) AS cnt FROM actors WHERE actor_id LIKE ?1")
            .bind(&pattern)
            .fetch_one(&self.pool)
            .await?;
        let count: i64 = row.get("cnt");
        Ok(count + 1)
    }

    /// List all active actors with energy_share > 0.
    /// Returns (actor_id, energy_share) pairs for energy production distribution (PIP-001 §3).
    pub async fn list_active_with_shares(&self) -> KernelResult<Vec<(String, f64)>> {
        let rows = sqlx::query(
            r#"
            SELECT actor_id, energy_share
            FROM actors
            WHERE status = 'active' AND energy_share > 0.0
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|row| {
                let actor_id: String = row.get("actor_id");
                let share: f64 = row.get("energy_share");
                (actor_id, share)
            })
            .collect())
    }

    /// List all direct children of a given actor.
    pub async fn list_children(&self, parent_id: &str) -> KernelResult<Vec<ActorRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT actor_id, actor_type, creator_id, lineage, purpose, status,
                   writable_targets, energy_share, reduction_policy, created_at, updated_at
            FROM actors
            WHERE creator_id = ?1
            "#,
        )
        .bind(parent_id)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_actor_record).collect()
    }

    /// List all active descendants of a given actor — actors whose lineage
    /// JSON array contains the ancestor_id.
    ///
    /// Used for cascade freeze operations. When an actor is frozen,
    /// all actors in its subtree (identified by lineage) are also frozen.
    pub async fn list_descendants(&self, ancestor_id: &str) -> KernelResult<Vec<ActorRecord>> {
        let lineage_pattern = format!("%\"{}\"%", ancestor_id);
        let rows = sqlx::query(
            r#"
            SELECT actor_id, actor_type, creator_id, lineage, purpose, status,
                   writable_targets, energy_share, reduction_policy, created_at, updated_at
            FROM actors
            WHERE lineage LIKE ?1 AND status = 'active'
            "#,
        )
        .bind(&lineage_pattern)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_actor_record).collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_actor_record(row: &sqlx::sqlite::SqliteRow) -> KernelResult<ActorRecord> {
    let actor_type_str: String = row.get("actor_type");
    let status_str: String = row.get("status");
    let lineage_json: String = row.get("lineage");
    let targets_json: String = row.get("writable_targets");
    let creator_id: Option<String> = row.get("creator_id");
    let purpose: Option<String> = row.get("purpose");

    let actor_type = ActorType::parse(&actor_type_str).ok_or_else(|| {
        KernelError::PolicyViolation(format!("invalid actor_type in DB: {actor_type_str}"))
    })?;
    let status = ActorStatus::parse(&status_str).ok_or_else(|| {
        KernelError::PolicyViolation(format!("invalid actor status in DB: {status_str}"))
    })?;
    let lineage: Vec<String> = serde_json::from_str(&lineage_json)
        .map_err(|e| KernelError::PolicyViolation(format!("invalid lineage JSON: {e}")))?;
    let writable_targets: Vec<WritableTarget> = serde_json::from_str(&targets_json)
        .map_err(|e| KernelError::PolicyViolation(format!("invalid writable_targets JSON: {e}")))?;

    Ok(ActorRecord {
        actor_id: row.get("actor_id"),
        actor_type,
        creator_id,
        lineage,
        purpose,
        status,
        writable_targets,
        energy_share: row.get("energy_share"),
        reduction_policy: row.get("reduction_policy"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}
