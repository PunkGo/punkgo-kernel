//! PunkGo kernel daemon — the main entry point for running the kernel.
//!
//! Bootstraps the kernel, spawns the IPC server and energy producer,
//! and handles graceful shutdown on CTRL-C.
//!
//! # What it does
//!
//! 1. Initializes the SQLite state directory and creates the root actor
//! 2. Starts the [`EnergyProducer`] background task (continuous tick-based energy distribution)
//! 3. Listens for IPC connections and dispatches requests to [`Kernel::handle_request`]
//! 4. On CTRL-C: signals the energy producer to stop, aborts the IPC server, and exits

use std::sync::Arc;

use anyhow::Result;
use punkgo_kernel::{EnergyProducer, Kernel, KernelConfig};
use tracing::{error, info, warn};

#[path = "ipc.rs"]
mod ipc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    let config = KernelConfig::default();
    let kernel = Arc::new(Kernel::bootstrap(&config).await?);
    info!(state_dir = %config.state_dir.display(), "kernel bootstrapped");

    // Phase 2: Create shutdown channel for graceful termination of background tasks.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Phase 2: Spawn energy production background task (E-P2).
    let energy_producer = EnergyProducer::new(
        kernel.pool(),
        kernel.actor_store().clone(),
        kernel.energy_ledger().clone(),
        kernel.stellar_config().clone(),
    );
    let mut energy_task = tokio::spawn(async move {
        energy_producer.run(shutdown_rx).await;
    });

    let endpoint = config.ipc_endpoint.clone();
    let kernel_for_server = Arc::clone(&kernel);
    let mut server =
        tokio::spawn(async move { ipc::run_ipc_server(kernel_for_server, &endpoint).await });

    tokio::select! {
        res = &mut server => {
            match res {
                Ok(Ok(())) => info!("ipc server exited"),
                Ok(Err(err)) => error!(error = %err, "ipc server exited with error"),
                Err(err) => error!(error = %err, "ipc server task join error"),
            }
        }
        res = &mut energy_task => {
            match res {
                Ok(()) => info!("energy producer exited"),
                Err(err) => error!(error = %err, "energy producer task join error"),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("ctrl-c received, shutting down kernel");

            // Signal energy producer to stop gracefully.
            let _ = shutdown_tx.send(true);

            server.abort();
            if let Err(err) = server.await {
                warn!(error = %err, "ipc server aborted");
            }

            // Wait for energy producer to finish (should be fast after shutdown signal).
            if let Err(err) = energy_task.await {
                warn!(error = %err, "energy producer task aborted");
            }
        }
    }

    drop(kernel);
    Ok(())
}
