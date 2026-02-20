//! Envelope persistence layer — CRUD operations on the `envelopes` table.
//!
//! Covers: PIP-001 §11 (envelope: budget + targets + actions + duration + checkpoint + hold).
//! Whitepaper §3 invariant 6: authorization changes enter event history.

use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use punkgo_core::consent::{EnvelopeRecord, EnvelopeSpec, EnvelopeStatus, HoldRule};
use punkgo_core::errors::{KernelError, KernelResult};

/// PIP-001 §11b: Parameters for creating a hold_request record.
///
/// Groups the data fields passed to [`EnvelopeStore::create_hold_request_in_tx`]
/// so that the function signature stays within clippy's argument-count limit.
pub struct NewHoldRequest<'a> {
    /// Stable identifier for this hold (UUID string).
    pub hold_id: &'a str,
    /// The envelope that was held.
    pub envelope_id: &'a str,
    /// The agent whose action triggered the hold.
    pub agent_id: &'a str,
    /// The target path of the triggering action.
    pub trigger_target: &'a str,
    /// The action type string of the triggering action.
    pub trigger_action: &'a str,
    /// Full payload snapshot of the pending action, stored as JSON.
    pub pending_payload: &'a serde_json::Value,
}

#[derive(Clone)]
pub struct EnvelopeStore {
    pool: SqlitePool,
}

