use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};
use uuid::Uuid;

use crate::audit::AuditLog;
use punkgo_core::action::{Action, ActionType, payload_hash_hex, quote_cost};
use punkgo_core::actor::{
    ActorType, CreateActorSpec, WritableTarget, build_lineage, derive_agent_id,
};
use punkgo_core::boundary::{check_writable_boundary, validate_child_targets};
use punkgo_core::consent::{self, AuthorizationMode, CheckpointLevel, EnvelopeSpec, HoldRule};
use punkgo_core::errors::{KernelError, KernelResult};
use punkgo_core::policy::{check_read_access, validate_action};

use super::lifecycle;
use crate::state::{
    ActorStore, EnergyLedger, EnergyReservation, EnvelopeStore, EventLog, EventRecord,
    NewHoldRequest, StateStore,
};
use punkgo_core::protocol::{RequestEnvelope, RequestType, ResponseEnvelope};
use punkgo_core::stellar::{StellarConfig, load_stellar_config};

/// Configuration for bootstrapping the kernel.
#[derive(Debug, Clone)]
pub struct KernelConfig {
    pub state_dir: PathBuf,
    pub ipc_endpoint: String,
}

impl Default for KernelConfig {
    fn default() -> Self {
        let state_dir = std::env::var("PUNKGO_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_state_dir());
        let ipc_endpoint =
            std::env::var("PUNKGO_IPC_ENDPOINT").unwrap_or_else(|_| default_ipc_endpoint());
        Self {
            state_dir,
            ipc_endpoint,
        }
    }
}

/// Default state directory: ~/.punkgo/state
///
/// Falls back to `./state` if home directory cannot be determined.
/// Default IPC endpoint.
///
/// On Windows, uses a file-path named pipe (`\\.\pipe\punkgo-kernel`) because
/// the `GenericNamespaced` namespace can hit "Access Denied" depending on
/// Windows security policy. On Unix, uses a simple namespace name.
fn default_ipc_endpoint() -> String {
    if cfg!(windows) {
        r"\\.\pipe\punkgo-kernel".to_string()
    } else {
        "punkgo-kernel".to_string()
    }
}

fn default_state_dir() -> PathBuf {
    if let Some(home) = home_dir() {
        return home.join(".punkgo").join("state");
    }
    PathBuf::from("state")
}

fn home_dir() -> Option<PathBuf> {
    // Unix / WSL
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home));
    }
    // Windows primary
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    // Windows fallback
    let drive = std::env::var_os("HOMEDRIVE")?;
    let path = std::env::var_os("HOMEPATH")?;
    let mut p = PathBuf::from(drive);
    p.push(path);
    Some(p)
}

/// Cryptographic receipt returned after a successful action submission.
///
/// Contains the event ID, log index, event hash, and energy costs.
/// This is the caller's proof that their action was committed to the
/// append-only history.
#[derive(Debug, Clone, Serialize)]
pub struct SubmitReceipt {
    pub event_id: String,
    pub log_index: i64,
    pub event_hash: String,
    pub reserved_cost: i64,
    pub settled_cost: i64,
    pub artifact_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReadQuery {
    kind: String,
    actor_id: Option<String>,
    /// PIP-001 §11: hold_id for hold_info queries.
    #[serde(default)]
    hold_id: Option<String>,
    limit: Option<i64>,
    /// For audit_inclusion_proof: the event's log_index to prove.
    log_index: Option<i64>,
    /// For audit_inclusion_proof / audit_consistency_proof: the tree size to prove against.
    tree_size: Option<i64>,
    /// For audit_consistency_proof: the older tree size.
    old_size: Option<i64>,
    /// Pagination: only return events with log_index < this value.
    /// Used for backward pagination (fetching older events).
    #[serde(default)]
    before_index: Option<i64>,
    /// Pagination: only return events with log_index > this value.
    /// Used for forward pagination (fetching newer events).
    #[serde(default)]
    after_index: Option<i64>,
    /// Identity of the requester. Used by visibility boundary checks.
    /// Currently optional — when absent, all reads are permitted (single-user).
    /// When multi-user collaboration is introduced, this becomes required.
    #[serde(default)]
    requester_id: Option<String>,
}

/// The PunkGo kernel — single-writer append-only event system.
///
/// The kernel is the **committer** (whitepaper §2): a structural role that
/// provides a single linearization point for actions. It is not a judge.
///
/// Use [`Kernel::bootstrap`] to initialize from a state directory, then
/// [`Kernel::handle_request`] to process IPC requests.
pub struct Kernel {
    state_store: StateStore,
    energy_ledger: EnergyLedger,
    event_log: EventLog,
    audit_log: AuditLog,
    actor_store: ActorStore,
    envelope_store: EnvelopeStore,
    stellar_config: StellarConfig,
}

impl Kernel {
    /// Initialize the kernel from a state directory.
    ///
    /// Creates the SQLite database, loads stellar config, and prepares the root actor.
    pub async fn bootstrap(config: &KernelConfig) -> KernelResult<Self> {
        let state_store = StateStore::bootstrap(&config.state_dir).await?;
        let energy_ledger = EnergyLedger::new(state_store.pool());
        let event_log = EventLog::new(state_store.pool());
        let audit_log = AuditLog::new(state_store.pool(), "punkgo/kernel");
        let actor_store = ActorStore::new(state_store.pool());
        let envelope_store = EnvelopeStore::new(state_store.pool());

        // Phase 2: Load stellar configuration (PIP-001 §1).
        let stellar_config_path = config.state_dir.join("stellar.toml");
        let stellar_config = load_stellar_config(&stellar_config_path)?;
        info!(
            energy_per_tick = stellar_config.effective_energy_per_tick(),
            int8_tops = stellar_config.int8_tops,
            tick_interval_ms = stellar_config.tick_interval_ms,
            "stellar configuration loaded"
        );

        Ok(Self {
            state_store,
            energy_ledger,
            event_log,
            audit_log,
            actor_store,
            envelope_store,
            stellar_config,
        })
    }

    /// Returns a reference to the stellar configuration for external use
    /// (e.g., energy producer startup).
    pub fn stellar_config(&self) -> &StellarConfig {
        &self.stellar_config
    }

    /// Returns a clone of the energy ledger for external use (e.g., energy producer).
    pub fn energy_ledger(&self) -> &EnergyLedger {
        &self.energy_ledger
    }

    /// Returns a clone of the actor store for external use (e.g., energy producer).
    pub fn actor_store(&self) -> &ActorStore {
        &self.actor_store
    }

    /// Returns a reference to the envelope store for external use.
    pub fn envelope_store(&self) -> &EnvelopeStore {
        &self.envelope_store
    }

    /// Returns the SQLite pool for external use (e.g., energy producer transaction).
    pub fn pool(&self) -> sqlx::SqlitePool {
        self.state_store.pool()
    }

