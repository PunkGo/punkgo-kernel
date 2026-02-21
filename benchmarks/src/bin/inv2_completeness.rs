use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let (kernel, _state) = bootstrap_kernel("bench-inv2").await;

    // Setup: create agent + envelope
    create_agent(
        &kernel,
        "comp-agent",
        1_000_000,
        json!([{"target": "workspace/**", "actions": ["create", "mutate"]}]),
    )
    .await;

    grant_envelope(
        &kernel,
        "comp-agent",
        500_000,
        json!(["workspace/**"]),
        json!(["create", "mutate"]),
        Some(json!([{"target": "workspace/sensitive/*", "actions": ["mutate"]}])),
        None,
    )
    .await;

    // Get event count before test actions
    let stats_before = read_ok(&kernel, json!({"kind": "stats"})).await;
    let events_before = stats_before["event_count"].as_i64().unwrap();

    // Track submitted actions and results
    let mut submitted: Vec<serde_json::Value> = Vec::new();
    let mut successful = 0i64;
    let mut failed_boundary = 0i64;

    // 1. Observe actions (root) — 5 (always succeed, no event for observe? let's check)
    for i in 0..5 {
        let resp = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Observe,
                &format!("workspace/obs/{i}"),
                json!({}),
            ),
        )
        .await;
        let ok = resp.status == "ok";
        if ok {
            successful += 1;
        }
        submitted
            .push(json!({"type": "observe", "target": format!("workspace/obs/{i}"), "ok": ok}));
    }

    // 2. Create actions (root creates sub-objects) — 5
    for i in 0..5 {
        let resp = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Create,
                &format!("workspace/create/{i}"),
                json!({"data": i}),
            ),
        )
        .await;
        let ok = resp.status == "ok";
        if ok {
            successful += 1;
        }
        submitted
            .push(json!({"type": "create", "target": format!("workspace/create/{i}"), "ok": ok}));
    }

    // 3. Mutate actions (agent, within boundary) — 5
    for i in 0..5 {
        let resp = submit_raw(
            &kernel,
            make_action(
                "comp-agent",
                ActionType::Mutate,
                &format!("workspace/allowed/{i}"),
                json!({"val": i}),
            ),
        )
        .await;
        let ok = resp.status == "ok";
        if ok {
            successful += 1;
        }
        submitted
            .push(json!({"type": "mutate", "target": format!("workspace/allowed/{i}"), "ok": ok}));
    }

    // 4. Mutate actions (agent, OUT of boundary) — 5 (should fail)
    for i in 0..5 {
        let resp = submit_raw(
            &kernel,
            make_action(
                "comp-agent",
                ActionType::Mutate,
                &format!("system/forbidden/{i}"),
                json!({"val": i}),
            ),
        )
        .await;
        let ok = resp.status == "ok";
        if !ok {
            failed_boundary += 1;
        }
        submitted.push(
            json!({"type": "mutate_oob", "target": format!("system/forbidden/{i}"), "ok": ok}),
        );
    }

    // 5. Hold test: trigger hold
    let hold_resp = submit_raw(
        &kernel,
        make_action(
            "comp-agent",
            ActionType::Mutate,
            "workspace/sensitive/data",
            json!({"hold": true}),
        ),
    )
    .await;
    let hold_triggered =
        hold_resp.status == "error" && hold_resp.payload.to_string().contains("hold triggered");
    let hold_id = if hold_triggered {
        extract_hold_id(&hold_resp.payload)
    } else {
        String::new()
    };

    // 6. Reject the hold
    let hold_response_ok = if hold_triggered {
        let resp = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Mutate,
                &format!("ledger/hold/{hold_id}"),
                json!({"decision": "reject"}),
            ),
        )
        .await;
        resp.status == "ok"
    } else {
        false
    };

    // Query all events
    let events_resp = read_ok(&kernel, json!({"kind": "events", "limit": 500})).await;
    let events = events_resp["events"].as_array().unwrap();
    let total_events = events.len() as i64;

    // Check for hold_request and hold_response events
    let event_types: Vec<&str> = events
        .iter()
        .filter_map(|e| e["action_type"].as_str())
        .collect();
    let hold_request_recorded = event_types.contains(&"hold_request");
    let hold_response_recorded = event_types.contains(&"hold_response");

    // Count events that correspond to our test actions (exclude setup events)
    let new_events = total_events - events_before;

    let elapsed_ms = start.elapsed().as_millis();

    let result = json!({
        "test": "INV-2 Completeness",
        "status": if hold_request_recorded && hold_response_recorded { "PASS" } else { "PARTIAL" },
        "actions_submitted": submitted.len(),
        "successful_actions": successful,
        "failed_boundary_violations": failed_boundary,
        "events_before_test": events_before,
        "events_after_test": total_events,
        "new_events": new_events,
        "event_types_in_log": event_types,
        "hold_test": {
            "hold_triggered": hold_triggered,
            "hold_request_recorded": hold_request_recorded,
            "hold_response_recorded": hold_response_recorded,
            "hold_response_ok": hold_response_ok,
        },
        "submitted_actions": submitted,
        "timing_ms": elapsed_ms,
        "notes": "Boundary violations (system/* by non-root) are rejected BEFORE the append step, so they produce no event. This is correct: only actions that pass validation enter the pipeline and get recorded. Hold events (hold_request, hold_response) are always recorded."
    });

    write_result("inv2_completeness.json", &result);
    println!("INV-2: {}", result["status"]);
}
