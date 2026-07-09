use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use super::{
    civil_from_unix_day, csv_cell, format_ymd, load_task_states, pct_saved, resolve_root,
    AnalyticsArgs,
};
use crate::memory_store::{record_feedback_with_metadata, FeedbackInput, FeedbackRecord};
use crate::savings_analytics::{load_run_savings, reset_run_savings, RunSavingsRecord};

#[derive(Debug, Serialize, Default)]
struct GainSummary {
    task_count: usize,
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_pct: f64,
    by_route: BTreeMap<String, usize>,
    route_roi: BTreeMap<String, GainRouteRoi>,
    remembered_failure_advice_count: usize,
}

#[derive(Debug, Serialize, Default)]
struct GainRouteRoi {
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_pct: f64,
    avg_saved_tokens: f64,
    downstream_followup_count: usize,
    avg_followups_per_invocation: f64,
}

#[derive(Debug, Serialize, Default)]
struct GainBucket {
    period: String,
    invocation_count: usize,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
    saved_est_tokens: u64,
    savings_pct: f64,
}

pub fn run_gain_command(args: AnalyticsArgs) -> Result<i32> {
    run_gain(args)
}

fn run_gain(args: AnalyticsArgs) -> Result<i32> {
    let root = resolve_root(&args.root)?;
    if args.reset {
        if !args.yes {
            anyhow::bail!("gain --reset requires --yes in non-interactive Packet28 mode");
        }
        let cleared = reset_run_savings(&root)?;
        if args.json {
            crate::cmd_common::emit_json(
                &json!({
                    "reset": true,
                    "cleared_run_savings": cleared,
                }),
                args.pretty,
            )?;
        } else {
            println!("Token savings stats reset to zero.");
            println!("cleared_run_savings={cleared}");
        }
        return Ok(0);
    }
    let states = load_task_states(&root, args.task_id.as_deref(), args.limit)?;
    let mut summary = GainSummary {
        task_count: states.len(),
        ..GainSummary::default()
    };
    for (_, state) in states {
        for invocation in state.recent_tool_invocations {
            summary.invocation_count += 1;
            summary.raw_est_tokens += invocation.raw_est_tokens.unwrap_or(0);
            summary.reduced_est_tokens += invocation.reduced_est_tokens.unwrap_or(0);
            let route = invocation
                .compact_path
                .unwrap_or_else(|| "unknown".to_string());
            *summary.by_route.entry(route.clone()).or_insert(0) += 1;
            record_gain_route_roi(
                &mut summary.route_roi,
                &route,
                invocation.raw_est_tokens.unwrap_or(0),
                invocation.reduced_est_tokens.unwrap_or(0),
            );
        }
    }
    let run_savings = load_run_savings(&root, args.limit)?;
    if args.remember_advice {
        summary.remembered_failure_advice_count = remember_failure_advice(&run_savings)?.len();
    }
    for record in &run_savings {
        summary.invocation_count += 1;
        summary.raw_est_tokens += record.raw_est_tokens;
        summary.reduced_est_tokens += record.reduced_est_tokens;
        let route = gain_route_for_run_savings(record);
        *summary.by_route.entry(route.clone()).or_insert(0) += 1;
        record_gain_route_roi(
            &mut summary.route_roi,
            &route,
            record.raw_est_tokens,
            record.reduced_est_tokens,
        );
    }
    record_gain_route_followups(&mut summary.route_roi, &run_savings);
    summary.saved_est_tokens = summary
        .raw_est_tokens
        .saturating_sub(summary.reduced_est_tokens);
    summary.savings_pct = pct_saved(summary.raw_est_tokens, summary.reduced_est_tokens);
    let format = gain_output_format(&args);
    if args.json || format == "json" {
        crate::cmd_common::emit_json(&serde_json::to_value(summary)?, args.pretty)?;
    } else if format == "csv" {
        print_gain_csv(&summary);
    } else if format == "history" {
        print_gain_history(&run_savings);
    } else if format == "failures" {
        print_gain_failures(&run_savings);
        if args.remember_advice {
            println!(
                "remembered_failure_advice_count={}",
                summary.remembered_failure_advice_count
            );
        }
    } else if format == "daily" {
        print_gain_buckets(&run_savings, GainBucketKind::Daily);
    } else if format == "weekly" {
        print_gain_buckets(&run_savings, GainBucketKind::Weekly);
    } else if format == "monthly" {
        print_gain_buckets(&run_savings, GainBucketKind::Monthly);
    } else if format == "quota" {
        print_gain_quota(&summary, args.quota_tokens, &args.tier);
    } else if format == "graph" {
        print_gain_graph(&summary);
    } else if format == "all" {
        print_gain_all(&summary, &run_savings, args.quota_tokens, &args.tier);
    } else {
        if let Some(warning) = gain_disabled_bypass_warning(args.sessions_dir.as_deref()) {
            eprintln!("{warning}");
            eprintln!();
        }
        println!("tasks={}", summary.task_count);
        println!("invocations={}", summary.invocation_count);
        println!("raw_est_tokens={}", summary.raw_est_tokens);
        println!("reduced_est_tokens={}", summary.reduced_est_tokens);
        println!("saved_est_tokens={}", summary.saved_est_tokens);
        println!("savings_pct={:.1}", summary.savings_pct);
        for (route, count) in summary.by_route {
            println!("route.{route}={count}");
        }
    }
    Ok(0)
}

