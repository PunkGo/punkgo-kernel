//! PunkGo CLI client — interact with a running `punkgo-kerneld` daemon.
//!
//! Communicates over IPC (Unix socket or Windows named pipe) using line-delimited JSON.
//!
//! # Commands
//!
//! - `read` — query kernel state (health, actor_energy, events, snapshot, etc.)
//! - `quote` — get energy cost for an action without submitting
//! - `submit` — submit an action through the 7-step pipeline
//! - `seed-actor` — shorthand for creating a new actor via root
//! - `audit` — query the cryptographic audit trail (checkpoint, proof, consistency)
//! - `send` — send a typed request with custom payload
//! - `raw` — send a raw JSON request envelope

use std::env;
use std::io::{BufRead, BufReader, Write};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use uuid::Uuid;

fn main() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    let endpoint = parse_endpoint(&mut args);
    if args.is_empty() {
        print_usage();
        return Ok(());
    }

    let command = args.remove(0);
    let request = match command.as_str() {
        "read" => build_read_request(args)?,
        "quote" => build_action_request("quote", args)?,
        "submit" => build_action_request("submit", args)?,
        "seed-actor" => build_seed_actor_request(args)?,
        "audit" => build_audit_request(args)?,
        "send" => build_send_request(args)?,
        "raw" => build_raw_request(args)?,
        "help" | "-h" | "--help" => {
            print_usage();
            return Ok(());
        }
        other => bail!("unknown command: {other}"),
    };

    let line = serde_json::to_string(&request)?;
    let response_line = send_request(&endpoint, &line)?;
    let response_json: Value = serde_json::from_str(&response_line)
        .with_context(|| format!("invalid JSON response: {response_line}"))?;

    println!("{}", serde_json::to_string_pretty(&response_json)?);
    Ok(())
}

fn parse_endpoint(args: &mut Vec<String>) -> String {
    if args.len() >= 2 && (args[0] == "--endpoint" || args[0] == "-e") {
        let endpoint = args[1].clone();
        args.drain(0..2);
        return endpoint;
    }
    default_endpoint()
}

fn build_read_request(args: Vec<String>) -> Result<Value> {
    if args.is_empty() {
        bail!(
            "usage: punkgo-cli read <kind> [--actor-id <id>] [--limit <n>] \
             [--log-index <n>] [--tree-size <n>] [--old-size <n>]"
        );
    }

    let kind = args[0].clone();
    let mut actor_id: Option<String> = None;
    let mut limit: Option<i64> = None;
    let mut log_index: Option<i64> = None;
    let mut tree_size: Option<i64> = None;
    let mut old_size: Option<i64> = None;

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--actor-id" => {
                if i + 1 >= args.len() {
                    bail!("--actor-id requires a value");
                }
                actor_id = Some(args[i + 1].clone());
                i += 2;
            }
            "--limit" => {
                if i + 1 >= args.len() {
                    bail!("--limit requires a value");
                }
                let parsed = args[i + 1]
                    .parse::<i64>()
                    .with_context(|| format!("invalid --limit value: {}", args[i + 1]))?;
                limit = Some(parsed);
                i += 2;
            }
            "--log-index" => {
                if i + 1 >= args.len() {
                    bail!("--log-index requires a value");
                }
                let parsed = args[i + 1]
                    .parse::<i64>()
                    .with_context(|| format!("invalid --log-index value: {}", args[i + 1]))?;
                log_index = Some(parsed);
                i += 2;
            }
            "--tree-size" => {
                if i + 1 >= args.len() {
                    bail!("--tree-size requires a value");
                }
                let parsed = args[i + 1]
                    .parse::<i64>()
                    .with_context(|| format!("invalid --tree-size value: {}", args[i + 1]))?;
                tree_size = Some(parsed);
                i += 2;
            }
            "--old-size" => {
                if i + 1 >= args.len() {
                    bail!("--old-size requires a value");
                }
                let parsed = args[i + 1]
                    .parse::<i64>()
                    .with_context(|| format!("invalid --old-size value: {}", args[i + 1]))?;
                old_size = Some(parsed);
                i += 2;
            }
            other => bail!("unknown read option: {other}"),
        }
    }

    let mut payload = Map::new();
    payload.insert("kind".to_string(), Value::String(kind));
    if let Some(v) = actor_id {
        payload.insert("actor_id".to_string(), Value::String(v));
    }
    if let Some(v) = limit {
        payload.insert("limit".to_string(), json!(v));
    }
    if let Some(v) = log_index {
        payload.insert("log_index".to_string(), json!(v));
    }
    if let Some(v) = tree_size {
        payload.insert("tree_size".to_string(), json!(v));
    }
    if let Some(v) = old_size {
        payload.insert("old_size".to_string(), json!(v));
    }

    Ok(json!({
        "request_id": request_id(),
        "type": "read",
        "payload": Value::Object(payload),
    }))
}

