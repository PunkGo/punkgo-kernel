use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let start = Instant::now();

    // ========== Part 1: Energy exhaustion test ==========
    let (kernel, _state) = bootstrap_kernel("bench-inv5").await;

    // Create agent with exactly 100 energy
    create_agent(
        &kernel,
        "energy-agent",
        100,
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope(
        &kernel,
        "energy-agent",
        100,
        json!(["workspace/**"]),
        json!(["mutate"]),
        None,
        None,
    )
    .await;

    let (initial_balance, _) = query_energy(&kernel, "energy-agent").await;

    // Submit Mutate actions (cost=15 each) until InsufficientEnergy
    let mut actions_log: Vec<serde_json::Value> = Vec::new();
    let mut successful_mutates = 0;
    let mut insufficient_triggered = false;

    for i in 0..20 {
        let (balance_before, _reserved_before) = query_energy(&kernel, "energy-agent").await;
        let resp = submit_raw(
            &kernel,
            make_action(
                "energy-agent",
                ActionType::Mutate,
                &format!("workspace/e/{i}"),
                json!({"i": i}),
            ),
        )
        .await;

        let ok = resp.status == "ok";
        let (balance_after, _reserved_after) = query_energy(&kernel, "energy-agent").await;

        if ok {
            successful_mutates += 1;
        } else {
            insufficient_triggered = true;
            actions_log.push(json!({
                "seq": i,
                "type": "mutate",
                "cost": 15,
                "balance_before": balance_before,
                "balance_after": balance_after,
                "success": false,
                "error": "InsufficientEnergy"
            }));
            break;
        }

        actions_log.push(json!({
            "seq": i,
            "type": "mutate",
            "cost": 15,
            "balance_before": balance_before,
            "balance_after": balance_after,
            "success": true
        }));
    }

    let (final_balance, _final_reserved) = query_energy(&kernel, "energy-agent").await;
    let balance_never_negative = final_balance >= 0;
    let expected_final = initial_balance - (successful_mutates * 15);

    // ========== Part 2: Hold commitment cost test ==========
    let (kernel2, _state2) = bootstrap_kernel("bench-inv5-hold").await;

    create_agent(
        &kernel2,
        "hold-energy-agent",
        1000,
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope(
        &kernel2,
        "hold-energy-agent",
        1000,
        json!(["workspace/**"]),
        json!(["mutate"]),
        Some(json!([{"target": "workspace/gated/*", "actions": ["mutate"]}])),
        None,
    )
    .await;

    let (hold_balance_before, _) = query_energy(&kernel2, "hold-energy-agent").await;

    // Trigger hold
    let hold_resp = submit_raw(
        &kernel2,
        make_action(
            "hold-energy-agent",
            ActionType::Mutate,
            "workspace/gated/file",
            json!({"data": 1}),
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

    let (_, hold_reserved) = query_energy(&kernel2, "hold-energy-agent").await;

    // Reject hold → 20% commitment cost
    let hold_reject_ok = if hold_triggered {
        let resp = submit_raw(
            &kernel2,
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

    let (hold_balance_after, hold_reserved_after) =
        query_energy(&kernel2, "hold-energy-agent").await;

    // Expected: commitment_cost = ceil(reserved * 0.2)
    let expected_commitment_cost = ((hold_reserved as f64) * 0.2).ceil() as i64;
    let actual_deducted = hold_balance_before - hold_balance_after;
    let commitment_cost_correct = actual_deducted == expected_commitment_cost;

    let elapsed_ms = start.elapsed().as_millis();

    let all_pass = insufficient_triggered
        && balance_never_negative
        && hold_triggered
        && hold_reject_ok
        && commitment_cost_correct
        && hold_reserved_after == 0;

    let result = json!({
        "test": "INV-5 Energy Conservation",
        "status": if all_pass { "PASS" } else { "FAIL" },
        "exhaustion_test": {
            "envelope_budget": 100,
            "initial_balance": initial_balance,
            "cost_per_mutate": 15,
            "successful_mutates": successful_mutates,
            "total_consumed": successful_mutates * 15,
            "final_balance": final_balance,
            "expected_final_balance": expected_final,
            "balance_matches": final_balance == expected_final,
            "insufficient_energy_enforced": insufficient_triggered,
            "balance_never_negative": balance_never_negative,
            "actions": actions_log
        },
        "hold_commitment_test": {
            "initial_balance": hold_balance_before,
            "hold_triggered": hold_triggered,
            "reserved_energy": hold_reserved,
            "hold_rejected": hold_reject_ok,
            "expected_commitment_cost": expected_commitment_cost,
            "actual_deducted": actual_deducted,
            "commitment_cost_correct": commitment_cost_correct,
            "remaining_balance": hold_balance_after,
            "reserved_after_reject": hold_reserved_after,
        },
        "timing_ms": elapsed_ms,
        "notes": "Energy balance never goes negative. InsufficientEnergy is enforced at the reserve step. Hold reject charges 20% commitment cost (ceil rounding)."
    });

    write_result("inv5_energy.json", &result);
    println!("INV-5: {}", if all_pass { "PASS" } else { "FAIL" });
}
