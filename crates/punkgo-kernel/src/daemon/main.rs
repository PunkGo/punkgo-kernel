//! PunkGo kernel daemon — the main entry point for running the kernel.
//!
//! Bootstraps the kernel, spawns the IPC server and energy producer,
//! and handles graceful shutdown on CTRL-C.
//!
//! # Lifecycle
//!
//! 1. Acquires exclusive flock on `daemon.addr` (single-instance guard)
//! 2. Writes `pid=<PID>\naddr=<endpoint>` to `daemon.addr` for service discovery
//! 3. Bootstraps the kernel (SQLite state, root actor)
//! 4. Starts the [`EnergyProducer`] background task
//! 5. Listens for IPC connections and dispatches requests to [`Kernel::handle_request`]
//! 6. On shutdown (CTRL-C or IPC shutdown command): cleans up socket, truncates `daemon.addr`

use std::io::{Seek, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use fs2::FileExt;
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

    // Parse --replace flag from args.
    let replace = std::env::args().any(|a| a == "--replace");

    // 1. Ensure state_dir exists BEFORE anything else.
    let config = KernelConfig::default();
    std::fs::create_dir_all(&config.state_dir)?;

    // 2. Open daemon.addr and try flock for single-instance guard.
    let addr_path = config.state_dir.join("daemon.addr");
    let mut addr_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&addr_path)?;

    match addr_file.try_lock_exclusive() {
        Ok(()) => {
            // Got lock — clean up old daemon-*.sock files from crashed instances.
            cleanup_old_sockets(&config.state_dir);
        }
        Err(_) => {
            if !replace {
                // Read existing daemon.addr to show PID.
                let content = std::fs::read_to_string(&addr_path).unwrap_or_default();
                let old_pid = parse_pid(&content).unwrap_or(0);
                eprintln!("punkgo-kerneld is already running (PID {old_pid}).");
                eprintln!("Use --replace to stop it and start a new instance.");
                std::process::exit(1);
            }
            // --replace: gracefully stop old daemon, then take over.
            replace_old_daemon(&addr_path)?;
            // Blocking wait for lock (old daemon releasing).
            addr_file.lock_exclusive()?;
            cleanup_old_sockets(&config.state_dir);
        }
    }

    // 3. Write our info to daemon.addr (truncate first to clear stale data).
    addr_file.set_len(0)?;
    addr_file.seek(std::io::SeekFrom::Start(0))?;
    write!(
        addr_file,
        "pid={}\naddr={}",
        std::process::id(),
        config.ipc_endpoint
    )?;
    addr_file.sync_all()?;

    // 4. Bootstrap kernel.
    let kernel = Arc::new(Kernel::bootstrap(&config).await?);
    info!(state_dir = %config.state_dir.display(), "kernel bootstrapped");

    // 5. Create shutdown channel for graceful termination.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn energy production background task.
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
    let shutdown_tx_for_ipc = shutdown_tx.clone();
    let mut server = tokio::spawn(async move {
        ipc::run_ipc_server(kernel_for_server, &endpoint, shutdown_tx_for_ipc).await
    });

    // 6. Wait for shutdown signal (CTRL-C, IPC shutdown, or task failure).
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
        _ = shutdown_signal(shutdown_tx.clone()) => {
            info!("shutdown signal received, shutting down kernel");

            server.abort();
            if let Err(err) = server.await {
                warn!(error = %err, "ipc server aborted");
            }

            // Wait for energy producer to finish.
            if let Err(err) = energy_task.await {
                warn!(error = %err, "energy producer task aborted");
            }
        }
    }

    // 7. Cleanup on exit.
    // Remove our per-PID socket file.
    let _ = std::fs::remove_file(&config.ipc_endpoint);
    // Truncate daemon.addr (do NOT delete — preserves inode for flock stability).
    let _ = addr_file.set_len(0);
    // flock released automatically when addr_file is dropped.
    drop(kernel);
    info!("daemon exited cleanly");
    Ok(())
}

/// Wait for CTRL-C or an IPC shutdown command (via the watch channel).
async fn shutdown_signal(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    let mut rx = shutdown_tx.subscribe();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            let _ = shutdown_tx.send(true);
        }
        _ = async {
            loop {
                rx.changed().await.ok();
                if *rx.borrow() {
                    break;
                }
            }
        } => {}
    }
}

/// Remove stale `daemon-*.sock` files left by crashed daemons.
fn cleanup_old_sockets(state_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("daemon-") && name.ends_with(".sock") {
            info!(file = %name, "removing stale daemon socket");
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Parse `pid=<N>` from daemon.addr content.
fn parse_pid(content: &str) -> Option<u32> {
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("pid=") {
            return val.trim().parse().ok();
        }
    }
    None
}

/// Parse `addr=<endpoint>` from daemon.addr content.
fn parse_addr(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("addr=") {
            return Some(val.trim().to_string());
        }
    }
    None
}

/// Stop the old daemon: try graceful IPC shutdown, then force kill if needed.
fn replace_old_daemon(addr_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(addr_path).unwrap_or_default();
    let old_pid = parse_pid(&content).unwrap_or(0);
    let old_addr = parse_addr(&content);

    if let Some(addr) = old_addr {
        // Try sending shutdown command via IPC.
        info!(pid = old_pid, addr = %addr, "sending shutdown to old daemon");
        if let Err(err) = send_shutdown_ipc(&addr) {
            warn!(error = %err, "failed to send IPC shutdown, will force kill");
        }
    }

    // Wait up to 3 seconds for old daemon to exit.
    if old_pid > 0 {
        for _ in 0..30 {
            if !is_process_alive(old_pid) {
                info!(pid = old_pid, "old daemon exited");
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Still alive — force kill.
        warn!(pid = old_pid, "old daemon did not exit, force killing");
        kill_process(old_pid);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    Ok(())
}

/// Send a shutdown request to an old daemon via IPC (blocking).
fn send_shutdown_ipc(endpoint: &str) -> Result<()> {
    use interprocess::local_socket::{
        GenericFilePath, GenericNamespaced, ToFsName, ToNsName, traits::Stream as _,
    };
    use std::io::{BufRead, BufReader};

    let name = if endpoint.contains('/') || endpoint.contains('\\') {
        endpoint.to_fs_name::<GenericFilePath>()?
    } else {
        endpoint.to_ns_name::<GenericNamespaced>()?
    };

    let mut conn = interprocess::local_socket::Stream::connect(name)?;

    let request = serde_json::json!({
        "request_id": "shutdown-replace",
        "type": "read",
        "payload": { "kind": "shutdown" }
    });
    let mut msg = serde_json::to_vec(&request)?;
    msg.push(b'\n');
    std::io::Write::write_all(&mut conn, &msg)?;

    // Read response (best-effort).
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    let _ = reader.read_line(&mut line);

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
