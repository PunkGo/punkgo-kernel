//! PunkGo cryptographic audit trail — Merkle tree proofs and checkpoints.
//!
//! This crate implements the append-only verification layer using Google's
//! [tlog](https://research.swtch.com/tlog) algorithm:
//!
//! - [`AuditLog::append_leaf`] — add event hash to the Merkle tree
//! - [`AuditLog::make_checkpoint`] — generate C2SP-compatible signed tree head
//! - [`AuditLog::inclusion_proof`] — RFC 6962 proof that an event exists in the tree
//! - [`AuditLog::consistency_proof`] — RFC 6962 proof that the tree is append-only
//!
//! These proofs enable whitepaper invariants §3.5 (append-only provable) and
//! §3.7 (independently verifiable). Any third party can verify history integrity
//! using only the checkpoint and proof hashes.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use tlog_tiles::{checkpoint::Checkpoint, tlog};
use tracing::debug;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("proof error: {0}")]
    Proof(#[from] tlog::Error),
    #[error("malformed checkpoint")]
    Checkpoint,
    #[error("hash not found at index {0}")]
    HashNotFound(u64),
    #[error("invalid hex hash: {0}")]
    InvalidHex(String),
    #[error("no checkpoint exists yet")]
    NoCheckpoint,
}

/// A persisted Signed Tree Head (checkpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditCheckpoint {
    pub tree_size: i64,
    pub root_hash: String,
    /// C2SP tlog-checkpoint canonical text (unsigned in dev mode).
    pub checkpoint_text: String,
    pub created_at: String,
}

/// In-memory HashReader backed by a HashMap (loaded from DB before each tlog call).
struct InMemHashReader(HashMap<u64, tlog::Hash>);

impl tlog::HashReader for InMemHashReader {
    fn read_hashes(&self, indexes: &[u64]) -> Result<Vec<tlog::Hash>, tlog::Error> {
        indexes
            .iter()
            .map(|&i| self.0.get(&i).copied().ok_or(tlog::Error::IndexesNotInTree))
            .collect()
    }
}

/// Audit log: maintains the Merkle tree of event hashes and generates
/// C2SP-compatible checkpoints and proofs using Google's tlog algorithm.
#[derive(Clone)]
pub struct AuditLog {
    pool: SqlitePool,
    origin: String,
}

impl AuditLog {
    pub fn new(pool: SqlitePool, origin: impl Into<String>) -> Self {
        Self {
            pool,
            origin: origin.into(),
        }
    }

