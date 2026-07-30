use std::collections::BTreeMap;
use std::fs;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{civil_from_unix_day, csv_cell, format_ymd, pct_saved, resolve_root, CcEconomicsArgs};
use crate::savings_analytics::{load_run_savings, RunSavingsRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum EconomicsGranularity {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Debug, Deserialize)]
struct CcusageMetrics {
    #[serde(rename = "inputTokens")]
    input_tokens: u64,
    #[serde(rename = "outputTokens")]
    output_tokens: u64,
    #[serde(rename = "cacheCreationTokens", default)]
    cache_creation_tokens: u64,
    #[serde(rename = "cacheReadTokens", default)]
    cache_read_tokens: u64,
    #[serde(rename = "totalTokens")]
    total_tokens: u64,
    #[serde(rename = "totalCost")]
    total_cost: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct EconomicsCcusage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    total_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
struct EconomicsPeriod {
    granularity: EconomicsGranularity,
    period: String,
    ccusage: EconomicsCcusage,
    packet28_commands: usize,
    packet28_saved_tokens: u64,
    packet28_savings_pct: f64,
    weighted_input_cost_per_token: Option<f64>,
    estimated_savings_usd: Option<f64>,
}

#[derive(Debug, Serialize, Default)]
struct EconomicsTotals {
    cc_cost_usd: f64,
    cc_total_tokens: u64,
    cc_active_tokens: u64,
    packet28_commands: usize,
    packet28_saved_tokens: u64,
    estimated_savings_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
struct EconomicsReport {
    source: String,
    periods: Vec<EconomicsPeriod>,
    totals: EconomicsTotals,
}

pub fn run_cc_economics(args: CcEconomicsArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    let granularities = economics_granularities(&args);
    let ccusage_source = load_ccusage_source(&args)?;
    let run_savings = load_run_savings(&root, args.limit)?;
    let report = build_economics_report(&ccusage_source, &run_savings, &granularities);
    let format = args.format.trim().to_ascii_lowercase();
    if args.json || format == "json" {
        crate::cmd_common::emit_json(&serde_json::to_value(report)?, args.pretty)?;
    } else if format == "csv" {
        print_economics_csv(&report);
    } else {
        print_economics_text(&report);
    }
    Ok(0)
}

fn economics_granularities(args: &CcEconomicsArgs) -> Vec<EconomicsGranularity> {
    if args.all {
        return vec![
            EconomicsGranularity::Daily,
            EconomicsGranularity::Weekly,
            EconomicsGranularity::Monthly,
        ];
    }
    let mut items = Vec::new();
    if args.daily {
        items.push(EconomicsGranularity::Daily);
    }
    if args.weekly {
        items.push(EconomicsGranularity::Weekly);
    }
    if args.monthly {
        items.push(EconomicsGranularity::Monthly);
    }
    if items.is_empty() {
        items.push(EconomicsGranularity::Monthly);
    }
    items
}

fn load_ccusage_source(args: &CcEconomicsArgs) -> Result<(String, Value)> {
    if let Some(path) = args.ccusage_json.as_deref() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read ccusage JSON '{path}'"))?;
        let value = serde_json::from_str(&content).context("invalid ccusage JSON")?;
        return Ok((path.to_string(), value));
    }
    let mut merged = serde_json::Map::new();
    let mut loaded_any = false;
    for granularity in economics_granularities(args) {
        let Some(output) = run_ccusage_command(&[granularity])? else {
            continue;
        };
        let value: Value = serde_json::from_str(&output).context("invalid ccusage JSON")?;
        let key = match granularity {
            EconomicsGranularity::Daily => "daily",
            EconomicsGranularity::Weekly => "weekly",
            EconomicsGranularity::Monthly => "monthly",
        };
        if let Some(items) = value.get(key).cloned() {
            merged.insert(key.to_string(), items);
            loaded_any = true;
        }
    }
    if loaded_any {
        Ok(("ccusage".to_string(), Value::Object(merged)))
    } else {
        Ok(("unavailable".to_string(), json!({})))
    }
}

fn run_ccusage_command(granularities: &[EconomicsGranularity]) -> Result<Option<String>> {
    let granularity = granularities
        .first()
        .copied()
        .unwrap_or(EconomicsGranularity::Monthly);
    let subcommand = match granularity {
        EconomicsGranularity::Daily => "daily",
        EconomicsGranularity::Weekly => "weekly",
        EconomicsGranularity::Monthly => "monthly",
    };
    for mut command in candidate_ccusage_commands(subcommand) {
        let output = match command.output() {
            Ok(output) => output,
            Err(_) => continue,
        };
        if output.status.success() {
            return Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()));
        }
    }
    Ok(None)
}

