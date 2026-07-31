use super::*;

pub(crate) fn usage_for_packets(packets: &[KernelPacket]) -> BudgetUsage {
    let mut usage = BudgetUsage::default();

    for packet in packets {
        let body_bytes = estimate_json_bytes(&packet.body);
        usage.tokens = usage.tokens.saturating_add(
            packet
                .token_usage
                .unwrap_or_else(|| estimate_tokens(body_bytes)),
        );
        usage.bytes = usage.bytes.saturating_add(body_bytes);
        usage.runtime_ms = usage
            .runtime_ms
            .saturating_add(packet.runtime_ms.unwrap_or(0));
    }

    usage
}

pub(crate) fn enforce_budget(
    target: &str,
    stage: BudgetStage,
    budget: ExecutionBudget,
    usage: BudgetUsage,
) -> Result<(), KernelError> {
    if let Some(cap) = budget.token_cap {
        if usage.tokens > cap {
            return Err(KernelError::BudgetExceeded {
                target: target.to_string(),
                stage,
                metric: BudgetMetric::Tokens,
                used: usage.tokens,
                cap,
            });
        }
    }

    if let Some(cap) = budget.byte_cap {
        if usage.bytes > cap {
            return Err(KernelError::BudgetExceeded {
                target: target.to_string(),
                stage,
                metric: BudgetMetric::Bytes,
                used: usage.bytes as u64,
                cap: cap as u64,
            });
        }
    }

    if let Some(cap) = budget.runtime_ms_cap {
        if usage.runtime_ms > cap {
            return Err(KernelError::BudgetExceeded {
                target: target.to_string(),
                stage,
                metric: BudgetMetric::RuntimeMs,
                used: usage.runtime_ms,
                cap,
            });
        }
    }

    Ok(())
}

pub(crate) fn default_packet_format() -> String {
    "packet-json".to_string()
}

pub(crate) fn default_cache_input(target: &str, request: &KernelRequest) -> Value {
    json!({
        "target": target,
        "input_packets": request.input_packets,
        "budget": request.budget,
        "policy_context": request.policy_context,
        "reducer_input": request.reducer_input,
    })
}

fn estimate_json_bytes(value: &Value) -> usize {
    serde_json::to_string(value)
        .map(|text| text.len())
        .unwrap_or(0)
}