    /// Handle an incoming IPC request — dispatches to quote, submit, or read.
    pub async fn handle_request(&self, req: RequestEnvelope) -> ResponseEnvelope {
        let request_id = req.request_id.clone();
        info!(
            request_id = %request_id,
            request_type = ?req.request_type,
            "received request"
        );
        match self.dispatch(req).await {
            Ok(payload) => {
                info!(request_id = %request_id, "request completed");
                ResponseEnvelope::ok(request_id, payload)
            }
            Err(err) => {
                warn!(error = %err, "request failed");
                ResponseEnvelope::err_structured(request_id, &err)
            }
        }
    }

    async fn dispatch(&self, req: RequestEnvelope) -> KernelResult<Value> {
        match req.request_type {
            RequestType::Quote => {
                let action: Action = serde_json::from_value(req.payload)?;
                validate_action(&action)?;
                let cost = quote_cost(&action);
                Ok(json!({ "cost": cost }))
            }
            RequestType::Submit => {
                let action: Action = serde_json::from_value(req.payload)?;
                let receipt = self.submit_action(action).await?;
                Ok(serde_json::to_value(receipt)?)
            }
            RequestType::Read => {
                let query: ReadQuery = serde_json::from_value(req.payload)?;
                self.read_query(query).await
            }
        }
    }

