use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use punkgo_core::errors::{KernelError, KernelResult};

#[derive(Debug, Clone)]
pub struct EnergyReservation {
    pub actor_id: String,
    pub reserved: i64,
}

#[derive(Clone)]
pub struct EnergyLedger {
    pool: SqlitePool,
}

impl EnergyLedger {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn reserve(&self, actor_id: &str, cost: i64) -> KernelResult<EnergyReservation> {
        if cost <= 0 {
            return Err(KernelError::PolicyViolation(
                "reserve cost must be positive".to_string(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT energy_balance, reserved_energy FROM energy_ledger WHERE actor_id = ?1",
        )
        .bind(actor_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            return Err(KernelError::ActorNotFound(actor_id.to_string()));
        };

        let balance: i64 = row.get("energy_balance");
        let reserved_energy: i64 = row.get("reserved_energy");
        let available = balance - reserved_energy;

        if available < cost {
            return Err(KernelError::InsufficientEnergy {
                actor_id: actor_id.to_string(),
                required: cost,
                available,
            });
        }

        sqlx::query(
            r#"
            UPDATE energy_ledger
            SET reserved_energy = reserved_energy + ?1,
                updated_at = ?2
            WHERE actor_id = ?3
            "#,
        )
        .bind(cost)
        .bind(now_millis_string())
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(EnergyReservation {
            actor_id: actor_id.to_string(),
            reserved: cost,
        })
    }

    /// Reserve energy within an existing transaction (PIP-001 §11c).
    /// Used by hold trigger to atomically reserve + write hold_request in one tx.
    pub async fn reserve_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        actor_id: &str,
        cost: i64,
    ) -> KernelResult<()> {
        if cost <= 0 {
            return Err(KernelError::PolicyViolation(
                "reserve cost must be positive".to_string(),
            ));
        }

        let row = sqlx::query(
            "SELECT energy_balance, reserved_energy FROM energy_ledger WHERE actor_id = ?1",
        )
        .bind(actor_id)
        .fetch_optional(&mut **tx)
        .await?;

        let Some(row) = row else {
            return Err(KernelError::ActorNotFound(actor_id.to_string()));
        };

        let balance: i64 = row.get("energy_balance");
        let reserved_energy: i64 = row.get("reserved_energy");
        let available = balance - reserved_energy;

        if available < cost {
            return Err(KernelError::InsufficientEnergy {
                actor_id: actor_id.to_string(),
                required: cost,
                available,
            });
        }

        sqlx::query(
            r#"
            UPDATE energy_ledger
            SET reserved_energy = reserved_energy + ?1,
                updated_at = ?2
            WHERE actor_id = ?3
            "#,
        )
        .bind(cost)
        .bind(now_millis_string())
        .bind(actor_id)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn settle(
        &self,
        actor_id: &str,
        reserved_cost: i64,
        actual_cost: i64,
    ) -> KernelResult<()> {
        let mut tx = self.pool.begin().await?;
        self.settle_in_tx(&mut tx, actor_id, reserved_cost, actual_cost)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn settle_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        actor_id: &str,
        reserved_cost: i64,
        actual_cost: i64,
    ) -> KernelResult<()> {
        if reserved_cost < 0 || actual_cost < 0 {
            return Err(KernelError::PolicyViolation(
                "settle values cannot be negative".to_string(),
            ));
        }

        let row = sqlx::query(
            "SELECT energy_balance, reserved_energy FROM energy_ledger WHERE actor_id = ?1",
        )
        .bind(actor_id)
        .fetch_optional(&mut **tx)
        .await?;

        let Some(row) = row else {
            return Err(KernelError::ActorNotFound(actor_id.to_string()));
        };

        let balance: i64 = row.get("energy_balance");
        let reserved_energy: i64 = row.get("reserved_energy");

        if reserved_energy < reserved_cost {
            return Err(KernelError::PolicyViolation(format!(
                "reserved mismatch for actor {actor_id}: reserved_in_ledger={reserved_energy}, reserved_cost={reserved_cost}, balance={balance}"
            )));
        }

        let extra_needed = (actual_cost - reserved_cost).max(0);
        let available_unreserved = balance - reserved_energy;
        if available_unreserved < extra_needed {
            return Err(KernelError::InsufficientEnergy {
                actor_id: actor_id.to_string(),
                required: extra_needed,
                available: available_unreserved,
            });
        }

        let new_balance = balance - actual_cost;
        if new_balance < 0 {
            return Err(KernelError::InsufficientEnergy {
                actor_id: actor_id.to_string(),
                required: actual_cost,
                available: balance,
            });
        }
        let new_reserved = reserved_energy - reserved_cost;

        sqlx::query(
            r#"
            UPDATE energy_ledger
            SET energy_balance = ?1,
                reserved_energy = ?2,
                updated_at = ?3
            WHERE actor_id = ?4
            "#,
        )
        .bind(new_balance)
        .bind(new_reserved)
        .bind(now_millis_string())
        .bind(actor_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn balance_view(&self, actor_id: &str) -> KernelResult<(i64, i64)> {
        let row = sqlx::query(
            "SELECT energy_balance, reserved_energy FROM energy_ledger WHERE actor_id = ?1",
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Err(KernelError::ActorNotFound(actor_id.to_string()));
        };

        Ok((row.get("energy_balance"), row.get("reserved_energy")))
    }

    /// Credit energy to an actor (for production distribution).
    /// Unlike `settle`, this simply adds energy without any reservation.
    /// Used by the energy producer to distribute per-tick production (PIP-001 §2/§3).
    pub async fn credit_energy_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        actor_id: &str,
        amount: i64,
    ) -> KernelResult<()> {
        if amount <= 0 {
            return Err(KernelError::PolicyViolation(
                "credit amount must be positive".to_string(),
            ));
        }

        let result = sqlx::query(
            r#"
            UPDATE energy_ledger
            SET energy_balance = energy_balance + ?1,
                updated_at = ?2
            WHERE actor_id = ?3
            "#,
        )
        .bind(amount)
        .bind(now_millis_string())
        .bind(actor_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(KernelError::ActorNotFound(actor_id.to_string()));
        }
        Ok(())
    }

    pub async fn create_actor_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        actor_id: &str,
        energy_balance: i64,
    ) -> KernelResult<()> {
        if actor_id.trim().is_empty() {
            return Err(KernelError::PolicyViolation(
                "seed actor_id cannot be empty".to_string(),
            ));
        }
        if energy_balance < 0 {
            return Err(KernelError::PolicyViolation(
                "seed energy_balance cannot be negative".to_string(),
            ));
        }

        let existing = sqlx::query("SELECT actor_id FROM energy_ledger WHERE actor_id = ?1")
            .bind(actor_id)
            .fetch_optional(&mut **tx)
            .await?;
        if existing.is_some() {
            return Err(KernelError::PolicyViolation(format!(
                "actor already exists: {actor_id}"
            )));
        }

        sqlx::query(
            r#"
            INSERT INTO energy_ledger (actor_id, energy_balance, reserved_energy, updated_at)
            VALUES (?1, ?2, 0, ?3)
            "#,
        )
        .bind(actor_id)
        .bind(energy_balance)
        .bind(now_millis_string())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}

fn now_millis_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis().to_string()
}