fn record_gain_route_roi(
    route_roi: &mut BTreeMap<String, GainRouteRoi>,
    route: &str,
    raw_est_tokens: u64,
    reduced_est_tokens: u64,
) {
    let entry = route_roi.entry(route.to_string()).or_default();
    entry.invocation_count += 1;
    entry.raw_est_tokens = entry.raw_est_tokens.saturating_add(raw_est_tokens);
    entry.reduced_est_tokens = entry.reduced_est_tokens.saturating_add(reduced_est_tokens);
    entry.saved_est_tokens = entry
        .raw_est_tokens
        .saturating_sub(entry.reduced_est_tokens);
    entry.savings_pct = pct_saved(entry.raw_est_tokens, entry.reduced_est_tokens);
    entry.avg_saved_tokens = if entry.invocation_count == 0 {
        0.0
    } else {
        entry.saved_est_tokens as f64 / entry.invocation_count as f64
    };
    entry.avg_followups_per_invocation = if entry.invocation_count == 0 {
        0.0
    } else {
        entry.downstream_followup_count as f64 / entry.invocation_count as f64
    };
}

fn record_gain_route_followups(
    route_roi: &mut BTreeMap<String, GainRouteRoi>,
    records: &[RunSavingsRecord],
) {
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.timestamp_unix_ms.cmp(&b.timestamp_unix_ms));
    for (index, record) in ordered.iter().enumerate() {
        let followups = count_downstream_followups(&ordered, index);
        if followups == 0 {
            continue;
        }
        let route = gain_route_for_run_savings(record);
        let entry = route_roi.entry(route).or_default();
        entry.downstream_followup_count = entry.downstream_followup_count.saturating_add(followups);
        entry.avg_followups_per_invocation = if entry.invocation_count == 0 {
            0.0
        } else {
            entry.downstream_followup_count as f64 / entry.invocation_count as f64
        };
    }
}

fn count_downstream_followups(records: &[&RunSavingsRecord], index: usize) -> usize {
    const FOLLOWUP_WINDOW_MS: u128 = 10 * 60 * 1000;
    let Some(record) = records.get(index) else {
        return 0;
    };
    records
        .iter()
        .skip(index + 1)
        .take_while(|candidate| {
            candidate
                .timestamp_unix_ms
                .saturating_sub(record.timestamp_unix_ms)
                <= FOLLOWUP_WINDOW_MS
        })
        .filter(|candidate| candidate.cwd == record.cwd)
        .count()
}

fn gain_route_for_run_savings(record: &RunSavingsRecord) -> String {
    if let Some(reason) = &record.fallback_reason {
        format!("run_fallback:{reason}")
    } else {
        format!("run_reducer:{}", record.family)
    }
}