    async fn submit_action(&self, action: Action) -> KernelResult<SubmitReceipt> {
        // Step 1: VALIDATE — policy + actor existence + frozen check
        validate_action(&action)?;
        if !self.state_store.actor_exists(&action.actor_id).await? {
            return Err(KernelError::ActorNotFound(action.actor_id.clone()));
        }

        // Phase 1: check frozen status via actors table.
        // Phase 3: check writable boundary (PIP-001 §8/§9).
        // Phase 4a: check lineage activity (PIP-001 §7) + lifecycle ops.
        if let Some(actor) = self.actor_store.get(&action.actor_id).await? {
            if actor.status == punkgo_core::actor::ActorStatus::Frozen
                && action.action_type.is_state_changing()
            {
                return Err(KernelError::ActorFrozen(format!(
                    "actor {} is frozen and cannot perform state-changing actions",
                    action.actor_id
                )));
            }
            // Phase 3: Boundary enforcement — check writable_targets.
            check_writable_boundary(&actor, &action.target, &action.action_type)?;

            // Phase 4a: Check lineage activity for agents (PIP-001 §7).
            // Agents need their entire creation chain to be active.
            if actor.actor_type == punkgo_core::actor::ActorType::Agent
                && action.action_type.is_state_changing()
            {
                lifecycle::check_lineage_active(&self.actor_store, &actor.lineage).await?;
            }

            // Phase 4b: Authorization check (PIP-001 §5/§6/§11).
            // Agents performing state-changing actions need an active envelope.
            if action.action_type.is_state_changing() {
                let envelope = self
                    .envelope_store
                    .get_active_for_actor(&action.actor_id)
                    .await?;

                // Lazy expiry check (PIP-001 §11 checkpoint): if envelope exists but is time-expired, mark it.
                let envelope = if let Some(env) = envelope {
                    if consent::is_envelope_expired(&env, now_millis_u64()) {
                        self.envelope_store
                            .set_status(&env.envelope_id, &consent::EnvelopeStatus::Expired)
                            .await?;
                        None
                    } else {
                        Some(env)
                    }
                } else {
                    None
                };

                let auth_mode = consent::check_authorization(&actor, envelope.as_ref())?;

                // For ManOnTheLoop, also verify envelope covers this specific action.
                if auth_mode == AuthorizationMode::ManOnTheLoop
                    && let Some(ref env) = envelope
                {
                    consent::check_envelope_covers(
                        env,
                        &action.target,
                        action.action_type.as_str(),
                    )?;

                    // PIP-001 §11e: Lazy-expire timed-out holds before checking new triggers.
                    if !env.hold_on.is_empty() {
                        if let Some(timeout_secs) = env.hold_timeout_secs {
                            if timeout_secs > 0 {
                                self.expire_timed_out_holds(&env.envelope_id, timeout_secs)
                                    .await?;
                            }
                        }
                    }

                    // PIP-001 §11b: Check hold_on rules — if triggered, create a hold_request
                    // event and reserve energy before execution.
                    // Skip the hold check if this action was already approved by a Human
                    // (re-execution after approve; PIP-001 §11d).
                    let hold_approved = action
                        .payload
                        .get("_hold_approved")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !hold_approved
                        && consent::check_hold_trigger(
                            &env.hold_on,
                            &action.target,
                            action.action_type.as_str(),
                        )
                    {
                        let hold_id = Uuid::new_v4().to_string();

                        // PIP-001 §11c: Commit = reserve energy (gasLimit model).
                        // Reserves energy upfront so agent can't overspend while hold is pending.
                        let reserved_cost = quote_cost(&action) as i64;

                        let hold_payload = json!({
                            "hold_id": &hold_id,
                            "agent_id": &action.actor_id,
                            "trigger": {
                                "target": &action.target,
                                "action_type": action.action_type.as_str()
                            },
                            "pending_action": {
                                "target": &action.target,
                                "action_type": action.action_type.as_str(),
                                "payload": &action.payload
                            },
                            "reserved_cost": reserved_cost,
                            "triggered_at": now_millis_string()
                        });

                        // Write hold_request event into history (PIP-001 §11b, whitepaper §3 inv 6).
                        let hold_action = Action {
                            actor_id: action.actor_id.clone(),
                            action_type: ActionType::Mutate,
                            target: format!("ledger/hold/{hold_id}"),
                            payload: hold_payload.clone(),
                            timestamp: None,
                        };
                        let mut hold_event = EventRecord {
                            id: Uuid::new_v4().to_string(),
                            log_index: 0,
                            event_hash: String::new(),
                            actor_id: action.actor_id.clone(),
                            action_type: "hold_request".to_string(),
                            target: format!("ledger/hold/{hold_id}"),
                            payload: hold_payload.clone(),
                            payload_hash: payload_hash_hex(&hold_action)?,
                            artifact_hash: None,
                            reserved_energy: reserved_cost,
                            settled_energy: 0,
                            timestamp: now_millis_string(),
                        };

                        // Atomic: reserve + hold_request + event in one transaction (PIP-001 §11b/§11c).
                        let pool = self.state_store.pool();
                        let mut tx = pool.begin().await?;
                        if reserved_cost > 0 {
                            self.energy_ledger
                                .reserve_in_tx(&mut tx, &action.actor_id, reserved_cost)
                                .await?;
                        }
                        self.event_log
                            .append_in_tx(&mut tx, &mut hold_event)
                            .await?;
                        self.envelope_store
                            .create_hold_request_in_tx(
                                &mut tx,
                                &NewHoldRequest {
                                    hold_id: &hold_id,
                                    envelope_id: &env.envelope_id,
                                    agent_id: &action.actor_id,
                                    trigger_target: &action.target,
                                    trigger_action: action.action_type.as_str(),
                                    pending_payload: &json!({
                                        "target": &action.target,
                                        "action_type": action.action_type.as_str(),
                                        "payload": &action.payload,
                                        "reserved_cost": reserved_cost
                                    }),
                                },
                            )
                            .await?;
                        // Envelope stays Active — agent can continue submitting (PIP-001 §11b).

                        // Audit trail — atomic with event (whitepaper §3 invariant 5).
                        let log_index = hold_event.log_index as u64;
                        self.audit_log
                            .append_leaf_in_tx(&mut tx, log_index, &hold_event.event_hash)
                            .await
                            .map_err(|e| KernelError::Audit(e.to_string()))?;
                        // Checkpoint generated lazily on read, not here.
                        tx.commit().await?;

                        return Err(KernelError::HoldTriggered {
                            hold_id,
                            agent_id: action.actor_id.clone(),
                        });
                    }
                }
            }
        }

        // PIP-001 §11d: Check for hold_response (approve/reject) operations.
        // Pattern: mutate "ledger/hold/<hold_id>" with payload { decision, instruction? }
        let hold_response = if matches!(action.action_type, ActionType::Mutate) {
            parse_hold_response(&action.target, &action.payload)
        } else {
            None
        };

        // Validate and execute hold_response before entering the normal pipeline.
        if let Some((ref hold_id, ref decision, ref instruction)) = hold_response {
            return self
                .execute_hold_response(&action, hold_id, decision, instruction.as_deref())
                .await;
        }

        // Phase 4a: Check for lifecycle operations (freeze/unfreeze/terminate).
        let lifecycle_op = if matches!(action.action_type, ActionType::Mutate) {
            lifecycle::parse_lifecycle_op(&action.target, &action.payload)
        } else {
            None
        };

        // Validate lifecycle authorization before proceeding.
        if let Some((ref target_actor_id, ref op)) = lifecycle_op {
            let initiator = self
                .actor_store
                .get(&action.actor_id)
                .await?
                .ok_or_else(|| KernelError::ActorNotFound(action.actor_id.clone()))?;
            let target_actor = self
                .actor_store
                .get(target_actor_id)
                .await?
                .ok_or_else(|| KernelError::ActorNotFound(target_actor_id.clone()))?;
            lifecycle::validate_lifecycle_authorization(&initiator, &target_actor, op).await?;
        }

        let create_actor_spec = parse_create_actor_spec(&action, &self.actor_store).await?;
        let create_envelope_spec =
            parse_create_envelope_spec(&action, &self.envelope_store).await?;
        let policy_version = parse_policy_version(&action);

        // Step 2: QUOTE + Step 3: RESERVE
        // PIP-001 §11d: Hold-approved actions already have energy reserved at trigger time.
        // The sentinel `_hold_reserved_cost` carries the pre-reserved amount so we skip
        // double-charging. We create a phantom EnergyReservation so settle correctly
        // reduces reserved_energy without calling reserve() again.
        let (reserved_cost, reservation) = if let Some(hold_reserved) = action
            .payload
            .get("_hold_reserved_cost")
            .and_then(|v| v.as_i64())
        {
            let phantom = if hold_reserved > 0 {
                Some(EnergyReservation {
                    actor_id: action.actor_id.clone(),
                    reserved: hold_reserved,
                })
            } else {
                None
            };
            (hold_reserved, phantom)
        } else {
            // All actions pay quote_cost (action_cost + append_cost).
            // For observe: action_cost=0 but append_cost >= 1 (Landauer).
            let cost = quote_cost(&action) as i64;
            let res = if cost > 0 {
                Some(self.energy_ledger.reserve(&action.actor_id, cost).await?)
            } else {
                None
            };
            (cost, res)
        };

        // Step 4: VALIDATE EXECUTE PAYLOAD (PIP-002 §2 — for Execute actions only)
        // The kernel does not execute anything. It validates the actor-submitted
        // result and records it. Actor executes, kernel records.
        let artifact_hash = if matches!(action.action_type, ActionType::Execute) {
            Some(Self::validate_execute_payload(&action.payload)?)
        } else {
            None
        };

        // Step 5: SETTLE
        // For PIP-002: settled cost equals reserved cost (no post-execution IO adjustment).
        let settled_cost = reserved_cost;

        // Step 6: APPEND
        let mut event = EventRecord {
            id: Uuid::new_v4().to_string(),
            log_index: 0,
            event_hash: String::new(),
            actor_id: action.actor_id.clone(),
            action_type: action.action_type.as_str().to_string(),
            target: action.target.clone(),
            payload: action.payload.clone(),
            payload_hash: payload_hash_hex(&action)?,
            artifact_hash: artifact_hash.clone(),
            reserved_energy: reserved_cost,
            settled_energy: settled_cost,
            timestamp: now_millis_string(),
        };
        self.finalize_energy_and_event(
            reservation.as_ref(),
            settled_cost,
            create_actor_spec.as_ref(),
            create_envelope_spec.as_ref(),
            &mut event,
        )
        .await?;

        // Step 7: POST-COMMIT — envelope budget consumption + checkpoints + policy + lifecycle
        if let Some(ref version) = policy_version {
            if let Err(err) = self.state_store.set_policy_version(version).await {
                warn!(error = %err, "failed to record policy version after commit");
            } else {
                info!(policy_version = %version, event_id = %event.id, "policy version updated");
            }
        }

        // Phase 4b: Post-commit envelope budget consumption and checkpoint (PIP-001 §11).
        if action.action_type.is_state_changing()
            && settled_cost > 0
            && let Ok(Some(actor)) = self.actor_store.get(&action.actor_id).await
            && actor.actor_type == ActorType::Agent
            && let Ok(Some(envelope)) = self
                .envelope_store
                .get_active_for_actor(&action.actor_id)
                .await
        {
            // Check checkpoint BEFORE consuming (so we use pre-consume state).
            let checkpoint = consent::check_checkpoint(&envelope, settled_cost);

            // Consume budget.
            let pool = self.state_store.pool();
            let mut tx = pool.begin().await?;
            match self
                .envelope_store
                .consume_budget_in_tx(&mut tx, &envelope.envelope_id, settled_cost)
                .await
            {
                Ok(_new_consumed) => {
                    // Handle checkpoint levels (PIP-001 §11).
                    match checkpoint {
                        Some(CheckpointLevel::Halt) => {
                            self.envelope_store
                                .set_status_in_tx(
                                    &mut tx,
                                    &envelope.envelope_id,
                                    &consent::EnvelopeStatus::Expired,
                                )
                                .await?;
                            info!(
                                envelope_id = %envelope.envelope_id,
                                actor_id = %action.actor_id,
                                "checkpoint: HALT — envelope budget exhausted"
                            );
                        }
                        Some(CheckpointLevel::Report) => {
                            info!(
                                envelope_id = %envelope.envelope_id,
                                actor_id = %action.actor_id,
                                budget = envelope.budget,
                                consumed = envelope.budget_consumed + settled_cost,
                                "checkpoint: REPORT — summary logged"
                            );
                        }
                        None => {}
                    }
                    tx.commit().await?;
                }
                Err(err) => {
                    warn!(
                        error = %err,
                        "envelope budget consumption failed (event already committed)"
                    );
                    // Transaction is automatically rolled back on drop.
                }
            }
        }

        // Phase 4a: Execute lifecycle operation after event is committed.
        if let Some((ref target_actor_id, ref op)) = lifecycle_op {
            let pool = self.state_store.pool();
            match op {
                punkgo_core::actor::LifecycleOp::Freeze { .. } => {
                    match lifecycle::execute_freeze(&self.actor_store, &pool, target_actor_id).await
                    {
                        Ok(frozen_ids) => {
                            info!(
                                target = %target_actor_id,
                                cascade_count = frozen_ids.len(),
                                "lifecycle: freeze executed"
                            );
                        }
                        Err(err) => {
                            warn!(error = %err, "lifecycle: freeze failed (event already committed)");
                        }
                    }
                }
                punkgo_core::actor::LifecycleOp::Unfreeze => {
                    match lifecycle::execute_unfreeze(&self.actor_store, &pool, target_actor_id)
                        .await
                    {
                        Ok(()) => {
                            info!(target = %target_actor_id, "lifecycle: unfreeze executed");
                        }
                        Err(err) => {
                            warn!(error = %err, "lifecycle: unfreeze failed (event already committed)");
                        }
                    }
                }
                punkgo_core::actor::LifecycleOp::Terminate { .. } => {
                    // Terminate sets the actor to frozen permanently.
                    // Orphan handling is a future enhancement — for now, children are cascade-frozen.
                    match lifecycle::execute_freeze(&self.actor_store, &pool, target_actor_id).await
                    {
                        Ok(frozen_ids) => {
                            info!(
                                target = %target_actor_id,
                                cascade_count = frozen_ids.len(),
                                "lifecycle: terminate executed (cascade frozen)"
                            );
                        }
                        Err(err) => {
                            warn!(error = %err, "lifecycle: terminate failed (event already committed)");
                        }
                    }
                }
                punkgo_core::actor::LifecycleOp::UpdateEnergyShare { energy_share } => {
                    match lifecycle::execute_update_energy_share(
                        &self.actor_store,
                        &pool,
                        target_actor_id,
                        *energy_share,
                    )
                    .await
                    {
                        Ok(()) => {
                            info!(
                                target = %target_actor_id,
                                energy_share,
                                "lifecycle: energy_share updated"
                            );
                        }
                        Err(err) => {
                            warn!(error = %err, "lifecycle: update_energy_share failed (event already committed)");
                        }
                    }
                }
            }
        }

        Ok(SubmitReceipt {
            event_id: event.id,
            log_index: event.log_index,
            event_hash: event.event_hash,
            reserved_cost,
            settled_cost,
            artifact_hash,
        })
    }

