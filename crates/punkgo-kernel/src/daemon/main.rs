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

use std::path::PathBuf;
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

    // Acquire PID lockfile — prevents multiple daemons and detects stale pipes.
    let lockfile = config.state_dir.join("daemon.pid");
    check_and_acquire_lock(&lockfile)?;

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
    info!(endpoint = %endpoint, "IPC endpoint resolved");
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

    // Clean up lockfile on graceful shutdown.
    let _ = std::fs::remove_file(&lockfile);
    drop(kernel);
    Ok(())
}

/// Check for existing daemon via PID lockfile. If a stale lock exists (process
/// dead), prompt the user to confirm cleanup.
fn check_and_acquire_lock(lockfile: &std::path::Path) -> Result<()> {
    if lockfile.exists() {
        let content = std::fs::read_to_string(lockfile).unwrap_or_default();
        let old_pid: u32 = content.trim().parse().unwrap_or(0);

        if old_pid > 0 && is_process_alive(old_pid) {
            eprintln!("Another punkgo-kerneld is running (PID {old_pid}).");
            eprintln!();
            eprint!("Kill it and continue? [y/N] ");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes") {
                kill_process(old_pid);
                std::thread::sleep(std::time::Duration::from_secs(2));
            } else {
                eprintln!("Aborted.");
                std::process::exit(1);
            }
        } else {
            // Stale lockfile — old daemon crashed. Clean up silently.
            info!(old_pid, "removing stale lockfile");
        }
    }

    // Write our PID.
    std::fs::write(lockfile, std::process::id().to_string())?;
    Ok(())
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output();
        match output {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                // CSV format: "process.exe","PID","...". Check for exact PID field.
                text.contains(&format!("\"{pid}\""))
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn kill_process(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}