fn gain_disabled_bypass_warning(sessions_dir: Option<&str>) -> Option<String> {
    let sessions_dir = sessions_dir.map(PathBuf::from).unwrap_or_else(|| {
        #[cfg(unix)]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(format!("{home}/.claude/projects"));
            }
        }
        PathBuf::from("/tmp/.claude/projects")
    });
    let files =
        crate::cmd_discover::collect_session_files_for_scan(&sessions_dir, 200, false, Some(7))
            .ok()?;
    if files.is_empty() || files.len() > 200 {
        return None;
    }

    let mut total = 0usize;
    let mut bypassed = 0usize;
    for path in files {
        let Ok(commands) = crate::cmd_discover::extract_bash_commands(&path) else {
            continue;
        };
        for (command, _) in commands {
            total += 1;
            if let Some(actual_command) =
                crate::cmd_discover::strip_active_disabled_prefix(&command)
            {
                let route = crate::route_registry::decide_command_route(&actual_command);
                if !matches!(route.kind, crate::route_registry::RouteKind::RawPassthrough) {
                    bypassed += 1;
                }
            }
        }
    }

    if total == 0 {
        return None;
    }
    let pct = (bypassed as f64 / total as f64) * 100.0;
    (pct > 10.0).then(|| {
        format!(
            "[warn] {bypassed} commands ({pct:.0}%) used PACKET28_DISABLED/RTK_DISABLED unnecessarily - run `Packet28 discover` for details"
        )
    })
}

fn gain_output_format(args: &AnalyticsArgs) -> String {
    let explicit = args.format.trim().to_ascii_lowercase();
    if args.json || explicit != "text" {
        return explicit;
    }
    if args.graph {
        "graph"
    } else if args.history {
        "history"
    } else if args.failures {
        "failures"
    } else if args.daily {
        "daily"
    } else if args.weekly {
        "weekly"
    } else if args.monthly {
        "monthly"
    } else if args.quota {
        "quota"
    } else if args.all {
        "all"
    } else {
        "text"
    }
    .to_string()
}

#[derive(Debug, Clone, Copy)]
enum GainBucketKind {
    Daily,
    Weekly,
    Monthly,
}

fn print_gain_buckets(records: &[RunSavingsRecord], kind: GainBucketKind) {
    let mut buckets = BTreeMap::<String, GainBucket>::new();
    for record in records {
        let label = gain_bucket_label(record.timestamp_unix_ms, kind);
        let bucket = buckets.entry(label.clone()).or_insert_with(|| GainBucket {
            period: label,
            ..GainBucket::default()
        });
        bucket.invocation_count += 1;
        bucket.raw_est_tokens += record.raw_est_tokens;
        bucket.reduced_est_tokens += record.reduced_est_tokens;
    }
    println!(
        "period,invocation_count,raw_est_tokens,reduced_est_tokens,saved_est_tokens,savings_pct"
    );
    for bucket in buckets.values_mut() {
        bucket.saved_est_tokens = bucket
            .raw_est_tokens
            .saturating_sub(bucket.reduced_est_tokens);
        bucket.savings_pct = pct_saved(bucket.raw_est_tokens, bucket.reduced_est_tokens);
        println!(
            "{},{},{},{},{},{:.1}",
            csv_cell(&bucket.period),
            bucket.invocation_count,
            bucket.raw_est_tokens,
            bucket.reduced_est_tokens,
            bucket.saved_est_tokens,
            bucket.savings_pct
        );
    }
}

fn print_gain_quota(summary: &GainSummary, quota_tokens: Option<u64>, tier: &str) {
    let (quota, tier_label) = quota_tokens
        .map(|tokens| (tokens, "custom"))
        .unwrap_or_else(|| tier_quota_tokens(tier));
    let used_pct = if quota == 0 {
        0.0
    } else {
        (summary.reduced_est_tokens as f64 / quota as f64) * 100.0
    };
    let avoided_pct = if quota == 0 {
        0.0
    } else {
        (summary.saved_est_tokens as f64 / quota as f64) * 100.0
    };
    println!("tier={tier_label}");
    println!("quota_tokens={quota}");
    println!("raw_est_tokens={}", summary.raw_est_tokens);
    println!("reduced_est_tokens={}", summary.reduced_est_tokens);
    println!("saved_est_tokens={}", summary.saved_est_tokens);
    println!("quota_used_pct={used_pct:.1}");
    println!("quota_avoided_pct={avoided_pct:.1}");
}

fn tier_quota_tokens(tier: &str) -> (u64, &'static str) {
    const PRO_MONTHLY_TOKENS: u64 = 6_000_000;
    match tier {
        "pro" => (PRO_MONTHLY_TOKENS, "pro"),
        "5x" => (PRO_MONTHLY_TOKENS * 5, "5x"),
        "20x" => (PRO_MONTHLY_TOKENS * 20, "20x"),
        _ => (PRO_MONTHLY_TOKENS, "pro"),
    }
}

