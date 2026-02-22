use punkgo_benchmarks::write_result;
use serde_json::json;

fn main() {
    let cpu_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let processor = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default();
    let _processor_count = std::env::var("NUMBER_OF_PROCESSORS").unwrap_or_default();

    // Get git commit
    let git_commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    // Get rustc version
    let rustc_version = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    let now = chrono::Utc::now().to_rfc3339();

    let result = json!({
        "hardware": {
            "cpu": processor,
            "cpu_threads": cpu_threads,
            "os": format!("{os} {arch}"),
            "ram": "see system info",
            "storage": "SSD (assumed)",
            "int8_tops": "configured in stellar.toml (default: 100.0)"
        },
        "software": {
            "rust_version": rustc_version,
            "punkgo_version": punkgo_core::VERSION,
            "git_commit": git_commit,
            "storage_backend": "SQLite (WAL mode)"
        },
        "benchmark": {
            "date": now,
            "repetitions": 10,
            "warmup_runs": 3
        }
    });

    write_result("environment.json", &result);
}