/// High-level `audit` subcommand with ergonomic subcommands:
///   `audit checkpoint`
///   `audit proof LOG_INDEX [--tree-size N]`
///   `audit consistency OLD_SIZE NEW_SIZE`
fn build_audit_request(args: Vec<String>) -> Result<Value> {
    if args.is_empty() {
        bail!(
            "usage: punkgo-cli audit <subcommand>\nsubcommands:\n  checkpoint\n  proof <log-index> [--tree-size <n>]\n  consistency <old-size> <new-size>"
        );
    }
    match args[0].as_str() {
        "checkpoint" => {
            let payload = json!({ "kind": "audit_checkpoint" });
            Ok(json!({
                "request_id": request_id(),
                "type": "read",
                "payload": payload
            }))
        }
        "proof" => {
            if args.len() < 2 {
                bail!("usage: punkgo-cli audit proof <log-index> [--tree-size <n>]");
            }
            let log_index = args[1]
                .parse::<i64>()
                .with_context(|| format!("invalid log-index: {}", args[1]))?;
            let mut tree_size: Option<i64> = None;
            let mut i = 2usize;
            while i < args.len() {
                if args[i] == "--tree-size" {
                    if i + 1 >= args.len() {
                        bail!("--tree-size requires a value");
                    }
                    tree_size = Some(
                        args[i + 1]
                            .parse::<i64>()
                            .with_context(|| format!("invalid --tree-size: {}", args[i + 1]))?,
                    );
                    i += 2;
                } else {
                    bail!("unknown option: {}", args[i]);
                }
            }
            let mut payload = json!({
                "kind": "audit_inclusion_proof",
                "log_index": log_index
            });
            if let Some(s) = tree_size {
                payload["tree_size"] = json!(s);
            }
            Ok(json!({
                "request_id": request_id(),
                "type": "read",
                "payload": payload
            }))
        }
        "consistency" => {
            if args.len() < 3 {
                bail!("usage: punkgo-cli audit consistency <old-size> <new-size>");
            }
            let old_size = args[1]
                .parse::<i64>()
                .with_context(|| format!("invalid old-size: {}", args[1]))?;
            let new_size = args[2]
                .parse::<i64>()
                .with_context(|| format!("invalid new-size: {}", args[2]))?;
            Ok(json!({
                "request_id": request_id(),
                "type": "read",
                "payload": {
                    "kind": "audit_consistency_proof",
                    "old_size": old_size,
                    "tree_size": new_size
                }
            }))
        }
        other => bail!("unknown audit subcommand: {other}"),
    }
}

fn build_action_request(request_type: &str, args: Vec<String>) -> Result<Value> {
    if args.len() != 1 {
        bail!("usage: punkgo-cli {request_type} '<action-json>'");
    }
    let payload: Value = serde_json::from_str(&args[0])
        .with_context(|| format!("invalid action JSON: {}", args[0]))?;

    Ok(json!({
        "request_id": request_id(),
        "type": request_type,
        "payload": payload
    }))
}

fn build_send_request(args: Vec<String>) -> Result<Value> {
    if args.len() != 2 {
        bail!("usage: punkgo-cli send <quote|submit|read> '<payload-json>'");
    }
    let request_type = args[0].as_str();
    if !matches!(request_type, "quote" | "submit" | "read") {
        bail!("invalid request type: {request_type}");
    }

    let payload: Value = serde_json::from_str(&args[1])
        .with_context(|| format!("invalid payload JSON: {}", args[1]))?;

    Ok(json!({
        "request_id": request_id(),
        "type": request_type,
        "payload": payload
    }))
}

fn build_seed_actor_request(args: Vec<String>) -> Result<Value> {
    if args.is_empty() {
        bail!("usage: punkgo-cli seed-actor <new-actor-id> [--energy <n>] [--as <operator-actor>]");
    }
    let new_actor_id = args[0].clone();
    if new_actor_id.trim().is_empty() {
        bail!("new actor id cannot be empty");
    }

    let mut energy_balance: i64 = 1000;
    let mut operator_actor = "root".to_string();

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--energy" => {
                if i + 1 >= args.len() {
                    bail!("--energy requires a value");
                }
                let parsed = args[i + 1]
                    .parse::<i64>()
                    .with_context(|| format!("invalid --energy value: {}", args[i + 1]))?;
                if parsed < 0 {
                    bail!("--energy must be >= 0");
                }
                energy_balance = parsed;
                i += 2;
            }
            "--as" => {
                if i + 1 >= args.len() {
                    bail!("--as requires a value");
                }
                operator_actor = args[i + 1].clone();
                i += 2;
            }
            other => bail!("unknown seed-actor option: {other}"),
        }
    }

    let action = json!({
        "actor_id": operator_actor,
        "action_type": "create",
        "target": "ledger/actor",
        "payload": {
            "actor_id": new_actor_id,
            "energy_balance": energy_balance
        }
    });

    Ok(json!({
        "request_id": request_id(),
        "type": "submit",
        "payload": action
    }))
}