    async fn finalize_energy_and_event(
        &self,
        reservation: Option<&EnergyReservation>,
        settled_cost: i64,
        create_actor: Option<&CreateActorSpec>,
        create_envelope: Option<&(String, EnvelopeSpec, Option<String>)>,
        event: &mut EventRecord,
    ) -> KernelResult<()> {
        let pool = self.state_store.pool();
        let mut tx = pool.begin().await?;

        // Energy settlement (if reservation exists).
        if let Some(res) = reservation {
            self.energy_ledger
                .settle_in_tx(&mut tx, &res.actor_id, res.reserved, settled_cost)
                .await?;
        }

        // Actor creation (if applicable).
        if let Some(spec) = create_actor {
            self.energy_ledger
                .create_actor_in_tx(&mut tx, &spec.actor_id, spec.energy_balance)
                .await?;
            self.actor_store.create_in_tx(&mut tx, spec).await?;
            info!(
                created_actor = %spec.actor_id,
                actor_type = spec.actor_type.as_str(),
                energy_balance = spec.energy_balance,
                "actor created in transaction"
            );
        }

        // Envelope creation (if applicable, whitepaper §3 invariant 6).
        if let Some((envelope_id, spec, parent_id)) = create_envelope {
            self.envelope_store
                .create_in_tx(&mut tx, envelope_id, spec, parent_id.as_deref())
                .await?;
            info!(
                envelope_id = %envelope_id,
                actor_id = %spec.actor_id,
                grantor_id = %spec.grantor_id,
                budget = spec.budget,
                "envelope created in transaction"
            );
        }

        // Event append.
        self.event_log.append_in_tx(&mut tx, event).await?;

        // Audit trail update — atomic with event (whitepaper §3 invariant 5).
        let log_index = event.log_index as u64;
        self.audit_log
            .append_leaf_in_tx(&mut tx, log_index, &event.event_hash)
            .await
            .map_err(|e| KernelError::Audit(e.to_string()))?;

        // Checkpoint is NOT generated on every event. It is a derived artifact
        // (tree root computed from leaf hashes) and can be generated on-demand
        // when queried (receipt, show, verify) or via explicit checkpoint command.
        // This keeps the write path fast and lock-free for concurrent access.
        tx.commit().await?;
        info!(event_id = %event.id, log_index = event.log_index, "event committed");
        Ok(())
    }

    /// PIP-002 §2+§3: Validate execute action payload.
    ///
    /// The kernel MUST reject an execute submission that is missing any required
    /// field or uses an invalid OID format. Returns the artifact_hash on success.
    fn validate_execute_payload(payload: &Value) -> KernelResult<String> {
        Self::require_valid_oid(payload, "input_oid")?;
        Self::require_valid_oid(payload, "output_oid")?;

        if payload.get("exit_code").and_then(|v| v.as_i64()).is_none() {
            return Err(KernelError::ExecutePayloadInvalid(
                "missing or invalid exit_code (must be integer)".to_string(),
            ));
        }

        let artifact_hash = Self::require_valid_oid(payload, "artifact_hash")?;
        Ok(artifact_hash)
    }

    /// Validate that a payload field exists and matches OID format: `sha256:<64 hex chars>`.
    fn require_valid_oid(payload: &Value, field: &str) -> KernelResult<String> {
        let val = payload
            .get(field)
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::ExecutePayloadInvalid(format!("missing {field}")))?;

        if !val.starts_with("sha256:") || val.len() != 71 {
            return Err(KernelError::ExecutePayloadInvalid(format!(
                "{field} must be sha256:<64 hex chars>, got: {val}"
            )));
        }
        let hex_part = &val[7..];
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(KernelError::ExecutePayloadInvalid(format!(
                "{field} contains non-hex characters: {val}"
            )));
        }

