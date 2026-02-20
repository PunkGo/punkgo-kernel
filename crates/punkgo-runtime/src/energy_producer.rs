//! Energy production background task — continuous energy generation per tick.
//!
//! Covers: PIP-001 §2 (continuous production), §3 (share distribution).
//!
//! Every `tick_interval_ms` milliseconds, the producer:
//! 1. Queries all active actors with `energy_share > 0`
//! 2. Computes total shares across active actors
//! 3. Allocates `energy_per_tick` proportionally to each actor
//! 4. Credits energy via `credit_energy_in_tx()`
//!
//! The kernel itself does not accumulate energy (energy neutrality). All produced energy
//! is distributed to actors.

use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use punkgo_core::stellar::StellarConfig;
use punkgo_state::{ActorStore, EnergyLedger};

/// Background energy producer that runs in a tokio task.
pub struct EnergyProducer {
    pool: SqlitePool,
    actor_store: ActorStore,
    energy_ledger: EnergyLedger,
    config: StellarConfig,
}

/// Result of a single production tick — used for testing and monitoring.
#[derive(Debug, Clone)]
pub struct TickResult {
    pub total_energy_produced: i64,
    pub actors_credited: usize,
    pub total_shares: f64,
}

impl EnergyProducer {
    pub fn new(
        pool: SqlitePool,
        actor_store: ActorStore,
        energy_ledger: EnergyLedger,
        config: StellarConfig,
    ) -> Self {
        Self {
            pool,
            actor_store,
            energy_ledger,
            config,
        }
    }

    /// Run the production loop until the shutdown signal is received.
    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let tick_interval = std::time::Duration::from_millis(self.config.tick_interval_ms);
        let energy_per_tick = self.config.effective_energy_per_tick();

        info!(
            energy_per_tick,
            tick_interval_ms = self.config.tick_interval_ms,
            "energy producer started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(tick_interval) => {
                    match self.produce_tick(energy_per_tick).await {
                        Ok(result) => {
                            debug!(
                                total_produced = result.total_energy_produced,
                                actors = result.actors_credited,
                                "energy tick completed"
                            );
                        }
                        Err(err) => {
                            warn!(error = %err, "energy production tick failed");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!("energy producer shutting down");
                    return;
                }
            }
        }
    }

    /// Execute a single production tick. Public for testing.
    pub async fn produce_tick(&self, energy_per_tick: i64) -> Result<TickResult, String> {
        // 1. Query all active actors with energy_share > 0
        let actors = self
            .actor_store
            .list_active_with_shares()
            .await
            .map_err(|e| format!("failed to list active actors: {e}"))?;

        if actors.is_empty() {
            return Ok(TickResult {
                total_energy_produced: 0,
                actors_credited: 0,
                total_shares: 0.0,
            });
        }

        // 2. Compute total shares
        let total_shares: f64 = actors.iter().map(|(_, share)| share).sum();
        if total_shares <= 0.0 {
            return Ok(TickResult {
                total_energy_produced: 0,
                actors_credited: 0,
                total_shares: 0.0,
            });
        }

        // 3. Allocate proportionally — use floor + remainder distribution
        // to ensure sum of credits == energy_per_tick exactly (energy neutrality).
        let mut allocations: Vec<(&str, i64)> = Vec::with_capacity(actors.len());
        let mut allocated_total: i64 = 0;

        for (actor_id, share) in &actors {
            let proportion = *share / total_shares;
            let credit = (energy_per_tick as f64 * proportion).floor() as i64;
            allocations.push((actor_id.as_str(), credit));
            allocated_total += credit;
        }

        // Distribute remainder to actors in order (round-robin for fairness)
        let mut remainder = energy_per_tick - allocated_total;
        let mut idx = 0;
        while remainder > 0 && !allocations.is_empty() {
            allocations[idx].1 += 1;
            remainder -= 1;
            idx = (idx + 1) % allocations.len();
        }

        // 4. Credit energy in a single transaction (energy neutrality: kernel doesn't keep any)
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("failed to begin tx: {e}"))?;

        let mut actors_credited = 0;
        let mut total_credited: i64 = 0;

        for (actor_id, credit) in &allocations {
            if *credit <= 0 {
                continue;
            }
            self.energy_ledger
                .credit_energy_in_tx(&mut tx, actor_id, *credit)
                .await
                .map_err(|e| format!("failed to credit {actor_id}: {e}"))?;
            actors_credited += 1;
            total_credited += credit;
        }

        tx.commit()
            .await
            .map_err(|e| format!("failed to commit energy production tx: {e}"))?;

        Ok(TickResult {
            total_energy_produced: total_credited,
            actors_credited,
            total_shares,
        })
    }
}