fn build_raw_request(args: Vec<String>) -> Result<Value> {
    if args.len() != 1 {
        bail!("usage: punkgo-cli raw '<request-json>'");
    }
    let mut request: Value = serde_json::from_str(&args[0])
        .with_context(|| format!("invalid request JSON: {}", args[0]))?;

    let obj = request
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("request JSON must be an object"))?;
    if !obj.contains_key("request_id") {
        obj.insert("request_id".to_string(), Value::String(request_id()));
    }
    if !obj.contains_key("type") {
        bail!("raw request missing required field: type");
    }
    if !obj.contains_key("payload") {
        obj.insert("payload".to_string(), json!({}));
    }

    Ok(request)
}

fn send_request(endpoint: &str, request_line: &str) -> Result<String> {
    use interprocess::local_socket::{
        GenericFilePath, GenericNamespaced, Name, Stream, ToFsName, ToNsName, traits::Stream as _,
    };

    fn endpoint_to_name(endpoint: &str) -> std::io::Result<Name<'_>> {
        if endpoint.contains('/') || endpoint.contains('\\') {
            endpoint.to_fs_name::<GenericFilePath>()
        } else {
            endpoint.to_ns_name::<GenericNamespaced>()
        }
    }

    let name =
        endpoint_to_name(endpoint).with_context(|| format!("invalid endpoint name: {endpoint}"))?;
    let mut stream =
        Stream::connect(name).with_context(|| format!("connect to {endpoint} failed"))?;
    stream.write_all(request_line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut resp = String::new();
    let n = reader.read_line(&mut resp)?;
    if n == 0 {
        bail!("empty response from kernel");
    }
    Ok(resp.trim().to_string())
}

fn default_endpoint() -> String {
    "punkgo-kernel".to_string()
}

fn request_id() -> String {
    Uuid::new_v4().to_string()
}

fn print_usage() {
    println!("punkgo-cli - interact with punkgo-kerneld");
    println!();
    println!("Usage:");
    println!("  punkgo-cli [--endpoint <pipe-or-socket>] <command> ...");
    println!();
    println!("Commands:");
    println!(
        "  read <kind> [--actor-id <id>] [--limit <n>] [--log-index <n>] [--tree-size <n>] [--old-size <n>]"
    );
    println!("  quote '<action-json>'");
    println!("  submit '<action-json>'");
    println!("  seed-actor <new-actor-id> [--energy <n>] [--as <operator-actor>]");
    println!("  audit checkpoint");
    println!("  audit proof <log-index> [--tree-size <n>]");
    println!("  audit consistency <old-size> <new-size>");
    println!("  send <quote|submit|read> '<payload-json>'");
    println!("  raw '<full-request-json>'");
    println!();
    println!("Read kinds:");
    println!("  health, actor_energy, events, stats, snapshot, paths");
    println!("  audit_checkpoint, audit_inclusion_proof, audit_consistency_proof");
    println!();
    println!("Examples:");
    println!("  punkgo-cli read health");
    println!("  punkgo-cli read actor_energy --actor-id root");
    println!("  punkgo-cli seed-actor alice --energy 5000");
    println!("  punkgo-cli audit checkpoint");
    println!("  punkgo-cli audit proof 0");
    println!("  punkgo-cli audit proof 0 --tree-size 3");
    println!("  punkgo-cli audit consistency 1 3");
    println!("  punkgo-cli read audit_inclusion_proof --log-index 0");
    println!(
        "  punkgo-cli quote '{{\"actor_id\":\"root\",\"action_type\":\"mutate\",\"target\":\"workspace/a\",\"payload\":{{\"k\":\"v\"}}}}'"
    );
    println!(
        "  punkgo-cli submit '{{\"actor_id\":\"root\",\"action_type\":\"execute\",\"target\":\"workspace/a\",\"payload\":{{\"command\":\"cmd\",\"args\":[\"/c\",\"echo hi\"]}}}}'"
    );
}