        Ok(val.to_string())
    }

    async fn read_query(&self, query: ReadQuery) -> KernelResult<Value> {
        // Visibility boundary gate for read queries.
        // Currently all-transparent. When multi-user collaboration is
        // introduced, check_read_access will enforce readable scope per
        // requester. See PIP-001 §8 (writable_targets).
        let requester = query.requester_id.as_deref().unwrap_or("anonymous");
        check_read_access(requester, &query.kind)?;

        match query.kind.as_str() {
            "health" => Ok(json!({ "status": "ok" })),
            "actor_energy" => {
                let actor_id = query.actor_id.ok_or_else(|| {
                    KernelError::InvalidRequest("actor_id is required for actor_energy".to_string())
                })?;
                let (energy_balance, reserved_energy) =
                    self.energy_ledger.balance_view(&actor_id).await?;
                Ok(json!({
                    "actor_id": actor_id,
                    "energy_balance": energy_balance,
                    "reserved_energy": reserved_energy
                }))
            }
            "events" => {
                let limit = query.limit.unwrap_or(20).clamp(1, 500);
                let events = self
                    .event_log
                    .query(
                        query.actor_id.as_deref(),
                        query.before_index,
                        query.after_index,
                        limit,
                    )
                    .await?;
                // Pagination metadata: smallest log_index in result set.
                // Client passes this as `before_index` to fetch the next page.
                let next_cursor = events.last().map(|e| e.log_index);
                let has_more = events.len() as i64 == limit;
                Ok(json!({
                    "events": events,
                    "has_more": has_more,
                    "next_cursor": next_cursor
                }))
            }
            "stats" => {
                let event_count = self.event_log.count().await?;
                Ok(json!({ "event_count": event_count }))
            }
            "snapshot" => {
                // Legacy: snapshot is superseded by audit checkpoint.
                // Return audit checkpoint data for backward compatibility.
                let event_count = self.event_log.count().await?;
                self.audit_log
                    .ensure_checkpoint(event_count as u64)
                    .await
                    .map_err(|e| KernelError::Audit(e.to_string()))?;
                let cp = self
                    .audit_log
                    .latest_checkpoint()
                    .await
                    .map_err(|e| KernelError::Audit(e.to_string()))?;
                Ok(json!({
                    "event_count": event_count,
                    "snapshot_hash": cp.root_hash,
                    "generated_at": cp.created_at
                }))
            }
            "paths" => {
                let paths = self.state_store.paths();
                Ok(json!({
                    "root": paths.root.display().to_string(),
                    "workspace_root": paths.workspace_root.display().to_string(),
                    "quarantine_root": paths.quarantine_root.display().to_string(),
                    "db_path": paths.db_path.display().to_string()
                }))
            }
            "audit_checkpoint" => {
                let event_count = self.event_log.count().await?;
                self.audit_log
                    .ensure_checkpoint(event_count as u64)
                    .await
                    .map_err(|e| KernelError::Audit(e.to_string()))?;
                let cp = self
                    .audit_log
                    .latest_checkpoint()
                    .await
                    .map_err(|e| KernelError::Audit(e.to_string()))?;
                Ok(serde_json::to_value(cp)?)
            }
            "audit_inclusion_proof" => {
                let log_index = query.log_index.ok_or_else(|| {
                    KernelError::InvalidRequest(
                        "log_index is required for audit_inclusion_proof".to_string(),
                    )
                })? as u64;
                let tree_size = match query.tree_size {
                    Some(s) => s as u64,
                    None => {
                        // Ensure checkpoint is current before deriving tree_size.
                        let event_count = self.event_log.count().await? as u64;
                        self.audit_log
                            .ensure_checkpoint(event_count)
                            .await
                            .map_err(|e| KernelError::Audit(e.to_string()))?;
                        self.audit_log
                            .tree_size()
                            .await
                            .map_err(|e| KernelError::Audit(e.to_string()))?
                    }
                };
                let proof = self
                    .audit_log
                    .inclusion_proof(log_index, tree_size)
                    .await
                    .map_err(|e| KernelError::Audit(e.to_string()))?;
                Ok(json!({
                    "log_index": log_index,
                    "tree_size": tree_size,
                    "proof": proof
                }))
            }
            "audit_consistency_proof" => {
                let old_size = query.old_size.ok_or_else(|| {
                    KernelError::InvalidRequest(
                        "old_size is required for audit_consistency_proof".to_string(),
                    )
                })? as u64;
                let new_size = query.tree_size.ok_or_else(|| {
                    KernelError::InvalidRequest(
                        "tree_size is required for audit_consistency_proof".to_string(),
                    )
                })? as u64;
                let proof = self
                    .audit_log
                    .consistency_proof(old_size, new_size)
                    .await
                    .map_err(|e| KernelError::Audit(e.to_string()))?;
                Ok(json!({
                    "old_size": old_size,
                    "new_size": new_size,
                    "proof": proof
                }))
            }
            // Phase 2: stellar configuration read query.
            "stellar_info" => {
                let config = &self.stellar_config;
                Ok(json!({
                    "gpu_model": config.gpu_model,
                    "cpu_model": config.cpu_model,
                    "int8_tops": config.int8_tops,
                    "energy_per_tick": config.effective_energy_per_tick(),
                    "tick_interval_ms": config.tick_interval_ms,
                    "luminosity_source": config.luminosity_source
                }))
            }
            // Phase 4b: envelope_info read query — retrieve active envelope for an actor.
            "envelope_info" => {
                let actor_id = query.actor_id.ok_or_else(|| {
                    KernelError::InvalidRequest(
                        "actor_id is required for envelope_info".to_string(),
                    )
                })?;
                let envelope = self.envelope_store.get_active_for_actor(&actor_id).await?;
                match envelope {
                    Some(record) => Ok(serde_json::to_value(record)?),
                    None => Ok(json!({ "actor_id": actor_id, "envelope": null })),
                }
            }
            // Phase 1: actor_info read query — retrieve actor record.
            "actor_info" => {
                let actor_id = query.actor_id.ok_or_else(|| {
                    KernelError::InvalidRequest("actor_id is required for actor_info".to_string())
                })?;
                let actor = self.actor_store.get(&actor_id).await?;
                match actor {
                    Some(record) => Ok(serde_json::to_value(record)?),
                    None => Err(KernelError::ActorNotFound(actor_id)),
                }
            }
            // PIP-001 §11: hold_info — retrieve a single hold_request by hold_id.
            "hold_info" => {
                let hold_id = query.hold_id.ok_or_else(|| {
                    KernelError::InvalidRequest("hold_id is required for hold_info".to_string())
                })?;
                let hold = self.envelope_store.get_hold_request(&hold_id).await?;
                match hold {
                    Some(record) => Ok(record),
                    None => Err(KernelError::InvalidRequest(format!(
                        "hold_request not found: {hold_id}"
                    ))),
                }
            }
            // PIP-001 §11: holds_pending — list pending hold_requests, optionally filtered by agent.
            "holds_pending" => {
                let holds = self
                    .envelope_store
                    .list_pending_holds(query.actor_id.as_deref())
                    .await?;
                Ok(json!({ "holds": holds }))
            }
            other => Err(KernelError::InvalidRequest(format!(
                "unsupported read query kind: {other}"
            ))),
        }
    }
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}