fn candidate_ccusage_commands(subcommand: &str) -> Vec<Command> {
    let mut direct = Command::new("ccusage");
    direct
        .arg(subcommand)
        .arg("--json")
        .arg("--since")
        .arg("20250101")
        .stderr(Stdio::null());
    let mut npx = Command::new("npx");
    npx.arg("--yes")
        .arg("ccusage")
        .arg(subcommand)
        .arg("--json")
        .arg("--since")
        .arg("20250101")
        .stderr(Stdio::null());
    vec![direct, npx]
}

fn build_economics_report(
    ccusage_source: &(String, Value),
    run_savings: &[RunSavingsRecord],
    granularities: &[EconomicsGranularity],
) -> EconomicsReport {
    let mut periods = Vec::new();
    for granularity in granularities {
        let mut map = BTreeMap::<String, EconomicsPeriod>::new();
        for (period, ccusage) in ccusage_periods(&ccusage_source.1, *granularity) {
            map.entry(period.clone())
                .or_insert_with(|| empty_economics_period(*granularity, &period))
                .ccusage = ccusage;
        }
        for record in run_savings {
            let period = economics_bucket_label(record.timestamp_unix_ms, *granularity);
            let entry = map
                .entry(period.clone())
                .or_insert_with(|| empty_economics_period(*granularity, &period));
            entry.packet28_commands += 1;
            entry.packet28_saved_tokens = entry.packet28_saved_tokens.saturating_add(
                record
                    .raw_est_tokens
                    .saturating_sub(record.reduced_est_tokens),
            );
        }
        for entry in map.values_mut() {
            entry.packet28_savings_pct = pct_saved(
                entry.packet28_saved_tokens + entry.ccusage.total_tokens,
                entry.ccusage.total_tokens,
            );
            if let Some(cpt) = weighted_input_cost_per_token(&entry.ccusage) {
                entry.weighted_input_cost_per_token = Some(cpt);
                entry.estimated_savings_usd = Some(entry.packet28_saved_tokens as f64 * cpt);
            }
        }
        periods.extend(map.into_values());
    }
    let totals = economics_totals(&periods);
    EconomicsReport {
        source: ccusage_source.0.clone(),
        periods,
        totals,
    }
}

fn empty_economics_period(granularity: EconomicsGranularity, period: &str) -> EconomicsPeriod {
    EconomicsPeriod {
        granularity,
        period: period.to_string(),
        ccusage: EconomicsCcusage::default(),
        packet28_commands: 0,
        packet28_saved_tokens: 0,
        packet28_savings_pct: 0.0,
        weighted_input_cost_per_token: None,
        estimated_savings_usd: None,
    }
}

fn ccusage_periods(
    value: &Value,
    granularity: EconomicsGranularity,
) -> Vec<(String, EconomicsCcusage)> {
    let key = match granularity {
        EconomicsGranularity::Daily => "daily",
        EconomicsGranularity::Weekly => "weekly",
        EconomicsGranularity::Monthly => "monthly",
    };
    let period_key = match granularity {
        EconomicsGranularity::Daily => "date",
        EconomicsGranularity::Weekly => "week",
        EconomicsGranularity::Monthly => "month",
    };
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let period = item.get(period_key)?.as_str()?.to_string();
            let metrics = serde_json::from_value::<CcusageMetrics>(item.clone()).ok()?;
            Some((
                period,
                EconomicsCcusage {
                    input_tokens: metrics.input_tokens,
                    output_tokens: metrics.output_tokens,
                    cache_creation_tokens: metrics.cache_creation_tokens,
                    cache_read_tokens: metrics.cache_read_tokens,
                    total_tokens: metrics.total_tokens,
                    total_cost_usd: metrics.total_cost,
                },
            ))
        })
        .collect()
}

