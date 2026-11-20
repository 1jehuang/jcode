use anyhow::Result;
use serde::Serialize;

pub async fn run_quota_command(emit_json: bool) -> Result<()> {
    let client = reqwest::Client::new();
    let coding = crate::auth::zcode::fetch_coding_plan_quota(&client).await;
    let billing = crate::auth::zcode::fetch_billing_summary(&client).await;

    if emit_json {
        #[derive(Serialize)]
        struct QuotaJson {
            coding_plan: Option<serde_json::Value>,
            coding_plan_error: Option<String>,
            billing: Option<serde_json::Value>,
            billing_error: Option<String>,
        }
        let coding_val = coding.as_ref().ok().map(|p| {
            serde_json::json!({
                "level": p.level,
                "limits": p.limits.iter().map(|l| serde_json::json!({
                    "type": l.r#type,
                    "unit": l.unit,
                    "total": l.total,
                    "usage": l.usage,
                    "remaining": l.remaining,
                    "percentage": l.percentage,
                    "next_reset_time": l.next_reset_time,
                })).collect::<Vec<_>>(),
            })
        });
        let out = QuotaJson {
            coding_plan: coding_val,
            coding_plan_error: coding.as_ref().err().map(|e| format!("{e:#}")),
            billing: billing.as_ref().ok().cloned(),
            billing_error: billing.as_ref().err().map(|e| format!("{e:#}")),
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    match coding {
        Ok(plan) => {
            println!(
                "ZCode Coding Plan (level: {})",
                plan.level.as_deref().unwrap_or("unknown")
            );
            if plan.limits.is_empty() {
                println!("  (no usage windows reported)");
            } else {
                for l in &plan.limits {
                    let rem = l
                        .remaining
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "-".to_string());
                    let pct = l
                        .percentage
                        .map(|v| format!(" ({v:.0}% used)"))
                        .unwrap_or_default();
                    println!("  - {}: remaining {rem}{pct}", l.r#type);
                }
            }
        }
        Err(err) => println!("ZCode Coding Plan quota unavailable: {err:#}"),
    }

    match billing {
        Ok(summary) => {
            if let Some(plans) = summary.pointer("/current/data/plans").and_then(|v| v.as_array()) {
                println!("Active ZCode Plans ({}):", plans.len());
                for p in plans {
                    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let id = p.get("plan_id").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("  - {name} ({id})");
                }
            }
            if let Some(balances) = summary
                .pointer("/balance/data/balances")
                .and_then(|v| v.as_array())
            {
                println!("Credit Balances ({}):", balances.len());
                for b in balances {
                    let name = b.get("show_name").and_then(|v| v.as_str()).unwrap_or("?");
                    let rem = b
                        .get("remaining_units")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let tot = b
                        .get("total_units")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    println!("  - {name}: {rem:.0} / {tot:.0}");
                }
            }
        }
        Err(err) => println!("ZCode billing summary unavailable: {err:#}"),
    }

    Ok(())
}

pub async fn run_claim_command(
    plan_id: Option<String>,
    list_only: bool,
    emit_json: bool,
) -> Result<()> {
    let client = reqwest::Client::new();
    if let Some(id) = plan_id {
        let outcome = crate::auth::zcode_claim::claim_plan(&client, &id).await;
        if emit_json {
            let val = match &outcome {
                crate::auth::zcode_claim::ClaimOutcome::Ok {
                    plan_id,
                    starts_at,
                    ends_at,
                } => serde_json::json!({
                    "ok": true,
                    "plan_id": plan_id,
                    "starts_at": starts_at,
                    "ends_at": ends_at,
                }),
                crate::auth::zcode_claim::ClaimOutcome::Failed {
                    plan_id,
                    kind,
                    code,
                    message,
                } => serde_json::json!({
                    "ok": false,
                    "plan_id": plan_id,
                    "kind": kind.to_string(),
                    "code": code,
                    "message": message,
                }),
            };
            println!("{}", serde_json::to_string_pretty(&val)?);
        } else {
            match outcome {
                crate::auth::zcode_claim::ClaimOutcome::Ok { plan_id, .. } => {
                    println!("Claimed ZCode plan: {plan_id}");
                }
                crate::auth::zcode_claim::ClaimOutcome::Failed {
                    plan_id,
                    kind,
                    code,
                    message,
                } => {
                    println!("Failed to claim {plan_id} ({kind}, code {code}): {message}");
                }
            }
        }
        return Ok(());
    }

    let plans = crate::auth::zcode_claim::list_claimable_plans(&client).await?;
    if list_only || plans.is_empty() {
        if emit_json {
            let val: Vec<_> = plans
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "plan_id": p.plan_id,
                        "name": p.name,
                        "description": p.description,
                        "starts_at": p.starts_at,
                        "ends_at": p.ends_at,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&val)?);
        } else if plans.is_empty() {
            println!("No claimable ZCode plans available right now.");
        } else {
            println!("Claimable ZCode plans ({}):", plans.len());
            for p in &plans {
                println!("  - {} ({})", p.name, p.plan_id);
            }
        }
        return Ok(());
    }

    let mut results = Vec::new();
    for p in &plans {
        let outcome = crate::auth::zcode_claim::claim_plan(&client, &p.plan_id).await;
        results.push(outcome);
    }
    if emit_json {
        let val: Vec<_> = results
            .iter()
            .map(|o| match o {
                crate::auth::zcode_claim::ClaimOutcome::Ok {
                    plan_id,
                    starts_at,
                    ends_at,
                } => serde_json::json!({
                    "ok": true,
                    "plan_id": plan_id,
                    "starts_at": starts_at,
                    "ends_at": ends_at,
                }),
                crate::auth::zcode_claim::ClaimOutcome::Failed {
                    plan_id,
                    kind,
                    code,
                    message,
                } => serde_json::json!({
                    "ok": false,
                    "plan_id": plan_id,
                    "kind": kind.to_string(),
                    "code": code,
                    "message": message,
                }),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&val)?);
    } else {
        for o in results {
            match o {
                crate::auth::zcode_claim::ClaimOutcome::Ok { plan_id, .. } => {
                    println!("Claimed ZCode plan: {plan_id}");
                }
                crate::auth::zcode_claim::ClaimOutcome::Failed {
                    plan_id,
                    kind,
                    code,
                    message,
                } => {
                    println!("Failed to claim {plan_id} ({kind}, code {code}): {message}");
                }
            }
        }
    }
    Ok(())
}

pub async fn run_offpeak_command(emit_json: bool) -> Result<()> {
    let Some(creds) = crate::auth::zcode_offpeak::OffPeakCredentials::from_credentials()
    else {
        anyhow::bail!("ZCode off-peak requires ZCode login (jcode login --provider zcode)");
    };
    let client = reqwest::Client::new();
    let avail = crate::auth::zcode_offpeak::availability(&client, &creds).await?;
    if emit_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "can_take_number": avail.can_take_number,
                "next_take_at": avail.next_take_at,
            }))?
        );
    } else if avail.can_take_number {
        println!("ZCode off-peak queue: available (can take ticket now)");
    } else {
        println!(
            "ZCode off-peak queue: unavailable (next_take_at: {:?})",
            avail.next_take_at
        );
    }
    Ok(())
}