fn now_millis_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// If the action is a `system/policy` Create, extract the policy version string.
/// Returns `None` for all other actions.
fn parse_policy_version(action: &Action) -> Option<String> {
    if !matches!(action.action_type, ActionType::Create) || action.target != "system/policy" {
        return None;
    }
    action
        .payload
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Phase 1: Parse actor creation spec from a `ledger/actor` Create action.
///
/// Backward compatible: old format `{actor_id, energy_balance}` is supported
/// by defaulting to actor_type=agent, purpose="legacy", writable_targets=[].
///
/// New format adds: actor_type, purpose, writable_targets, energy_share.
async fn parse_create_actor_spec(
    action: &Action,
    actor_store: &ActorStore,
) -> KernelResult<Option<CreateActorSpec>> {
    if !matches!(action.action_type, ActionType::Create) || action.target != "ledger/actor" {
        return Ok(None);
    }

    let payload = action.payload.as_object().ok_or_else(|| {
        KernelError::InvalidRequest("seed actor payload must be an object".to_string())
    })?;

    // Required: actor_id (or derive from purpose)
    let explicit_id = payload.get("actor_id").and_then(Value::as_str);
    let purpose = payload
        .get("purpose")
        .and_then(Value::as_str)
        .unwrap_or("legacy");
    let energy_balance = payload
        .get("energy_balance")
        .and_then(Value::as_i64)
        .unwrap_or(1000);

    // Actor type: default to "agent" for backward compat
    let actor_type_str = payload
        .get("actor_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let actor_type = ActorType::parse(actor_type_str).ok_or_else(|| {
        KernelError::InvalidRequest(format!("invalid actor_type: {actor_type_str}"))
    })?;

    // Derive ID if not explicitly provided
    let actor_id = if let Some(id) = explicit_id {
        id.to_string()
    } else {
        if purpose == "legacy" {
            return Err(KernelError::InvalidRequest(
                "actor_id or purpose is required to create an actor".to_string(),
            ));
        }
        let seq = actor_store.next_sequence(&action.actor_id, purpose).await?;
        derive_agent_id(&action.actor_id, purpose, seq)
    };

    // Build lineage from creator (PIP-001 §7) + enforce §5 (only humans create agents)
    let creator_record = actor_store.get(&action.actor_id).await?;
    let (creator_type, creator_lineage) = match &creator_record {
        Some(record) => (record.actor_type.clone(), record.lineage.clone()),
        // Fallback for legacy actors not yet in actors table
        None => (ActorType::Human, vec![]),
    };

    // PIP-001 §5: Only Humans can create Agents. Reject Agent as creator.
    if creator_type == ActorType::Agent && actor_type == ActorType::Agent {
        return Err(KernelError::PolicyViolation(
            "agents cannot create agents — creation right belongs to humans (PIP-001 §5)"
                .to_string(),
        ));
    }

    let lineage = build_lineage(&creator_type, &action.actor_id, &creator_lineage);

    // Writable targets: parse from payload or default to empty
    let writable_targets: Vec<WritableTarget> = if let Some(targets_val) =
        payload.get("writable_targets")
    {
        serde_json::from_value(targets_val.clone())
            .map_err(|e| KernelError::InvalidRequest(format!("invalid writable_targets: {e}")))?
    } else {
        vec![]
    };

    // Phase 3 PIP-001 §11: validate child targets are subset of creator's boundary.
    if !writable_targets.is_empty()
        && let Some(ref creator) = creator_record
    {
        validate_child_targets(creator, &writable_targets)?;
    }

    // Energy share: default 0.0
    let energy_share = payload
        .get("energy_share")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    let reduction_policy = payload
        .get("reduction_policy")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_string();

    Ok(Some(CreateActorSpec {
        actor_id,
        actor_type,
        creator_id: action.actor_id.clone(),
        lineage,
        purpose: Some(purpose.to_string()),
        writable_targets,
        energy_balance,
        energy_share,
        reduction_policy,
    }))
}

/// Phase 4b: Parse envelope creation spec from a `ledger/envelope` Create action.
///
/// Returns (envelope_id, EnvelopeSpec, parent_envelope_id) if applicable.
/// The grantor is always the action's actor_id (the human or parent agent granting
/// the envelope).
///
/// PIP-001 §11: If the grantor is an agent, their active envelope is used as the parent
/// and reduction validation is performed.
async fn parse_create_envelope_spec(
    action: &Action,
    envelope_store: &EnvelopeStore,
) -> KernelResult<Option<(String, EnvelopeSpec, Option<String>)>> {
    if !matches!(action.action_type, ActionType::Create) || action.target != "ledger/envelope" {
        return Ok(None);
    }

    let payload = action.payload.as_object().ok_or_else(|| {
        KernelError::InvalidRequest("envelope payload must be an object".to_string())
    })?;

    let actor_id = payload
        .get("actor_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            KernelError::InvalidRequest("actor_id is required to create an envelope".to_string())
        })?
        .to_string();

    let budget = payload
        .get("budget")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            KernelError::InvalidRequest("budget is required to create an envelope".to_string())
        })?;

    if budget <= 0 {
        return Err(KernelError::InvalidRequest(
            "envelope budget must be positive".to_string(),
        ));
    }

    let targets: Vec<String> = if let Some(targets_val) = payload.get("targets") {
        serde_json::from_value(targets_val.clone())
            .map_err(|e| KernelError::InvalidRequest(format!("invalid envelope targets: {e}")))?
    } else {
        return Err(KernelError::InvalidRequest(
            "targets are required to create an envelope".to_string(),
        ));
    };

    let actions: Vec<String> = if let Some(actions_val) = payload.get("actions") {
        serde_json::from_value(actions_val.clone())
            .map_err(|e| KernelError::InvalidRequest(format!("invalid envelope actions: {e}")))?
    } else {
        return Err(KernelError::InvalidRequest(
            "actions are required to create an envelope".to_string(),
        ));
    };

    let duration_secs = payload.get("duration_secs").and_then(Value::as_i64);
    let report_every = payload.get("report_every").and_then(Value::as_i64);
    let hold_timeout_secs = payload.get("hold_timeout_secs").and_then(Value::as_i64);

    // PIP-001 §11a: Parse hold_on rules.
    let hold_on: Vec<HoldRule> = if let Some(hold_on_val) = payload.get("hold_on") {
        serde_json::from_value(hold_on_val.clone())
            .map_err(|e| KernelError::InvalidRequest(format!("invalid hold_on rules: {e}")))?
    } else {
        vec![]
    };

    let spec = EnvelopeSpec {
        actor_id,
        grantor_id: action.actor_id.clone(),
        budget,
        targets,
        actions,
        duration_secs,
        report_every,
        hold_on,
        hold_timeout_secs,
    };

    // PIP-001 §11: If grantor is an agent with an active envelope, validate reduction.
    let parent_envelope_id = if let Some(grantor_envelope) = envelope_store
        .get_active_for_actor(&action.actor_id)
        .await?
    {
        consent::validate_envelope_reduction(&grantor_envelope, &spec)?;
        Some(grantor_envelope.envelope_id)
    } else {
        None
    };

    let envelope_id = Uuid::new_v4().to_string();

    Ok(Some((envelope_id, spec, parent_envelope_id)))
}