fn weighted_input_cost_per_token(ccusage: &EconomicsCcusage) -> Option<f64> {
    let weighted_units = ccusage.input_tokens as f64
        + (5.0 * ccusage.output_tokens as f64)
        + (1.25 * ccusage.cache_creation_tokens as f64)
        + (0.1 * ccusage.cache_read_tokens as f64);
    (weighted_units > 0.0).then_some(ccusage.total_cost_usd / weighted_units)
}

fn economics_totals(periods: &[EconomicsPeriod]) -> EconomicsTotals {
    let mut totals = EconomicsTotals::default();
    let mut weighted_savings = 0.0;
    let mut has_weighted_savings = false;
    for period in periods {
        totals.cc_cost_usd += period.ccusage.total_cost_usd;
        totals.cc_total_tokens = totals
            .cc_total_tokens
            .saturating_add(period.ccusage.total_tokens);
        totals.cc_active_tokens = totals
            .cc_active_tokens
            .saturating_add(period.ccusage.input_tokens + period.ccusage.output_tokens);
        totals.packet28_commands += period.packet28_commands;
        totals.packet28_saved_tokens = totals
            .packet28_saved_tokens
            .saturating_add(period.packet28_saved_tokens);
        if let Some(savings) = period.estimated_savings_usd {
            weighted_savings += savings;
            has_weighted_savings = true;
        }
    }
    totals.estimated_savings_usd = has_weighted_savings.then_some(weighted_savings);
    totals
}

fn economics_bucket_label(timestamp_unix_ms: u128, kind: EconomicsGranularity) -> String {
    let day = (timestamp_unix_ms / 86_400_000) as i64;
    match kind {
        EconomicsGranularity::Daily => format_ymd(day),
        EconomicsGranularity::Weekly => {
            let weekday_monday_zero = (day + 3).rem_euclid(7);
            format_ymd(day - weekday_monday_zero)
        }
        EconomicsGranularity::Monthly => {
            let (year, month, _) = civil_from_unix_day(day);
            format!("{year:04}-{month:02}")
        }
    }
}

fn print_economics_text(report: &EconomicsReport) {
    println!("Packet28 Claude Code economics");
    println!("source={}", report.source);
    println!("cc_cost_usd={:.4}", report.totals.cc_cost_usd);
    println!("cc_total_tokens={}", report.totals.cc_total_tokens);
    println!("packet28_commands={}", report.totals.packet28_commands);
    println!(
        "packet28_saved_tokens={}",
        report.totals.packet28_saved_tokens
    );
    if let Some(value) = report.totals.estimated_savings_usd {
        println!("estimated_savings_usd={value:.4}");
    }
    for period in &report.periods {
        println!(
            "period={:?}:{} cc_cost_usd={:.4} saved_tokens={} estimated_savings_usd={}",
            period.granularity,
            period.period,
            period.ccusage.total_cost_usd,
            period.packet28_saved_tokens,
            period
                .estimated_savings_usd
                .map(|value| format!("{value:.4}"))
                .unwrap_or_else(|| "n/a".to_string())
        );
    }
}

fn print_economics_csv(report: &EconomicsReport) {
    println!(
        "granularity,period,cc_cost_usd,cc_total_tokens,packet28_commands,packet28_saved_tokens,estimated_savings_usd"
    );
    for period in &report.periods {
        println!(
            "{:?},{},{:.6},{},{},{},{}",
            period.granularity,
            csv_cell(&period.period),
            period.ccusage.total_cost_usd,
            period.ccusage.total_tokens,
            period.packet28_commands,
            period.packet28_saved_tokens,
            period
                .estimated_savings_usd
                .map(|value| format!("{value:.6}"))
                .unwrap_or_default()
        );
    }
}