    /// Called after each event is committed. Appends the leaf hash to the
    /// Merkle tree and stores all newly computable intermediate node hashes.
    ///
    /// `log_index` is 0-based. `leaf_hash_hex` is the 64-char hex event_hash.
    pub async fn append_leaf(&self, log_index: u64, leaf_hash_hex: &str) -> Result<(), AuditError> {
        let leaf = hex_to_hash(leaf_hash_hex)?;
        let n = log_index;

        // Hashes to write: start with the leaf itself.
        let mut to_store: Vec<(u64, tlog::Hash)> = vec![(tlog::stored_hash_index(0, n), leaf)];
        let mut current = leaf;
        let mut level = 0u8;

        // Walk up the tree: while n's bit at `level` is 1, we are the right
        // child at that level and can compute the parent node hash.
        while (n >> level) & 1 == 1 {
            let n_at_level = n >> level;
            let left_idx = tlog::stored_hash_index(level, n_at_level - 1);
            let left = self.read_hash(left_idx).await?;
            let parent = tlog::node_hash(left, current);
            level += 1;
            let parent_idx = tlog::stored_hash_index(level, n_at_level >> 1);
            to_store.push((parent_idx, parent));
            current = parent;
        }

        // Persist all computed hashes atomically.
        let mut tx = self.pool.begin().await?;
        for (idx, hash) in to_store {
            sqlx::query("INSERT OR REPLACE INTO audit_hashes (hash_index, hash) VALUES (?1, ?2)")
                .bind(idx as i64)
                .bind(hash.0.as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        debug!(log_index, "audit leaf appended");
        Ok(())
    }

    /// Generates and persists a checkpoint for the current tree size.
    /// `tree_size` = log_index + 1 (number of events committed so far).
    pub async fn make_checkpoint(&self, tree_size: u64) -> Result<AuditCheckpoint, AuditError> {
        let reader = self.load_all_hashes().await?;
        let root = tlog::tree_hash(tree_size, &reader)?;
        let root_hex = hash_to_hex(&root);

        let cp = Checkpoint::new(&self.origin, tree_size, root, "")
            .map_err(|_| AuditError::Checkpoint)?;
        let cp_text = String::from_utf8(cp.to_bytes()).unwrap_or_default();

        let now = now_millis_string();
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO audit_checkpoints
                (tree_size, root_hash, checkpoint_text, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        )
        .bind(tree_size as i64)
        .bind(&root_hex)
        .bind(&cp_text)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        debug!(tree_size, root_hash = %root_hex, "checkpoint created");
        Ok(AuditCheckpoint {
            tree_size: tree_size as i64,
            root_hash: root_hex,
            checkpoint_text: cp_text,
            created_at: now,
        })
    }

    /// Returns the latest checkpoint, if any.
    pub async fn latest_checkpoint(&self) -> Result<AuditCheckpoint, AuditError> {
        let row = sqlx::query(
            r#"
            SELECT tree_size, root_hash, checkpoint_text, created_at
            FROM audit_checkpoints
            ORDER BY tree_size DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| AuditCheckpoint {
            tree_size: r.get("tree_size"),
            root_hash: r.get("root_hash"),
            checkpoint_text: r.get("checkpoint_text"),
            created_at: r.get("created_at"),
        })
        .ok_or(AuditError::NoCheckpoint)
    }

    /// Generates an inclusion proof showing that the event at `log_index`
    /// is contained in the tree of size `tree_size`.
    ///
    /// Returns proof hashes as hex strings (RFC 6962 Merkle audit path).
    pub async fn inclusion_proof(
        &self,
        log_index: u64,
        tree_size: u64,
    ) -> Result<Vec<String>, AuditError> {
        let reader = self.load_all_hashes().await?;
        let proof = tlog::prove_record(tree_size, log_index, &reader)?;
        Ok(proof.iter().map(hash_to_hex).collect())
    }

    /// Generates a consistency proof showing that the tree of size `new_size`
    /// is an append-only extension of the tree of size `old_size`.
    ///
    /// Returns proof hashes as hex strings (RFC 6962 consistency proof).
    pub async fn consistency_proof(
        &self,
        old_size: u64,
        new_size: u64,
    ) -> Result<Vec<String>, AuditError> {
        let reader = self.load_all_hashes().await?;
        let proof = tlog::prove_tree(new_size, old_size, &reader)?;
        Ok(proof.iter().map(hash_to_hex).collect())
    }

    /// Returns the current tree size (number of leaves stored).
    pub async fn tree_size(&self) -> Result<u64, AuditError> {
        // Use the latest checkpoint's tree_size as authoritative.
        let row =
            sqlx::query("SELECT tree_size FROM audit_checkpoints ORDER BY tree_size DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .map(|r| r.get::<i64, _>("tree_size") as u64)
            .unwrap_or(0))
    }

    async fn read_hash(&self, idx: u64) -> Result<tlog::Hash, AuditError> {
        let row = sqlx::query("SELECT hash FROM audit_hashes WHERE hash_index = ?1")
            .bind(idx as i64)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(r) => {
                let bytes: Vec<u8> = r.get("hash");
                if bytes.len() != 32 {
                    return Err(AuditError::HashNotFound(idx));
                }
                let mut h = [0u8; 32];
                h.copy_from_slice(&bytes);
                Ok(tlog::Hash(h))
            }
            None => Err(AuditError::HashNotFound(idx)),
        }
    }

    async fn load_all_hashes(&self) -> Result<InMemHashReader, AuditError> {
        let rows = sqlx::query("SELECT hash_index, hash FROM audit_hashes ORDER BY hash_index ASC")
            .fetch_all(&self.pool)
            .await?;

        let map = rows
            .into_iter()
            .map(|r| {
                let idx: i64 = r.get("hash_index");
                let bytes: Vec<u8> = r.get("hash");
                let mut h = [0u8; 32];
                h.copy_from_slice(&bytes);
                (idx as u64, tlog::Hash(h))
            })
            .collect();

        Ok(InMemHashReader(map))
    }
}

fn hex_to_hash(hex: &str) -> Result<tlog::Hash, AuditError> {
    if hex.len() != 64 {
        return Err(AuditError::InvalidHex(hex.to_string()));
    }
    let mut bytes = [0u8; 32];
    for i in 0..32 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| AuditError::InvalidHex(hex.to_string()))?;
    }
    Ok(tlog::Hash(bytes))
}

fn hash_to_hex(h: &tlog::Hash) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for &b in h.0.iter() {
        out.push(LUT[(b >> 4) as usize] as char);
        out.push(LUT[(b & 0x0f) as usize] as char);
    }
    out
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}