impl EnvelopeStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a new envelope within an existing transaction.
    pub async fn create_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        envelope_id: &str,
        spec: &EnvelopeSpec,
        parent_envelope_id: Option<&str>,
    ) -> KernelResult<EnvelopeRecord> {
        let targets_json = serde_json::to_string(&spec.targets).map_err(|e| {
            KernelError::PolicyViolation(format!("failed to serialize targets: {e}"))
        })?;
        let actions_json = serde_json::to_string(&spec.actions).map_err(|e| {
            KernelError::PolicyViolation(format!("failed to serialize actions: {e}"))
        })?;
        let hold_on_json = serde_json::to_string(&spec.hold_on).map_err(|e| {
            KernelError::PolicyViolation(format!("failed to serialize hold_on: {e}"))
        })?;
        let now = now_millis_string();

        // Compute expires_at from duration_secs
        let expires_at = spec.duration_secs.map(|d| {
            let now_ms: u64 = now.parse().unwrap_or(0);
            let expires_ms = now_ms + (d as u64) * 1000;
            expires_ms.to_string()
        });

        sqlx::query(
            r#"
            INSERT INTO envelopes (
                envelope_id, actor_id, grantor_id, parent_envelope_id,
                budget, budget_consumed, targets, actions,
                duration_secs, report_every,
                hold_on, hold_timeout_secs,
                status, last_report_at,
                created_at, expires_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9,
                    ?10, ?11,
                    'active', 0, ?12, ?13, ?12)
            "#,
        )
        .bind(envelope_id)
        .bind(&spec.actor_id)
        .bind(&spec.grantor_id)
        .bind(parent_envelope_id)
        .bind(spec.budget)
        .bind(&targets_json)
        .bind(&actions_json)
        .bind(spec.duration_secs)
        .bind(spec.report_every)
        .bind(&hold_on_json)
        .bind(spec.hold_timeout_secs)
        .bind(&now)
        .bind(&expires_at)
        .execute(&mut **tx)
        .await?;

        Ok(EnvelopeRecord {
            envelope_id: envelope_id.to_string(),
            actor_id: spec.actor_id.clone(),
            grantor_id: spec.grantor_id.clone(),
            parent_envelope_id: parent_envelope_id.map(|s| s.to_string()),
            budget: spec.budget,
            budget_consumed: 0,
            targets: spec.targets.clone(),
            actions: spec.actions.clone(),
            duration_secs: spec.duration_secs,
            report_every: spec.report_every,
            hold_on: spec.hold_on.clone(),
            hold_timeout_secs: spec.hold_timeout_secs,
            status: EnvelopeStatus::Active,
            last_report_at: 0,
            created_at: now.clone(),
            expires_at,
            updated_at: now,
        })
    }

    /// Get the active envelope for an actor. Returns None if no active envelope exists.
    pub async fn get_active_for_actor(
        &self,
        actor_id: &str,
    ) -> KernelResult<Option<EnvelopeRecord>> {
        let row = sqlx::query(
            r#"
            SELECT envelope_id, actor_id, grantor_id, parent_envelope_id,
                   budget, budget_consumed, targets, actions,
                   duration_secs, report_every,
                   hold_on, hold_timeout_secs,
                   status, last_report_at,
                   created_at, expires_at, updated_at
            FROM envelopes
            WHERE actor_id = ?1 AND status = 'active'
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_envelope_record(&row)?)),
            None => Ok(None),
        }
    }

    /// Get an envelope by ID.
    pub async fn get(&self, envelope_id: &str) -> KernelResult<Option<EnvelopeRecord>> {
        let row = sqlx::query(
            r#"
            SELECT envelope_id, actor_id, grantor_id, parent_envelope_id,
                   budget, budget_consumed, targets, actions,
                   duration_secs, report_every,
                   hold_on, hold_timeout_secs,
                   status, last_report_at,
                   created_at, expires_at, updated_at
            FROM envelopes
            WHERE envelope_id = ?1
            "#,
        )
        .bind(envelope_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_envelope_record(&row)?)),
            None => Ok(None),
        }
    }

    /// PIP-001 §11b: Get a hold_request record by hold_id.
    pub async fn get_hold_request(&self, hold_id: &str) -> KernelResult<Option<serde_json::Value>> {
        let row = sqlx::query(
            r#"
            SELECT hold_id, envelope_id, agent_id, trigger_target, trigger_action,
                   pending_payload, status, decision, instruction, triggered_at, resolved_at
            FROM hold_requests
            WHERE hold_id = ?1
            "#,
        )
        .bind(hold_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(serde_json::json!({
                "hold_id": r.get::<String, _>("hold_id"),
                "envelope_id": r.get::<String, _>("envelope_id"),
                "agent_id": r.get::<String, _>("agent_id"),
                "trigger_target": r.get::<String, _>("trigger_target"),
                "trigger_action": r.get::<String, _>("trigger_action"),
                "pending_payload": serde_json::from_str::<serde_json::Value>(
                    &r.get::<String, _>("pending_payload")
                ).unwrap_or(serde_json::Value::Null),
                "status": r.get::<String, _>("status"),
                "decision": r.get::<Option<String>, _>("decision"),
                "instruction": r.get::<Option<String>, _>("instruction"),
                "triggered_at": r.get::<String, _>("triggered_at"),
                "resolved_at": r.get::<Option<String>, _>("resolved_at"),
            }))),
            None => Ok(None),
        }
    }

    /// PIP-001 §11b: List pending hold_requests, optionally filtered by agent_id.
    ///
    /// Returns records ordered by `triggered_at` ascending (oldest first).
    /// If `agent_id` is `None`, all pending holds are returned regardless of agent.
    pub async fn list_pending_holds(
        &self,
        agent_id: Option<&str>,
    ) -> KernelResult<Vec<serde_json::Value>> {
        let rows = if let Some(id) = agent_id {
            sqlx::query(
                r#"
                SELECT hold_id, envelope_id, agent_id, trigger_target, trigger_action,
                       pending_payload, status, decision, instruction,
                       triggered_at, resolved_at
                FROM hold_requests
                WHERE status = 'pending' AND agent_id = ?1
                ORDER BY triggered_at ASC
                "#,
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT hold_id, envelope_id, agent_id, trigger_target, trigger_action,
                       pending_payload, status, decision, instruction,
                       triggered_at, resolved_at
                FROM hold_requests
                WHERE status = 'pending'
                ORDER BY triggered_at ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        };

        let mut result = Vec::new();
        for row in rows {
            let pending_payload: serde_json::Value =
                serde_json::from_str(&row.get::<String, _>("pending_payload"))
                    .unwrap_or(serde_json::Value::Null);
            result.push(serde_json::json!({
                "hold_id":        row.get::<String, _>("hold_id"),
                "envelope_id":    row.get::<String, _>("envelope_id"),
                "agent_id":       row.get::<String, _>("agent_id"),
                "trigger_target": row.get::<String, _>("trigger_target"),
                "trigger_action": row.get::<String, _>("trigger_action"),
                "pending_payload": pending_payload,
                "status":         row.get::<String, _>("status"),
                "decision":       row.get::<Option<String>, _>("decision"),
                "instruction":    row.get::<Option<String>, _>("instruction"),
                "triggered_at":   row.get::<String, _>("triggered_at"),
                "resolved_at":    row.get::<Option<String>, _>("resolved_at"),
            }));
        }
        Ok(result)
    }

    /// PIP-001 §11e: List pending hold_requests for a specific envelope.
    pub async fn list_pending_holds_for_envelope(
        &self,
        envelope_id: &str,
    ) -> KernelResult<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"
            SELECT hold_id, envelope_id, agent_id, trigger_target, trigger_action,
                   pending_payload, status, decision, instruction,
                   triggered_at, resolved_at
            FROM hold_requests
            WHERE status = 'pending' AND envelope_id = ?1
            ORDER BY triggered_at ASC
            "#,
        )
        .bind(envelope_id)
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        for row in rows {
            let pending_payload: serde_json::Value =
                serde_json::from_str(&row.get::<String, _>("pending_payload"))
                    .unwrap_or(serde_json::Value::Null);
            result.push(serde_json::json!({
                "hold_id":        row.get::<String, _>("hold_id"),
                "envelope_id":    row.get::<String, _>("envelope_id"),
                "agent_id":       row.get::<String, _>("agent_id"),
                "trigger_target": row.get::<String, _>("trigger_target"),
                "trigger_action": row.get::<String, _>("trigger_action"),
                "pending_payload": pending_payload,
                "status":         row.get::<String, _>("status"),
                "decision":       row.get::<Option<String>, _>("decision"),
                "instruction":    row.get::<Option<String>, _>("instruction"),
                "triggered_at":   row.get::<String, _>("triggered_at"),
                "resolved_at":    row.get::<Option<String>, _>("resolved_at"),
            }));
        }
        Ok(result)
    }

    /// PIP-001 §11b: Create a hold_request record within a transaction.
    pub async fn create_hold_request_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        req: &NewHoldRequest<'_>,
    ) -> KernelResult<()> {
        let payload_json = serde_json::to_string(req.pending_payload)?;
        let now = now_millis_string();
        sqlx::query(
            r#"
            INSERT INTO hold_requests (
                hold_id, envelope_id, agent_id, trigger_target, trigger_action,
                pending_payload, status, triggered_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)
            "#,
        )
        .bind(req.hold_id)
        .bind(req.envelope_id)
        .bind(req.agent_id)
        .bind(req.trigger_target)
        .bind(req.trigger_action)
        .bind(&payload_json)
        .bind(&now)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// PIP-001 §11d: Resolve a hold_request (approve or reject) within a transaction.
    ///
    /// `decision` is "approve" or "reject" (PIP-001 §11d HoldResponse).
    /// The DB `status` column uses the past-tense forms "approved"/"rejected".
    pub async fn resolve_hold_request_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        hold_id: &str,
        decision: &str,
        instruction: Option<&str>,
    ) -> KernelResult<()> {
        // Map decision → status (DB uses past-tense forms per CHECK constraint).
        let status = match decision {
            "approve" => "approved",
            "reject" => "rejected",
            "timed_out" => "timed_out",
            other => {
                return Err(KernelError::PolicyViolation(format!(
                    "invalid hold decision: {other}; must be 'approve', 'reject', or 'timed_out'"
                )));
            }
        };

        let now = now_millis_string();
        let result = sqlx::query(
            r#"
            UPDATE hold_requests
            SET status = ?1, decision = ?2, instruction = ?3, resolved_at = ?4
            WHERE hold_id = ?5 AND status = 'pending'
            "#,
        )
        .bind(status)
        .bind(decision)
        .bind(instruction)
        .bind(&now)
        .bind(hold_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(KernelError::PolicyViolation(format!(
                "hold_request {hold_id} not found or already resolved"
            )));
        }
        Ok(())
    }

    /// Consume budget from an envelope within a transaction.
    /// Returns the new total consumed amount.
    pub async fn consume_budget_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        envelope_id: &str,
        amount: i64,
    ) -> KernelResult<i64> {
        let row =
            sqlx::query("SELECT budget, budget_consumed FROM envelopes WHERE envelope_id = ?1")
                .bind(envelope_id)
                .fetch_optional(&mut **tx)
                .await?;

        let Some(row) = row else {
            return Err(KernelError::PolicyViolation(format!(
                "envelope not found: {envelope_id}"
            )));
        };

        let budget: i64 = row.get("budget");
        let consumed: i64 = row.get("budget_consumed");
        let new_consumed = consumed + amount;

        // Allow consuming up to budget (halt is handled at checkpoint level)
        if new_consumed > budget {
            return Err(KernelError::AuthorizationRequired(format!(
                "envelope {} budget exceeded: budget={}, already_consumed={}, requested={}",
                envelope_id, budget, consumed, amount
            )));
        }

        let now = now_millis_string();
        sqlx::query(
            r#"
            UPDATE envelopes
            SET budget_consumed = ?1, updated_at = ?2
            WHERE envelope_id = ?3
            "#,
        )
        .bind(new_consumed)
        .bind(&now)
        .bind(envelope_id)
        .execute(&mut **tx)
        .await?;

        Ok(new_consumed)
    }

    /// Set envelope status within a transaction.
    pub async fn set_status_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        envelope_id: &str,
        status: &EnvelopeStatus,
    ) -> KernelResult<()> {
        let now = now_millis_string();
        let result = sqlx::query(
            r#"
            UPDATE envelopes
            SET status = ?1, updated_at = ?2
            WHERE envelope_id = ?3
            "#,
        )
        .bind(status.as_str())
        .bind(&now)
        .bind(envelope_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(KernelError::PolicyViolation(format!(
                "envelope not found: {envelope_id}"
            )));
        }
        Ok(())
    }

    /// Set envelope status without a transaction (convenience method).
    pub async fn set_status(&self, envelope_id: &str, status: &EnvelopeStatus) -> KernelResult<()> {
        let mut tx = self.pool.begin().await?;
        self.set_status_in_tx(&mut tx, envelope_id, status).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Lazy expiry check: if envelope is time-expired, mark as expired and return true.
    pub async fn check_and_expire(&self, envelope_id: &str) -> KernelResult<bool> {
        let envelope = self.get(envelope_id).await?;
        let Some(envelope) = envelope else {
            return Ok(false);
        };

        if envelope.status != EnvelopeStatus::Active {
            return Ok(false);
        }

        let now_ms = now_millis();
        if punkgo_core::consent::is_envelope_expired(&envelope, now_ms) {
            self.set_status(envelope_id, &EnvelopeStatus::Expired)
                .await?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Update checkpoint tracking fields.
    pub async fn update_checkpoint_tracking(
        &self,
        envelope_id: &str,
        last_report_at: i64,
    ) -> KernelResult<()> {
        let now = now_millis_string();
        sqlx::query(
            "UPDATE envelopes SET last_report_at = ?1, updated_at = ?2 WHERE envelope_id = ?3",
        )
        .bind(last_report_at)
        .bind(&now)
        .bind(envelope_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_envelope_record(row: &sqlx::sqlite::SqliteRow) -> KernelResult<EnvelopeRecord> {
    let status_str: String = row.get("status");
    let targets_json: String = row.get("targets");
    let actions_json: String = row.get("actions");
    let hold_on_json: String = row.get("hold_on");

    let status = EnvelopeStatus::parse(&status_str).ok_or_else(|| {
        KernelError::PolicyViolation(format!("invalid envelope status in DB: {status_str}"))
    })?;
    let targets: Vec<String> = serde_json::from_str(&targets_json)
        .map_err(|e| KernelError::PolicyViolation(format!("invalid targets JSON: {e}")))?;
    let actions: Vec<String> = serde_json::from_str(&actions_json)
        .map_err(|e| KernelError::PolicyViolation(format!("invalid actions JSON: {e}")))?;
    let hold_on: Vec<HoldRule> = serde_json::from_str(&hold_on_json)
        .map_err(|e| KernelError::PolicyViolation(format!("invalid hold_on JSON: {e}")))?;

    Ok(EnvelopeRecord {
        envelope_id: row.get("envelope_id"),
        actor_id: row.get("actor_id"),
        grantor_id: row.get("grantor_id"),
        parent_envelope_id: row.get("parent_envelope_id"),
        budget: row.get("budget"),
        budget_consumed: row.get("budget_consumed"),
        targets,
        actions,
        duration_secs: row.get("duration_secs"),
        report_every: row.get("report_every"),
        hold_on,
        hold_timeout_secs: row.get("hold_timeout_secs"),
        status,
        last_report_at: row.get("last_report_at"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        updated_at: row.get("updated_at"),
    })
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
