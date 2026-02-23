use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use punkgo_core::errors::{KernelError, KernelResult};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use tracing::{info, warn};

#[derive(Clone)]
pub struct StatePaths {
    pub root: PathBuf,
    pub workspace_root: PathBuf,
    pub quarantine_root: PathBuf,
    pub blobs_root: PathBuf,
    pub db_path: PathBuf,
}

pub struct StateStore {
    pool: SqlitePool,
    paths: StatePaths,
    _lock_file: File,
    lock_path: PathBuf,
}

impl StateStore {
    pub async fn bootstrap(root: impl AsRef<Path>) -> KernelResult<Self> {
        let root = root.as_ref().to_path_buf();
        let workspace_root = root.join("workspaces");
        let quarantine_root = root.join("quarantine");
        let event_log_root = root.join("event_log");
        let blobs_root = root.join("blobs");

        fs::create_dir_all(&workspace_root)?;
        fs::create_dir_all(&quarantine_root)?;
        fs::create_dir_all(&event_log_root)?;
        fs::create_dir_all(&blobs_root)?;

        let lock_path = root.join("kernel.lock");
        let lock_file_result = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path);
        let lock_file = match lock_file_result {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(KernelError::PolicyViolation(format!(
                    "state lock exists at {}. If no kernel process is running, remove this file and retry.",
                    lock_path.display()
                )));
            }
            Err(err) => return Err(err.into()),
        };

        let db_path = root.join("punkgo.db");
        let connect_options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(std::time::Duration::from_secs(5));

        let max_conn = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(8)
            .max(8);

        let pool = SqlitePoolOptions::new()
            .max_connections(max_conn)
            .connect_with(connect_options)
            .await?;

        Self::init_schema(&pool).await?;
        Self::seed_defaults(&pool).await?;

        let store = Self {
            pool,
            paths: StatePaths {
                root,
                workspace_root,
                quarantine_root,
                blobs_root,
                db_path,
            },
            _lock_file: lock_file,
            lock_path,
        };

        info!(state_root = %store.paths.root.display(), "state store bootstrapped");
        store.run_startup_integrity_checks().await?;
        Ok(store)
    }

    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    pub fn paths(&self) -> &StatePaths {
        &self.paths
    }

    pub async fn actor_exists(&self, actor_id: &str) -> KernelResult<bool> {
        let row = sqlx::query("SELECT actor_id FROM actors WHERE actor_id = ?1")
            .bind(actor_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn init_schema(pool: &SqlitePool) -> KernelResult<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                log_index INTEGER NOT NULL UNIQUE,
                event_hash TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                action_type TEXT NOT NULL,
                target TEXT NOT NULL,
                payload TEXT NOT NULL,
                payload_hash TEXT NOT NULL,
                artifact_hash TEXT,
                reserved_energy INTEGER NOT NULL,
                settled_energy INTEGER NOT NULL,
                timestamp TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS energy_ledger (
                actor_id TEXT PRIMARY KEY,
                energy_balance INTEGER NOT NULL,
                reserved_energy INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS system_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS audit_hashes (
                hash_index INTEGER NOT NULL PRIMARY KEY,
                hash BLOB NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS audit_checkpoints (
                tree_size INTEGER NOT NULL PRIMARY KEY,
                root_hash TEXT NOT NULL,
                checkpoint_text TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Phase 1: actors table — explicit actor records with type, lineage, boundary.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS actors (
                actor_id         TEXT PRIMARY KEY,
                actor_type       TEXT NOT NULL CHECK(actor_type IN ('human', 'agent')),
                creator_id       TEXT,
                lineage          TEXT NOT NULL DEFAULT '[]',
                purpose          TEXT,
                status           TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'frozen')),
                writable_targets TEXT NOT NULL DEFAULT '[]',
                energy_share     REAL NOT NULL DEFAULT 0.0,
                reduction_policy TEXT NOT NULL DEFAULT 'none',
                created_at       TEXT NOT NULL,
                updated_at       TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Phase 4b: envelopes table — budget envelope authorization system (PIP-001 §11).
        // PIP-001 §11a: hold_on, hold_timeout_secs are part of the base schema.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS envelopes (
                envelope_id        TEXT PRIMARY KEY,
                actor_id           TEXT NOT NULL,
                grantor_id         TEXT NOT NULL,
                parent_envelope_id TEXT,
                budget             INTEGER NOT NULL,
                budget_consumed    INTEGER NOT NULL DEFAULT 0,
                targets            TEXT NOT NULL,
                actions            TEXT NOT NULL,
                duration_secs      INTEGER,
                report_every       INTEGER,
                hold_on            TEXT NOT NULL DEFAULT '[]',
                hold_timeout_secs  INTEGER,
                status             TEXT NOT NULL DEFAULT 'active'
                                   CHECK(status IN ('active','expired','revoked')),
                last_report_at     INTEGER NOT NULL DEFAULT 0,
                created_at         TEXT NOT NULL,
                expires_at         TEXT,
                updated_at         TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        // PIP-001 §11b: hold_requests table — tracks hold events and pending actions.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hold_requests (
                hold_id          TEXT PRIMARY KEY,
                envelope_id      TEXT NOT NULL,
                agent_id         TEXT NOT NULL,
                trigger_target   TEXT NOT NULL,
                trigger_action   TEXT NOT NULL,
                pending_payload  TEXT NOT NULL,
                status           TEXT NOT NULL DEFAULT 'pending'
                                 CHECK(status IN ('pending','approved','rejected','timed_out')),
                decision         TEXT,
                instruction      TEXT,
                triggered_at     TEXT NOT NULL,
                resolved_at      TEXT,
                FOREIGN KEY (envelope_id) REFERENCES envelopes(envelope_id)
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    async fn seed_defaults(pool: &SqlitePool) -> KernelResult<()> {
        let now = now_millis_string();
        sqlx::query(
            r#"
            INSERT INTO system_meta (key, value)
            VALUES ('schema_version', 'v1')
            ON CONFLICT(key) DO NOTHING
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO energy_ledger (actor_id, energy_balance, reserved_energy, updated_at)
            VALUES ('root', 1000000, 0, ?1)
            ON CONFLICT(actor_id) DO NOTHING
            "#,
        )
        .bind(&now)
        .execute(pool)
        .await?;

        // Phase 1: seed root as human actor with full wildcard boundary (PIP-001 §10).
        sqlx::query(
            r#"
            INSERT INTO actors (
                actor_id, actor_type, creator_id, lineage, purpose, status,
                writable_targets, energy_share, reduction_policy, created_at, updated_at
            )
            VALUES ('root', 'human', NULL, '[]', NULL, 'active',
                    '[{"target":"**","actions":["create","mutate","execute"]}]',
                    100.0, 'none', ?1, ?1)
            ON CONFLICT(actor_id) DO NOTHING
            "#,
        )
        .bind(&now)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn run_startup_integrity_checks(&self) -> KernelResult<()> {
        info!("running startup integrity checks");
        let violation = sqlx::query(
            r#"
            SELECT actor_id, energy_balance, reserved_energy
            FROM energy_ledger
            WHERE reserved_energy < 0
               OR energy_balance < 0
               OR reserved_energy > energy_balance
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = violation {
            let actor_id: String = row.get("actor_id");
            return Err(KernelError::PolicyViolation(format!(
                "energy ledger consistency violation for actor {actor_id}"
            )));
        }

        let invalid_action = sqlx::query(
            r#"
            SELECT action_type
            FROM events
            WHERE action_type NOT IN (
                'observe', 'create', 'mutate', 'execute',
                'circuit_breaker',
                'hold_request', 'hold_response', 'hold_timeout'
            )
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = invalid_action {
            let action_type: String = row.get("action_type");
            return Err(KernelError::PolicyViolation(format!(
                "action integrity violation: unsupported action_type={action_type}"
            )));
        }

        let invalid_energy_event = sqlx::query(
            r#"
            SELECT id, log_index, action_type, reserved_energy, settled_energy
            FROM events
            WHERE (action_type IN ('mutate', 'execute') AND (reserved_energy <= 0 OR settled_energy <= 0))
               OR reserved_energy < 0
               OR settled_energy < 0
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = invalid_energy_event {
            let event_id: String = row.get("id");
            return Err(KernelError::PolicyViolation(format!(
                "energy integrity violation in event {event_id}"
            )));
        }

        self.validate_audit_coverage().await?;

        info!("startup integrity checks passed");
        Ok(())
    }

    /// Verify audit log coverage — the latest checkpoint's tree_size must
    /// equal the total event count, meaning every committed event is captured
    /// in the Merkle tree. Emits a warning on gap rather than refusing to start.
    async fn validate_audit_coverage(&self) -> KernelResult<()> {
        let event_count: i64 = sqlx::query("SELECT COUNT(*) AS cnt FROM events")
            .fetch_one(&self.pool)
            .await?
            .get("cnt");

        if event_count == 0 {
            return Ok(());
        }

        let checkpoint_tree_size: Option<i64> =
            sqlx::query("SELECT tree_size FROM audit_checkpoints ORDER BY tree_size DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await?
                .map(|r| r.get("tree_size"));

        match checkpoint_tree_size {
            None => {
                warn!(
                    event_count,
                    "audit coverage gap: {} event(s) committed but no audit checkpoint exists",
                    event_count
                );
            }
            Some(tree_size) if tree_size < event_count => {
                warn!(
                    event_count,
                    checkpoint_tree_size = tree_size,
                    "audit coverage gap: {} event(s) not yet covered by Merkle checkpoint",
                    event_count - tree_size
                );
            }
            _ => {}
        }

        Ok(())
    }

    /// Records a governance policy version change in system_meta.
    /// Called by the kernel whenever a `system/policy` Create event is committed.
    pub async fn set_policy_version(&self, version: &str) -> KernelResult<()> {
        self.set_meta("policy_version", version).await
    }

    async fn set_meta(&self, key: &str, value: &str) -> KernelResult<()> {
        sqlx::query(
            r#"
            INSERT INTO system_meta (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl Drop for StateStore {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}