// ---------------------------------------------------------------------------
// PIP-001 §11d: Hold response parsing and execution
// ---------------------------------------------------------------------------

/// Parse a `ledger/hold/<hold_id>` mutate target into (hold_id, decision, instruction).
///
/// Returns `None` for any other target.
fn parse_hold_response(target: &str, payload: &Value) -> Option<(String, String, Option<String>)> {
    // Pattern: "ledger/hold/<hold_id>"
    let rest = target.strip_prefix("ledger/hold/")?;
    // Ensure there is a non-empty hold_id segment and nothing after it.
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    let hold_id = rest.to_string();

    let decision = payload.get("decision").and_then(Value::as_str)?.to_string();

    // Validate decision value.
    if decision != "approve" && decision != "reject" {
        return None;
    }

    let instruction = payload
        .get("instruction")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    Some((hold_id, decision, instruction))
}

impl Kernel {
    /// PIP-001 §11d: Execute a hold_response (approve or reject).
    ///
    /// Validates that:
    /// 1. The caller is a Human actor (whitepaper §2: Human sovereignty).
    /// 2. The hold_request exists and is still pending.
    /// 3. The hold envelope belongs to an Agent actor.
    ///
    /// On approve: re-executes the pending action and writes a `hold_response` event.
    /// On reject: discards the pending action and writes a `hold_response` event.
    async fn execute_hold_response(
        &self,
        human_action: &Action,
        hold_id: &str,
        decision: &str,
        instruction: Option<&str>,
    ) -> KernelResult<SubmitReceipt> {
        // --- Validate caller is Human ---
        let caller = self
            .actor_store
            .get(&human_action.actor_id)
            .await?
            .ok_or_else(|| KernelError::ActorNotFound(human_action.actor_id.clone()))?;

        if caller.actor_type != punkgo_core::actor::ActorType::Human {
            return Err(KernelError::PolicyViolation(format!(
                "only Human actors can resolve holds; caller {} is {:?}",
                human_action.actor_id, caller.actor_type
            )));
        }

        // --- Load hold_request ---
        let hold_record = self
            .envelope_store
            .get_hold_request(hold_id)
            .await?
            .ok_or_else(|| {
                KernelError::PolicyViolation(format!("hold_request not found: {hold_id}"))
            })?;

        let _envelope_id = hold_record
            .get("envelope_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                KernelError::PolicyViolation("hold_request missing envelope_id".to_string())
            })?
            .to_string();