fn print_gain_graph(summary: &GainSummary) {
    println!("Packet28 savings graph");
    println!("invocations={}", summary.invocation_count);
    println!("saved_est_tokens={}", summary.saved_est_tokens);
    println!("savings_pct={:.1}", summary.savings_pct);
    println!();
    println!("{:<36} {:>7} {:>7}  impact", "route", "count", "share");
    let total = summary.invocation_count.max(1);
    for (route, count) in &summary.by_route {
        let share = (*count as f64 / total as f64) * 100.0;
        let width = ((*count * 40) / total).max(1);
        println!(
            "{:<36} {:>7} {:>6.1}%  {}",
            graph_route_cell(route, 36),
            count,
            share,
            "#".repeat(width)
        );
    }
}

fn graph_route_cell(route: &str, max_width: usize) -> String {
    if route.len() <= max_width {
        return route.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    format!("{}...", &route[..max_width - 3])
}

fn print_gain_all(
    summary: &GainSummary,
    run_savings: &[RunSavingsRecord],
    quota_tokens: Option<u64>,
    tier: &str,
) {
    println!("[summary]");
    println!("tasks={}", summary.task_count);
    println!("invocations={}", summary.invocation_count);
    println!("raw_est_tokens={}", summary.raw_est_tokens);
    println!("reduced_est_tokens={}", summary.reduced_est_tokens);
    println!("saved_est_tokens={}", summary.saved_est_tokens);
    println!("savings_pct={:.1}", summary.savings_pct);
    println!();
    println!("[graph]");
    print_gain_graph(summary);
    println!();
    println!("[daily]");
    print_gain_buckets(run_savings, GainBucketKind::Daily);
    println!();
    println!("[weekly]");
    print_gain_buckets(run_savings, GainBucketKind::Weekly);
    println!();
    println!("[monthly]");
    print_gain_buckets(run_savings, GainBucketKind::Monthly);
    println!();
    println!("[quota]");
    print_gain_quota(summary, quota_tokens, tier);
    println!();
    println!("[failures]");
    print_gain_failures(run_savings);
}

fn print_gain_history(records: &[RunSavingsRecord]) {
    println!("timestamp_unix_ms,family,raw_est_tokens,reduced_est_tokens,savings_percent,exit_code,command");
    for record in records {
        println!(
            "{},{},{},{},{:.1},{},{}",
            record.timestamp_unix_ms,
            csv_cell(&record.family),
            record.raw_est_tokens,
            record.reduced_est_tokens,
            record.savings_percent,
            record.exit_code,
            csv_cell(&record.command)
        );
    }
}

fn print_gain_failures(records: &[RunSavingsRecord]) {
    let mut repeat_counts = BTreeMap::<String, usize>::new();
    for record in records {
        if let Some(fingerprint) = record.failure_fingerprint.as_deref() {
            *repeat_counts.entry(fingerprint.to_string()).or_insert(0) += 1;
        }
    }
    println!("timestamp_unix_ms,family,exit_code,fallback_reason,failure_fingerprint,repeat_count,next_success_command,next_success_changed_paths,fix_advice,command");
    for record in records
        .iter()
        .filter(|record| record.exit_code != 0 || record.fallback_reason.is_some())
    {
        let repeat_count = record
            .failure_fingerprint
            .as_ref()
            .and_then(|fingerprint| repeat_counts.get(fingerprint))
            .copied()
            .unwrap_or(0);
        let next_success = next_success_record(records, record);
        let next_success_command = next_success
            .map(|record| record.command.clone())
            .unwrap_or_default();
        let next_success_changed_paths = next_success
            .map(|record| record.changed_paths.join(";"))
            .unwrap_or_default();
        let fix_advice = failure_fix_advice(
            repeat_count,
            next_success_command.as_str(),
            next_success_changed_paths.as_str(),
        );
        println!(
            "{},{},{},{},{},{},{},{},{},{}",
            record.timestamp_unix_ms,
            csv_cell(&record.family),
            record.exit_code,
            csv_cell(record.fallback_reason.as_deref().unwrap_or("")),
            csv_cell(record.failure_fingerprint.as_deref().unwrap_or("")),
            repeat_count,
            csv_cell(&next_success_command),
            csv_cell(&next_success_changed_paths),
            csv_cell(&fix_advice),
            csv_cell(&record.command)
        );
    }
}

fn print_gain_csv(summary: &GainSummary) {
    println!("kind,name,value");
    println!("summary,task_count,{}", summary.task_count);
    println!("summary,invocation_count,{}", summary.invocation_count);
    println!("summary,raw_est_tokens,{}", summary.raw_est_tokens);
    println!("summary,reduced_est_tokens,{}", summary.reduced_est_tokens);
    println!("summary,saved_est_tokens,{}", summary.saved_est_tokens);
    println!("summary,savings_pct,{:.1}", summary.savings_pct);
    for (route, count) in &summary.by_route {
        println!("route,{},{}", csv_cell(route), count);
    }
}

fn remember_failure_advice(records: &[RunSavingsRecord]) -> Result<Vec<FeedbackRecord>> {
    let repeat_counts = failure_repeat_counts(records);
    let mut remembered = Vec::new();
    let mut seen_fingerprints = BTreeMap::<String, ()>::new();
    for record in records
        .iter()
        .filter(|record| record.exit_code != 0 || record.fallback_reason.is_some())
    {
        let Some(fingerprint) = record.failure_fingerprint.as_deref() else {
            continue;
        };
        let repeat_count = repeat_counts.get(fingerprint).copied().unwrap_or(0);
        if repeat_count <= 1 || seen_fingerprints.contains_key(fingerprint) {
            continue;
        }
        let Some(next_success) = next_success_record(records, record) else {
            continue;
        };
        let changed_paths = next_success.changed_paths.join(";");
        let correction = failure_fix_advice(
            repeat_count,
            next_success.command.as_str(),
            changed_paths.as_str(),
        );
        let context = format!(
            "failure_fingerprint={fingerprint}\nrepeat_count={repeat_count}\nfailed_command={}\nnext_success_command={}\nnext_success_changed_paths={changed_paths}",
            record.command, next_success.command
        );
        let feedback = record_feedback_with_metadata(FeedbackInput {
            subject: &format!("failure_fingerprint:{fingerprint}"),
            correction: &correction,
            topic: Some("failure-advice"),
            context: Some(&context),
            predicted: Some(&record.command),
            reason: Some("confirmed by later successful same-directory run"),
            source: Some("packet28.gain.failures"),
            project: Some(&record.cwd),
        })?;
        seen_fingerprints.insert(fingerprint.to_string(), ());
        remembered.push(feedback);
    }
    Ok(remembered)
}

fn failure_repeat_counts(records: &[RunSavingsRecord]) -> BTreeMap<String, usize> {
    let mut repeat_counts = BTreeMap::<String, usize>::new();
    for record in records {
        if let Some(fingerprint) = record.failure_fingerprint.as_deref() {
            *repeat_counts.entry(fingerprint.to_string()).or_insert(0) += 1;
        }
    }
    repeat_counts
}

fn failure_fix_advice(
    repeat_count: usize,
    next_success_command: &str,
    next_success_changed_paths: &str,
) -> String {
    if next_success_command.is_empty() {
        return "no later successful same-directory run observed".to_string();
    }
    let prefix = if repeat_count > 1 {
        "repeated failure"
    } else {
        "failure"
    };
    if next_success_changed_paths.is_empty() {
        format!("{prefix}: retry with `{next_success_command}`")
    } else {
        format!("{prefix}: retry with `{next_success_command}` after inspecting {next_success_changed_paths}")
    }
}

fn next_success_record<'a>(
    records: &'a [RunSavingsRecord],
    failed: &RunSavingsRecord,
) -> Option<&'a RunSavingsRecord> {
    records
        .iter()
        .filter(|candidate| {
            candidate.timestamp_unix_ms > failed.timestamp_unix_ms
                && candidate.cwd == failed.cwd
                && candidate.exit_code == 0
        })
        .min_by_key(|candidate| candidate.timestamp_unix_ms)
}

fn gain_bucket_label(timestamp_unix_ms: u128, kind: GainBucketKind) -> String {
    let day = (timestamp_unix_ms / 86_400_000) as i64;
    match kind {
        GainBucketKind::Daily => format_ymd(day),
        GainBucketKind::Weekly => {
            let weekday_monday_zero = (day + 3).rem_euclid(7);
            let week_start = day - weekday_monday_zero;
            format!("week_of_{}", format_ymd(week_start))
        }
        GainBucketKind::Monthly => {
            let (year, month, _) = civil_from_unix_day(day);
            format!("{year:04}-{month:02}")
        }
    }
}
