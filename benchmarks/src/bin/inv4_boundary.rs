use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let (kernel, _state) = bootstrap_kernel("bench-inv4").await;

    // Create agent with narrow writable_targets: workspace/docs/* mutate only
    create_agent(
        &kernel,
        "docs-agent",
        1_000_000,
        json!([{"target": "workspace/docs/*", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope(
        &kernel,
        "docs-agent",
        500_000,
        json!(["workspace/docs/*"]),
        json!(["mutate"]),
        None,
        None,
    )
    .await;

    let mut test_cases: Vec<serde_json::Value> = Vec::new();

    // Case a: Observe on any target — ALLOW (observe is always exempt)
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "docs-agent",
                ActionType::Observe,
                "system/anything",
                json!({}),
            ),
        )
        .await;
        let pass = resp.status == "ok";
        test_cases.push(json!({
            "id": "a",
            "description": "Observe on any target (observe exempt from boundary)",
            "action": "observe system/anything",
            "expected": "allow",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    // Case b: Mutate within boundary — ALLOW
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "docs-agent",
                ActionType::Mutate,
                "workspace/docs/readme",
                json!({"content": "hello"}),
            ),
        )
        .await;
        let pass = resp.status == "ok";
        test_cases.push(json!({
            "id": "b",
            "description": "Mutate within boundary (workspace/docs/*)",
            "action": "mutate workspace/docs/readme",
            "expected": "allow",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    // Case c: Mutate outside boundary — DENY
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "docs-agent",
                ActionType::Mutate,
                "workspace/code/main.rs",
                json!({"content": "hack"}),
            ),
        )
        .await;
        let pass = resp.status == "error";
        test_cases.push(json!({
            "id": "c",
            "description": "Mutate outside boundary (workspace/code/*)",
            "action": "mutate workspace/code/main.rs",
            "expected": "deny",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    // Case d: Execute not in actions list — DENY
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "docs-agent",
                ActionType::Execute,
                "workspace/docs/script",
                json!({"command": "echo hi"}),
            ),
        )
        .await;
        let pass = resp.status == "error";
        test_cases.push(json!({
            "id": "d",
            "description": "Execute action not in writable_targets actions list",
            "action": "execute workspace/docs/script",
            "expected": "deny",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    // Case e: Privileged target (system/) — DENY
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "docs-agent",
                ActionType::Create,
                "system/config",
                json!({"key": "val"}),
            ),
        )
        .await;
        let pass = resp.status == "error";
        test_cases.push(json!({
            "id": "e",
            "description": "Create on privileged target (system/*)",
            "action": "create system/config",
            "expected": "deny",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    // Case f: Privileged target (ledger/) — DENY
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "docs-agent",
                ActionType::Create,
                "ledger/something",
                json!({"key": "val"}),
            ),
        )
        .await;
        let pass = resp.status == "error";
        test_cases.push(json!({
            "id": "f",
            "description": "Create on privileged target (ledger/*)",
            "action": "create ledger/something",
            "expected": "deny",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    // Case g: Root retains full access — ALLOW
    {
        let resp = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Mutate,
                "workspace/code/main.rs",
                json!({"root": true}),
            ),
        )
        .await;
        let pass = resp.status == "ok";
        test_cases.push(json!({
            "id": "g",
            "description": "Root retains full access to all targets",
            "action": "mutate workspace/code/main.rs (as root)",
            "expected": "allow",
            "actual": if resp.status == "ok" { "allow" } else { "deny" },
            "pass": pass
        }));
    }

    let all_passed = test_cases
        .iter()
        .all(|tc| tc["pass"].as_bool().unwrap_or(false));
    let elapsed_ms = start.elapsed().as_millis();

    let result = json!({
        "test": "INV-4 Boundary Enforcement",
        "status": if all_passed { "PASS" } else { "FAIL" },
        "test_cases": test_cases,
        "all_passed": all_passed,
        "timing_ms": elapsed_ms,
        "notes": "Observe is always exempt from boundary checks (PIP-001). Privileged targets (system/*, ledger/*) require root wildcard. Default deny: if no matching pattern in writable_targets, action is rejected."
    });

    write_result("inv4_boundary.json", &result);
    println!("INV-4: {}", if all_passed { "PASS" } else { "FAIL" });
}