        let agent_id = hold_record
            .get("agent_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                KernelError::PolicyViolation("hold_request missing agent_id".to_string())
            })?
            .to_string();

        let pending_payload = hold_record
            .get("pending_payload")
            .cloned()
            .unwrap_or(Value::Null);

        let trigger_target = hold_record
            .get("trigger_target")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let trigger_action = hold_record
            .get("trigger_action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let status = hold_record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("");
        if status != "pending" {
            return Err(KernelError::PolicyViolation(format!(
                "hold_request {hold_id} is already resolved (status={status})"
            )));
        }

        // PIP-001 §11e: Check if this hold has already timed out.
        // If so, run the lazy expiry first — it will auto-reject the hold.
        let envelope_id_str = hold_record
            .get("envelope_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Ok(Some(env)) = self.envelope_store.get(&envelope_id_str).await {
            if let Some(timeout_secs) = env.hold_timeout_secs {
                if timeout_secs > 0 {
                    let triggered_at: u64 = hold_record
                        .get("triggered_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let now = now_millis_u64();
                    if now.saturating_sub(triggered_at) > (timeout_secs as u64) * 1000 {
                        // Expire this hold first, then return error.
                        self.expire_timed_out_holds(&envelope_id_str, timeout_secs)
                            .await?;
                        return Err(KernelError::PolicyViolation(format!(
                            "hold_request {hold_id} has timed out and was auto-rejected"
                        )));
                    }
                }
            }
        }

        // --- Build hold_response event payload (PIP-001 §11d) ---
        let response_payload = json!({
            "hold_id": hold_id,
            "agent_id": &agent_id,
            "decision": decision,
            "instruction": instruction,
            "trigger": {
                "target": &trigger_target,
                "action_type": &trigger_action
            },
            "resolved_by": &human_action.actor_id,
            "resolved_at": now_millis_string()
        });

        let response_action = Action {
            actor_id: human_action.actor_id.clone(),
            action_type: ActionType::Mutate,
            target: format!("ledger/hold/{hold_id}"),
            payload: response_payload.clone(),
            timestamp: None,
        };

        // --- Atomic transaction: resolve hold_request + settle energy + write event ---
        // Envelope stays Active throughout — no status change needed (PIP-001 §11b).
        let pool = self.state_store.pool();
        let mut tx = pool.begin().await?;

        self.envelope_store
            .resolve_hold_request_in_tx(&mut tx, hold_id, decision, instruction)
            .await?;

        // PIP-001 §11f: On reject, settle commitment cost (20% of reserved).
        // settle_in_tx(reserved=full, actual=commitment) releases the rest.
        let hold_reserved_cost = pending_payload
            .get("reserved_cost")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let commitment_cost = if decision == "reject" && hold_reserved_cost > 0 {
            let cost = ((hold_reserved_cost as f64) * 0.2).ceil() as i64;
            self.energy_ledger
                .settle_in_tx(&mut tx, &agent_id, hold_reserved_cost, cost)
                .await?;
            cost
        } else {
            0
        };

        let mut response_event = EventRecord {
            id: Uuid::new_v4().to_string(),
            log_index: 0,
            event_hash: String::new(),
            actor_id: human_action.actor_id.clone(),
            action_type: "hold_response".to_string(),
            target: format!("ledger/hold/{hold_id}"),
            payload: response_payload.clone(),
            payload_hash: payload_hash_hex(&response_action)?,
            artifact_hash: None,
            reserved_energy: hold_reserved_cost,
            settled_energy: commitment_cost,
            timestamp: now_millis_string(),
        };
        self.event_log
            .append_in_tx(&mut tx, &mut response_event)
            .await?;

        // Audit trail — atomic with event (whitepaper §3 invariant 5).
        let log_index = response_event.log_index as u64;
        self.audit_log
            .append_leaf_in_tx(&mut tx, log_index, &response_event.event_hash)
            .await
            .map_err(|e| KernelError::Audit(e.to_string()))?;
        // Checkpoint generated lazily on read, not here.
        tx.commit().await?;

        info!(
            hold_id = %hold_id,
            decision = %decision,
            agent_id = %agent_id,
            resolved_by = %human_action.actor_id,
            "PIP-001 §11d: hold resolved"
        );

        // --- Approve: re-execute the pending action (PIP-001 §11d) ---
        if decision == "approve" {
            // Reconstruct the original action from the pending_payload stored in hold_request.
            let orig_target = pending_payload
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or(&trigger_target)
                .to_string();
            let orig_action_type_str = pending_payload
                .get("action_type")
                .and_then(Value::as_str)
                .unwrap_or(&trigger_action)
                .to_string();
            let mut orig_payload = pending_payload
                .get("payload")
                .cloned()
                .unwrap_or(Value::Null);

            // PIP-001 §11d: Energy was already reserved at hold trigger time.
            // `reserved_cost` was stored in pending_payload by the hold trigger.
            // Inject `_hold_reserved_cost` so submit_action skips double quote/reserve.

            // Mark the payload as hold-approved so the re-execution bypasses the hold check (PIP-001 §11d).
            // This sentinel prevents a re-trigger loop.
            if let Some(obj) = orig_payload.as_object_mut() {
                obj.insert("_hold_approved".to_string(), Value::Bool(true));
                obj.insert(
                    "_hold_reserved_cost".to_string(),
                    Value::Number(hold_reserved_cost.into()),
                );
                // PIP-001 §11d: If instruction is present, append it to the payload.
                if let Some(instr) = instruction {
                    obj.insert(
                        "_hold_instruction".to_string(),
                        Value::String(instr.to_string()),
                    );
                }
            } else {
                // If payload is not an object (e.g. null), wrap it.
                let mut obj = serde_json::Map::new();
                obj.insert("_hold_approved".to_string(), Value::Bool(true));
                obj.insert(
                    "_hold_reserved_cost".to_string(),
                    Value::Number(hold_reserved_cost.into()),
                );
                if let Some(instr) = instruction {
                    obj.insert(
                        "_hold_instruction".to_string(),
                        Value::String(instr.to_string()),
                    );
                }
                orig_payload = Value::Object(obj);
            }

            let orig_action_type = match orig_action_type_str.as_str() {
                "observe" => ActionType::Observe,
                "create" => ActionType::Create,
                "mutate" => ActionType::Mutate,
                _ => ActionType::Execute,
            };

            let pending_action = Action {
                actor_id: agent_id.clone(),
                action_type: orig_action_type,
                target: orig_target,
                payload: orig_payload,
                timestamp: None,
            };

            info!(
                hold_id = %hold_id,
                agent_id = %agent_id,
                "PIP-001 §11d: re-executing approved pending action"
            );

            // Re-submit the pending action on behalf of the agent.
            // This goes through the full pipeline (quota → reserve → execute → settle → append).
            // Use Box::pin to break the async recursion (submit_action → execute_hold_response → submit_action).
            return Box::pin(self.submit_action(pending_action)).await;
        }

        // --- Reject: return a receipt with commitment cost (PIP-001 §11f) ---
        Ok(SubmitReceipt {
            event_id: response_event.id,
            log_index: response_event.log_index,
            event_hash: response_event.event_hash,
            reserved_cost: hold_reserved_cost,
            settled_cost: commitment_cost,
            artifact_hash: None,
        })
    }

    /// PIP-001 §11e: Lazy expiry for timed-out holds.
    ///
    /// Called before hold-trigger checks in submit_action. Scans pending holds
    /// for the given envelope and auto-rejects any that have exceeded
    /// `hold_timeout_secs`. Same pattern as `is_envelope_expired()` — no
    /// background thread, just check-on-access.
    async fn expire_timed_out_holds(
        &self,
        envelope_id: &str,
        hold_timeout_secs: i64,
    ) -> KernelResult<()> {
        let holds = self
            .envelope_store
            .list_pending_holds_for_envelope(envelope_id)
            .await?;
        let now = now_millis_u64();

        for hold in holds {
            let triggered_at: u64 = hold
                .get("triggered_at")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            if now.saturating_sub(triggered_at) <= (hold_timeout_secs as u64) * 1000 {
                continue;
            }

            // Timed out — treat as reject (PIP-001 §11e/§11f).
            let hold_id = hold.get("hold_id").and_then(|v| v.as_str()).unwrap_or("");
            let agent_id = hold.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
            let pending_payload = hold.get("pending_payload").cloned().unwrap_or(Value::Null);
            let reserved_cost = pending_payload
                .get("reserved_cost")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);

            let commitment_cost = if reserved_cost > 0 {
                ((reserved_cost as f64) * 0.2).ceil() as i64
            } else {
                0
            };

            let timeout_payload = json!({
                "hold_id": hold_id,
                "agent_id": agent_id,
                "decision": "timed_out",
                "reserved_cost": reserved_cost,
                "commitment_cost": commitment_cost,
                "triggered_at": triggered_at.to_string(),
                "expired_at": now.to_string()
            });
            let timeout_action = Action {
                actor_id: agent_id.to_string(),
                action_type: ActionType::Mutate,
                target: format!("ledger/hold/{hold_id}"),
                payload: timeout_payload.clone(),
                timestamp: None,
            };
            let mut timeout_event = EventRecord {
                id: Uuid::new_v4().to_string(),
                log_index: 0,
                event_hash: String::new(),
                actor_id: agent_id.to_string(),
                action_type: "hold_timeout".to_string(),
                target: format!("ledger/hold/{hold_id}"),
                payload: timeout_payload,
                payload_hash: payload_hash_hex(&timeout_action)?,
                artifact_hash: None,
                reserved_energy: reserved_cost,
                settled_energy: commitment_cost,
                timestamp: now_millis_string(),
            };

            let pool = self.state_store.pool();
            let mut tx = pool.begin().await?;
            self.envelope_store
                .resolve_hold_request_in_tx(&mut tx, hold_id, "timed_out", None)
                .await?;
            if reserved_cost > 0 {
                self.energy_ledger
                    .settle_in_tx(&mut tx, agent_id, reserved_cost, commitment_cost)
                    .await?;
            }
            self.event_log
                .append_in_tx(&mut tx, &mut timeout_event)
                .await?;

            // Audit trail — atomic with event (whitepaper §3 invariant 5).
            let t_log_index = timeout_event.log_index as u64;
            self.audit_log
                .append_leaf_in_tx(&mut tx, t_log_index, &timeout_event.event_hash)
                .await
                .map_err(|e| KernelError::Audit(e.to_string()))?;
            // Checkpoint generated lazily on read, not here.
            tx.commit().await?;

            info!(
                hold_id = %hold_id,
                agent_id = %agent_id,
                commitment_cost = commitment_cost,
                "PIP-001 §11e: hold auto-expired (timeout)"
            );
        }
        Ok(())
    }
}
