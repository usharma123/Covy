//! Query planning, candidate selection, and source verification.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use packet28_reducer_core::{
    infer_symbols_from_pattern, SearchEngineStats, SearchGroup, SearchMatch, SearchRequest,
    SearchResult,
};
use regex::RegexBuilder;
use regex_syntax::hir::literal::{ExtractKind, Extractor, Seq};
use regex_syntax::hir::{Hir, HirKind};

use crate::error::{Result, SearchError};
use crate::model::{
    CompiledSearch, LayerKind, LiteralWindow, LoadedIndex, LoadedLayer, PositionSummary,
    PostingEntry, QueryCache, RegexIndexRuntime, SearchPlan, SparseCandidate, Verifier,
    MAX_GRAM_BYTES, MAX_INDEX_VERIFY_CANDIDATES, MAX_INDEX_VERIFY_DENOMINATOR,
    MAX_INDEX_VERIFY_NUMERATOR, MAX_LITERAL_COVER, MIN_GRAM_BYTES, POSITION_BUCKET_COUNT,
    SHORT_GRAM_BYTES,
};
use crate::paths::resolve_requested_paths;
use crate::postings::{
    build_covering_candidates, build_covering_hashes, checked_posting_bounds, decode_postings,
    hash_bytes, lookup_posting_range, normalize_for_index,
};
use crate::support::ResultContext;

/// Returns why a request should use the legacy search engine, if applicable.
///
/// # Errors
///
/// Returns [`SearchError::InvalidRegexSyntax`] or [`SearchError::InvalidRegex`]
/// for invalid expressions, and a typed corruption or conversion failure if a
/// previously validated posting cannot be planned safely.
pub fn guarded_fallback_reason(
    root: &Path,
    runtime: &RegexIndexRuntime,
    request: &SearchRequest,
) -> Result<Option<String>> {
    if !runtime.is_loaded() || runtime.manifest.status != "ready" {
        let reason = runtime
            .manifest
            .stale_reason
            .clone()
            .or_else(|| runtime.manifest.last_error.clone())
            .unwrap_or_else(|| "regex search index is not ready".to_string());
        return Ok(Some(reason));
    }
    let loaded = runtime.loaded.as_ref().ok_or(SearchError::IndexNotLoaded)?;
    let compiled = compile_request(request, loaded.as_ref())?;
    if let Some(reason) = compiled.must_fallback_reason.clone() {
        return Ok(Some(reason));
    }
    if matches!(compiled.plan, SearchPlan::All) {
        return Ok(Some(compiled.planner_fallback.unwrap_or_else(|| {
            "planner could not derive a selective index plan".to_string()
        })));
    }
    let (resolved_paths, _) = resolve_requested_paths(root, &request.requested_paths);
    let requested_filter = requested_filter_set(&resolved_paths);
    let all_paths = all_indexed_paths(loaded.as_ref(), requested_filter.as_ref());
    let mut engine = SearchEngineStats {
        engine: "indexed_regex".to_string(),
        index_generation: Some(runtime.manifest.generation),
        base_commit: runtime.manifest.base_commit.clone(),
        plan_kind: Some(compiled.plan_kind.clone()),
        planner_fallback: compiled
            .must_fallback_reason
            .clone()
            .or(compiled.planner_fallback.clone()),
        stale_reason: runtime.manifest.stale_reason.clone(),
        candidates_examined: 0,
        candidate_files: 0,
        verified_files: 0,
        index_lookups: 0,
        postings_bytes_read: 0,
        fallback_reason: None,
    };
    let mut cache = QueryCache::default();
    let candidates = candidate_paths_for_plan(
        loaded.as_ref(),
        &compiled.plan,
        requested_filter.as_ref(),
        &all_paths,
        &mut cache,
        &mut engine,
    )?;
    let pruned_candidates =
        prune_candidates_with_positions(loaded.as_ref(), &compiled.plan, &candidates, &mut cache);
    if should_fallback_to_rg(pruned_candidates.len(), all_paths.len()) {
        return Ok(Some(format!(
            "candidate set remained too broad for indexed verification ({}/{} files)",
            pruned_candidates.len(),
            all_paths.len()
        )));
    }
    Ok(None)
}

/// Executes an indexed search and verifies candidate files against the request.
///
/// # Errors
///
/// Returns [`SearchError::IndexNotLoaded`] when `runtime` has no validated
/// generation, [`SearchError::EmptyQuery`] for a blank query, typed regex errors
/// for invalid expressions, [`SearchError::Io`] when a candidate cannot be read,
/// or a typed corruption/conversion error for an invalid posting.
pub fn indexed_search(
    root: &Path,
    runtime: &RegexIndexRuntime,
    request: &SearchRequest,
) -> Result<SearchResult> {
    let loaded = runtime.loaded.as_ref().ok_or(SearchError::IndexNotLoaded)?;
    let query = request.query.trim();
    if query.is_empty() {
        return Err(SearchError::EmptyQuery);
    }

    let (resolved_paths, mut diagnostics) = resolve_requested_paths(root, &request.requested_paths);
    let requested_filter = requested_filter_set(&resolved_paths);
    let compiled = compile_request(request, loaded.as_ref())?;
    let mut engine = SearchEngineStats {
        engine: "indexed_regex".to_string(),
        index_generation: Some(runtime.manifest.generation),
        base_commit: runtime.manifest.base_commit.clone(),
        plan_kind: Some(compiled.plan_kind.clone()),
        planner_fallback: compiled
            .must_fallback_reason
            .clone()
            .or(compiled.planner_fallback.clone()),
        stale_reason: runtime.manifest.stale_reason.clone(),
        candidates_examined: 0,
        candidate_files: 0,
        verified_files: 0,
        index_lookups: 0,
        postings_bytes_read: 0,
        fallback_reason: None,
    };
    let mut cache = QueryCache::default();

    let all_paths = all_indexed_paths(loaded.as_ref(), requested_filter.as_ref());
    if let Some(reason) = compiled
        .must_fallback_reason
        .clone()
        .or(compiled.planner_fallback.clone())
    {
        diagnostics.push(reason);
    }
    let candidate_paths = candidate_paths_for_plan(
        loaded.as_ref(),
        &compiled.plan,
        requested_filter.as_ref(),
        &all_paths,
        &mut cache,
        &mut engine,
    )?;
    let pruned_candidate_paths = prune_candidates_with_positions(
        loaded.as_ref(),
        &compiled.plan,
        &candidate_paths,
        &mut cache,
    );

    engine.candidates_examined = candidate_paths.len();
    engine.candidate_files = candidate_paths.len();
    engine.verified_files = pruned_candidate_paths.len();

    let mut groups = Vec::new();
    let mut total_match_count = 0usize;
    for path in &pruned_candidate_paths {
        let file_groups =
            verify_path(root, path, &compiled.verifier, request.max_matches_per_file)?;
        if file_groups.is_empty() {
            continue;
        }
        total_match_count = total_match_count.saturating_add(file_groups.len());
        let displayed = file_groups.iter().take(12).cloned().collect::<Vec<_>>();
        groups.push(SearchGroup {
            path: path.clone(),
            match_count: file_groups.len(),
            displayed_match_count: displayed.len(),
            truncated: file_groups.len() > displayed.len(),
            matches: displayed,
        });
    }

    let paths = groups
        .iter()
        .map(|group| group.path.clone())
        .collect::<Vec<_>>();
    let regions = groups
        .iter()
        .flat_map(|group| {
            group
                .matches
                .iter()
                .map(|item| format!("{}:{}-{}", item.path, item.line, item.line))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let max_total_matches = request.max_total_matches.unwrap_or(50).clamp(1, 200);
    let mut returned_matches = Vec::new();
    for group in &groups {
        for item in &group.matches {
            if returned_matches.len() >= max_total_matches {
                break;
            }
            returned_matches.push(item.clone());
        }
        if returned_matches.len() >= max_total_matches {
            break;
        }
    }
    let returned_match_count = returned_matches.len();
    let compact_preview = render_compact_preview(total_match_count, &groups);

    Ok(SearchResult {
        query: query.to_string(),
        requested_paths: request.requested_paths.clone(),
        resolved_paths,
        match_count: total_match_count,
        returned_match_count,
        truncated: total_match_count > returned_match_count,
        paths,
        regions,
        symbols: infer_symbols_from_pattern(query),
        groups,
        compact_preview,
        diagnostics,
        engine: Some(engine),
    })
}

pub(crate) fn build_verifier(request: &SearchRequest, query: &str) -> Result<Verifier> {
    if request.fixed_string && !request.whole_word && !matches!(request.case_sensitive, Some(false))
    {
        return Ok(Verifier::FixedBytes {
            needle: query.as_bytes().to_vec(),
            case_insensitive: matches!(request.case_sensitive, Some(false)),
        });
    }
    let pattern = if request.fixed_string {
        regex::escape(query)
    } else {
        query.to_string()
    };
    let pattern = if request.whole_word {
        format!(r"\b(?:{})\b", pattern)
    } else {
        pattern
    };
    let whole_file_prefilter = if request.fixed_string {
        true
    } else {
        let hir = regex_syntax::parse(query).map_err(|source| SearchError::InvalidRegexSyntax {
            query: query.to_string(),
            source: Box::new(source),
        })?;
        !hir_has_line_anchors(&hir)
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(matches!(request.case_sensitive, Some(false)))
        .build()
        .map_err(|source| SearchError::InvalidRegex {
            query: query.to_string(),
            source: Box::new(source),
        })?;
    Ok(Verifier::Regex {
        regex,
        whole_file_prefilter,
    })
}

pub(crate) fn compile_request(
    request: &SearchRequest,
    loaded: &LoadedIndex,
) -> Result<CompiledSearch> {
    let query = request.query.trim();
    let verifier = build_verifier(request, query)?;
    let (plan, planner_fallback) = build_search_plan(request, query)?;
    let must_fallback_reason = classify_plan_fallback_reason(loaded, &plan);
    Ok(CompiledSearch {
        verifier,
        plan_kind: plan.kind_str().to_string(),
        plan,
        planner_fallback,
        must_fallback_reason,
    })
}

pub(crate) fn build_search_plan(
    request: &SearchRequest,
    query: &str,
) -> Result<(SearchPlan, Option<String>)> {
    if request.fixed_string {
        if matches!(request.case_sensitive, Some(false)) && !query.is_ascii() {
            return Ok((
                SearchPlan::All,
                Some(
                    "unicode ignore-case fixed-string queries use regex fallback instead of ASCII-only index normalization"
                        .to_string(),
                ),
            ));
        }
        let literal = normalize_for_index(query.as_bytes());
        if build_covering_hashes(&literal).is_empty() {
            return Ok((
                SearchPlan::All,
                Some(
                    "fixed string query is too short to derive a selective index plan".to_string(),
                ),
            ));
        }
        return Ok((SearchPlan::Literal(literal), None));
    }

    let hir = regex_syntax::parse(query).map_err(|source| SearchError::InvalidRegexSyntax {
        query: query.to_string(),
        source: Box::new(source),
    })?;
    let plan = normalize_plan(plan_from_hir(&hir));
    let planner_fallback = matches!(plan, SearchPlan::All).then(|| {
        "planner could not derive required literals; verifying all indexed candidates".to_string()
    });
    Ok((plan, planner_fallback))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanStrength {
    Strong,
    Weak,
}

pub(crate) fn classify_plan_fallback_reason(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
) -> Option<String> {
    match assess_plan_strength(loaded, plan) {
        Some(PlanStrength::Strong) => None,
        Some(PlanStrength::Weak) => Some(
            "planner derived only weak/common literals; routing broad regex to legacy_rg"
                .to_string(),
        ),
        None => Some("planner could not derive an index-safe branch set".to_string()),
    }
}

pub(crate) fn assess_plan_strength(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
) -> Option<PlanStrength> {
    match plan {
        SearchPlan::All => None,
        SearchPlan::Literal(literal) => Some(literal_strength(loaded, literal)),
        SearchPlan::And(children) => {
            let mut saw_strong = false;
            let sibling_literal_count = children
                .iter()
                .filter(|child| matches!(child, SearchPlan::Literal(_)))
                .count();
            for child in children {
                match assess_plan_strength_with_siblings(loaded, child, sibling_literal_count) {
                    Some(PlanStrength::Strong) => saw_strong = true,
                    Some(PlanStrength::Weak) => {}
                    None => return None,
                }
            }
            Some(if saw_strong {
                PlanStrength::Strong
            } else {
                PlanStrength::Weak
            })
        }
        SearchPlan::Or(children) => {
            let mut saw_strong = false;
            for child in children {
                match assess_plan_strength(loaded, child) {
                    Some(PlanStrength::Strong) => saw_strong = true,
                    Some(PlanStrength::Weak) => {}
                    None => return None,
                }
            }
            let total_paths = all_indexed_paths(loaded, None).len().max(1);
            let estimated_candidates = estimate_plan_cardinality(loaded, plan, None, total_paths);
            if saw_strong || estimated_candidates.saturating_mul(2) <= total_paths {
                return Some(PlanStrength::Strong);
            }
            Some(PlanStrength::Weak)
        }
    }
}

pub(crate) fn assess_plan_strength_with_siblings(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    sibling_literal_count: usize,
) -> Option<PlanStrength> {
    match plan {
        SearchPlan::Literal(literal) => Some(literal_strength_with_siblings(
            loaded,
            literal,
            sibling_literal_count,
        )),
        _ => assess_plan_strength(loaded, plan),
    }
}

pub(crate) fn literal_strength(loaded: &LoadedIndex, literal: &[u8]) -> PlanStrength {
    literal_strength_with_siblings(loaded, literal, 0)
}

pub(crate) fn literal_strength_with_siblings(
    loaded: &LoadedIndex,
    literal: &[u8],
    sibling_literal_count: usize,
) -> PlanStrength {
    let total_paths = all_indexed_paths(loaded, None).len().max(1);
    let min_docs = select_covering_candidates(loaded, literal)
        .into_iter()
        .map(|candidate| {
            lookup_posting_count(&loaded.base, candidate.hash)
                .unwrap_or(0)
                .saturating_add(overlay_posting_count(loaded, candidate.hash)) as usize
        })
        .min()
        .unwrap_or(total_paths);
    let literal_len = literal.len();
    if literal_len <= 3 && min_docs.saturating_mul(4) > total_paths {
        return PlanStrength::Weak;
    }
    if literal_len <= 4 && sibling_literal_count == 0 && min_docs.saturating_mul(3) > total_paths {
        return PlanStrength::Weak;
    }
    if literal_len >= 6 || min_docs <= 8 {
        return PlanStrength::Strong;
    }
    if sibling_literal_count > 0 && min_docs.saturating_mul(2) <= total_paths {
        return PlanStrength::Strong;
    }
    if min_docs.saturating_mul(8) <= total_paths {
        return PlanStrength::Strong;
    }
    PlanStrength::Weak
}

pub(crate) fn plan_from_hir(hir: &Hir) -> SearchPlan {
    match hir.kind() {
        HirKind::Empty | HirKind::Class(_) | HirKind::Look(_) => SearchPlan::All,
        HirKind::Literal(literal) => literal_plan_from_bytes(&literal.0),
        HirKind::Capture(capture) => combine_required_plan(plan_from_hir(&capture.sub), hir),
        HirKind::Concat(subs) => {
            combine_required_plan(normalize_and(subs.iter().map(plan_from_hir).collect()), hir)
        }
        HirKind::Alternation(subs) => {
            combine_required_plan(normalize_or(subs.iter().map(plan_from_hir).collect()), hir)
        }
        HirKind::Repetition(repetition) => plan_from_repetition(repetition, hir),
    }
}

pub(crate) fn plan_from_repetition(
    repetition: &regex_syntax::hir::Repetition,
    hir: &Hir,
) -> SearchPlan {
    if repetition.min == 0 {
        return SearchPlan::All;
    }
    let child_plan = plan_from_hir(&repetition.sub);
    let repeated_plan = if repetition.min > 1 {
        match &child_plan {
            SearchPlan::Literal(literal) if !literal.is_empty() => {
                let repeats_to_materialize = (repetition.min as usize)
                    .min((MAX_GRAM_BYTES / literal.len()).saturating_add(1));
                literal_plan_from_bytes(&literal.repeat(repeats_to_materialize))
            }
            _ => child_plan.clone(),
        }
    } else {
        child_plan
    };
    combine_required_plan(repeated_plan, hir)
}

pub(crate) fn combine_required_plan(structural: SearchPlan, hir: &Hir) -> SearchPlan {
    let extracted = plan_from_extractors(hir);
    match (normalize_plan(structural), normalize_plan(extracted)) {
        (SearchPlan::All, SearchPlan::All) => SearchPlan::All,
        (SearchPlan::All, other) | (other, SearchPlan::All) => other,
        (left, right) if left == right => left,
        (left, right) => normalize_and(vec![left, right]),
    }
}

pub(crate) fn plan_from_extractors(hir: &Hir) -> SearchPlan {
    let prefix = plan_from_extractor(hir, ExtractKind::Prefix);
    let suffix = plan_from_extractor(hir, ExtractKind::Suffix);
    combine_extractor_plans(prefix, suffix)
}

pub(crate) fn combine_extractor_plans(prefix: SearchPlan, suffix: SearchPlan) -> SearchPlan {
    match (prefix, suffix) {
        (SearchPlan::All, SearchPlan::All) => SearchPlan::All,
        (SearchPlan::All, other) | (other, SearchPlan::All) => other,
        (left, right) if left == right => left,
        (left, right) => normalize_and(vec![left, right]),
    }
}

pub(crate) fn plan_from_extractor(hir: &Hir, kind: ExtractKind) -> SearchPlan {
    let mut extractor = Extractor::new();
    extractor.limit_class(6).limit_repeat(8).limit_total(64);
    extractor.kind(kind.clone());
    let mut seq = extractor.extract(hir);
    if !seq.is_finite() || seq.is_empty() {
        return SearchPlan::All;
    }
    seq.minimize_by_preference();
    match kind {
        ExtractKind::Prefix => seq.keep_first_bytes(MAX_GRAM_BYTES),
        ExtractKind::Suffix => seq.keep_last_bytes(MAX_GRAM_BYTES),
        _ => return SearchPlan::All,
    }
    plan_from_literal_seq(&seq, kind)
}

pub(crate) fn plan_from_literal_seq(seq: &Seq, kind: ExtractKind) -> SearchPlan {
    let Some(literals) = seq.literals() else {
        return SearchPlan::All;
    };
    if let Some(common) = common_literal_from_seq(seq, kind) {
        return SearchPlan::Literal(common);
    }
    let mut normalized = Vec::<Vec<u8>>::new();
    for literal in literals {
        let bytes = normalize_for_index(literal.as_bytes());
        if is_poisonous_literal(&bytes) || build_covering_hashes(&bytes).is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == &bytes) {
            normalized.push(bytes);
        }
    }
    match normalized.len() {
        0 => SearchPlan::All,
        1 => SearchPlan::Literal(normalized.into_iter().next().unwrap_or_default()),
        _ => SearchPlan::Or(normalized.into_iter().map(SearchPlan::Literal).collect()),
    }
}

pub(crate) fn common_literal_from_seq(seq: &Seq, kind: ExtractKind) -> Option<Vec<u8>> {
    let common = match kind {
        ExtractKind::Prefix => seq.longest_common_prefix(),
        ExtractKind::Suffix => seq.longest_common_suffix(),
        _ => None,
    }?;
    let bytes = normalize_for_index(common);
    if is_poisonous_literal(&bytes) || build_covering_hashes(&bytes).is_empty() {
        return None;
    }
    Some(bytes)
}

pub(crate) fn literal_plan_from_bytes(bytes: &[u8]) -> SearchPlan {
    let normalized = normalize_for_index(bytes);
    if is_poisonous_literal(&normalized) || build_covering_hashes(&normalized).is_empty() {
        SearchPlan::All
    } else {
        SearchPlan::Literal(normalized)
    }
}

pub(crate) fn is_poisonous_literal(bytes: &[u8]) -> bool {
    bytes.len() < SHORT_GRAM_BYTES
}

pub(crate) fn normalize_plan(plan: SearchPlan) -> SearchPlan {
    match plan {
        SearchPlan::And(children) => normalize_and(children),
        SearchPlan::Or(children) => normalize_or(children),
        other => other,
    }
}

pub(crate) fn normalize_and(children: Vec<SearchPlan>) -> SearchPlan {
    let mut normalized = Vec::new();
    for child in children {
        match normalize_plan(child) {
            SearchPlan::All => {}
            SearchPlan::And(nested) => normalized.extend(nested),
            other if !normalized.iter().any(|existing| existing == &other) => {
                normalized.push(other)
            }
            _ => {}
        }
    }
    if normalized.is_empty() {
        SearchPlan::All
    } else if normalized.len() == 1 {
        normalized.into_iter().next().unwrap_or(SearchPlan::All)
    } else {
        SearchPlan::And(normalized)
    }
}

pub(crate) fn normalize_or(children: Vec<SearchPlan>) -> SearchPlan {
    let mut normalized = Vec::new();
    for child in children {
        match normalize_plan(child) {
            SearchPlan::All => return SearchPlan::All,
            SearchPlan::Or(nested) => normalized.extend(nested),
            other if !normalized.iter().any(|existing| existing == &other) => {
                normalized.push(other)
            }
            _ => {}
        }
    }
    if normalized.is_empty() {
        SearchPlan::All
    } else if normalized.len() == 1 {
        normalized.into_iter().next().unwrap_or(SearchPlan::All)
    } else {
        SearchPlan::Or(normalized)
    }
}

pub(crate) fn candidate_paths_for_plan(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    requested_filter: Option<&BTreeSet<String>>,
    all_paths: &BTreeSet<String>,
    cache: &mut QueryCache,
    engine: &mut SearchEngineStats,
) -> Result<BTreeSet<String>> {
    match plan {
        SearchPlan::All => Ok(all_paths.clone()),
        SearchPlan::Literal(literal) => {
            if let Some(cached) = cache.literal_candidates.get(literal) {
                return Ok(cached.clone());
            }
            let candidates = select_covering_candidates(loaded, literal);
            if candidates.is_empty() {
                return Ok(all_paths.clone());
            }
            let mut literal_paths: Option<BTreeSet<String>> = None;
            let mut selected_hashes = Vec::new();
            let mut covered = vec![false; literal.len()];
            let mut previous_size = all_paths.len();
            for candidate in candidates {
                let current =
                    paths_for_hash(loaded, candidate.hash, requested_filter, cache, engine)?;
                literal_paths = Some(match literal_paths {
                    Some(existing) => existing.intersection(&current).cloned().collect(),
                    None => current,
                });
                selected_hashes.push(candidate.hash);
                let coverage_end = candidate.end.min(covered.len());
                for slot in covered.iter_mut().take(coverage_end).skip(candidate.start) {
                    *slot = true;
                }
                let covered_all = covered.iter().all(|covered_byte| *covered_byte);
                let current_size = literal_paths.as_ref().map_or(0, BTreeSet::len);
                let materially_reduced =
                    current_size.saturating_mul(10) < previous_size.saturating_mul(9);
                if should_stop_literal_refinement(
                    current_size,
                    all_paths.len(),
                    covered_all,
                    selected_hashes.len(),
                    materially_reduced,
                ) {
                    break;
                }
                previous_size = current_size.max(1);
            }
            let resolved = literal_paths.unwrap_or_else(|| all_paths.clone());
            cache
                .literal_candidates
                .insert(literal.clone(), resolved.clone());
            cache
                .literal_hashes
                .insert(literal.clone(), selected_hashes);
            cache
                .literal_repeat_requirements
                .insert(literal.clone(), repeat_requirements_for_literal(literal));
            Ok(resolved)
        }
        SearchPlan::And(children) => {
            let mut current: Option<BTreeSet<String>> = None;
            let mut ordered = children.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|child| {
                estimate_plan_cardinality(loaded, child, requested_filter, all_paths.len())
            });
            for child in ordered {
                let child_paths = candidate_paths_for_plan(
                    loaded,
                    child,
                    requested_filter,
                    all_paths,
                    cache,
                    engine,
                )?;
                current = Some(match current {
                    Some(existing) => existing.intersection(&child_paths).cloned().collect(),
                    None => child_paths,
                });
            }
            Ok(current.unwrap_or_else(|| all_paths.clone()))
        }
        SearchPlan::Or(children) => {
            let mut union = BTreeSet::new();
            for child in children {
                union.extend(candidate_paths_for_plan(
                    loaded,
                    child,
                    requested_filter,
                    all_paths,
                    cache,
                    engine,
                )?);
            }
            Ok(union)
        }
    }
}

pub(crate) fn estimate_plan_cardinality(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    requested_filter: Option<&BTreeSet<String>>,
    all_path_count: usize,
) -> usize {
    match plan {
        SearchPlan::All => all_path_count,
        SearchPlan::Literal(literal) => {
            let hashes = select_covering_candidates(loaded, literal)
                .into_iter()
                .map(|candidate| candidate.hash)
                .collect::<Vec<_>>();
            if hashes.is_empty() {
                return all_path_count;
            }
            hashes
                .into_iter()
                .map(|hash| {
                    estimate_hash_cardinality(loaded, hash, requested_filter, all_path_count)
                })
                .min()
                .unwrap_or(all_path_count)
        }
        SearchPlan::And(children) => children
            .iter()
            .map(|child| estimate_plan_cardinality(loaded, child, requested_filter, all_path_count))
            .min()
            .unwrap_or(all_path_count),
        SearchPlan::Or(children) => children
            .iter()
            .map(|child| estimate_plan_cardinality(loaded, child, requested_filter, all_path_count))
            .sum::<usize>()
            .min(all_path_count),
    }
}

pub(crate) fn estimate_hash_cardinality(
    loaded: &LoadedIndex,
    hash: u64,
    requested_filter: Option<&BTreeSet<String>>,
    all_path_count: usize,
) -> usize {
    if let Some(filter) = requested_filter {
        let mut estimate = 0usize;
        if let Some(entries) = lookup_doc_ids_quiet(&loaded.base, hash) {
            for entry in entries {
                if let Some(doc) = loaded.base.docs.get(entry.doc_id as usize) {
                    if loaded.overlay_state.shadowed_paths.contains(&doc.path) {
                        continue;
                    }
                    if filter.contains(&doc.path) {
                        estimate = estimate.saturating_add(1);
                    }
                }
            }
        }
        for segment in &loaded.overlays {
            if let Some(entries) = lookup_doc_ids_quiet(&segment.layer, hash) {
                for entry in entries {
                    if let Some(doc) = segment.layer.docs.get(entry.doc_id as usize) {
                        if !overlay_doc_is_active(loaded, segment.generation, &doc.path) {
                            continue;
                        }
                        if filter.contains(&doc.path) {
                            estimate = estimate.saturating_add(1);
                        }
                    }
                }
            }
        }
        return estimate.min(all_path_count);
    }
    lookup_posting_count(&loaded.base, hash)
        .unwrap_or(0)
        .saturating_add(overlay_posting_count(loaded, hash)) as usize
}

pub(crate) fn select_covering_candidates(
    loaded: &LoadedIndex,
    literal: &[u8],
) -> Vec<SparseCandidate> {
    let mut candidates = build_covering_candidates(literal);
    candidates.sort_by(|left, right| {
        let left_docs = lookup_posting_count(&loaded.base, left.hash)
            .unwrap_or(0)
            .saturating_add(overlay_posting_count(loaded, left.hash));
        let right_docs = lookup_posting_count(&loaded.base, right.hash)
            .unwrap_or(0)
            .saturating_add(overlay_posting_count(loaded, right.hash));
        left_docs
            .cmp(&right_docs)
            .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
            .then_with(|| left.score.cmp(&right.score).reverse())
            .then_with(|| left.hash.cmp(&right.hash))
    });
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut covered = vec![false; literal.len()];
    for candidate in candidates {
        if !seen.insert(candidate.hash) {
            continue;
        }
        let adds_new_coverage = covered
            .iter()
            .enumerate()
            .skip(candidate.start)
            .take(candidate.end.saturating_sub(candidate.start))
            .any(|(_idx, slot)| !*slot);
        if adds_new_coverage || selected.len() < SHORT_GRAM_BYTES {
            let coverage_end = candidate.end.min(covered.len());
            for slot in covered.iter_mut().take(coverage_end).skip(candidate.start) {
                *slot = true;
            }
            selected.push(candidate);
        }
        if selected.len() >= MAX_LITERAL_COVER && covered.iter().all(|covered_byte| *covered_byte) {
            break;
        }
    }
    selected
}

pub(crate) fn paths_for_hash(
    loaded: &LoadedIndex,
    hash: u64,
    requested_filter: Option<&BTreeSet<String>>,
    cache: &mut QueryCache,
    engine: &mut SearchEngineStats,
) -> Result<BTreeSet<String>> {
    engine.index_lookups = engine.index_lookups.saturating_add(1);
    let mut paths = BTreeSet::new();

    if let Some(entries) =
        lookup_doc_ids_cached(&loaded.base, LayerKind::Base, hash, cache, engine)?
    {
        for entry in entries {
            if let Some(doc) = loaded.base.docs.get(entry.doc_id as usize) {
                if loaded.overlay_state.shadowed_paths.contains(&doc.path) {
                    continue;
                }
                if path_allowed(&doc.path, requested_filter) {
                    paths.insert(doc.path.clone());
                }
            }
        }
    }
    for segment in &loaded.overlays {
        if let Some(entries) = lookup_doc_ids_cached(
            &segment.layer,
            LayerKind::Overlay(segment.generation),
            hash,
            cache,
            engine,
        )? {
            for entry in entries {
                if let Some(doc) = segment.layer.docs.get(entry.doc_id as usize) {
                    if !overlay_doc_is_active(loaded, segment.generation, &doc.path) {
                        continue;
                    }
                    if path_allowed(&doc.path, requested_filter) {
                        paths.insert(doc.path.clone());
                    }
                }
            }
        }
    }
    Ok(paths)
}

pub(crate) fn lookup_doc_ids_quiet(layer: &LoadedLayer, hash: u64) -> Option<Vec<PostingEntry>> {
    // INVARIANT: `load_layer` validates every lookup range and posting block before
    // constructing an immutable `LoadedLayer`, so decoding cannot fail here.
    let lookup = layer.lookup.as_ref()?;
    let postings = layer.postings.as_ref()?;
    let meta = lookup_posting_range(lookup, hash)?;
    let (start, end) = checked_posting_bounds(meta.offset, meta.len, postings.len()).ok()?;
    decode_postings(&postings[start..end]).ok()
}

pub(crate) fn lookup_doc_ids_cached(
    layer: &LoadedLayer,
    layer_kind: LayerKind,
    hash: u64,
    cache: &mut QueryCache,
    engine: &mut SearchEngineStats,
) -> Result<Option<Vec<PostingEntry>>> {
    if let Some(cached) = cache.postings.get(&(layer_kind, hash)) {
        return Ok(cached.clone());
    }
    let value = lookup_doc_ids(layer, hash, engine)?;
    cache.postings.insert((layer_kind, hash), value.clone());
    Ok(value)
}

pub(crate) fn lookup_posting_count(layer: &LoadedLayer, hash: u64) -> Option<u32> {
    let lookup = layer.lookup.as_ref()?;
    Some(lookup_posting_range(lookup, hash)?.doc_count)
}

pub(crate) fn lookup_doc_ids(
    layer: &LoadedLayer,
    hash: u64,
    engine: &mut SearchEngineStats,
) -> Result<Option<Vec<PostingEntry>>> {
    let Some(lookup) = layer.lookup.as_ref() else {
        return Ok(None);
    };
    let Some(postings) = layer.postings.as_ref() else {
        return Ok(None);
    };
    let Some(meta) = lookup_posting_range(lookup, hash) else {
        return Ok(None);
    };
    let (start, end) = checked_posting_bounds(meta.offset, meta.len, postings.len())
        .context("loaded regex index has an invalid posting range")?;
    engine.postings_bytes_read = engine
        .postings_bytes_read
        .saturating_add(u64::from(meta.len));
    Ok(Some(decode_postings(&postings[start..end])?))
}

pub(crate) fn verify_path(
    root: &Path,
    path: &str,
    verifier: &Verifier,
    max_matches_per_file: Option<usize>,
) -> Result<Vec<SearchMatch>> {
    let bytes = fs::read(root.join(path)).with_context(|| {
        format!(
            "failed to read candidate file '{}'",
            root.join(path).display()
        )
    })?;
    match verifier {
        Verifier::Regex {
            regex,
            whole_file_prefilter,
        } => {
            let text = String::from_utf8_lossy(&bytes);
            if *whole_file_prefilter && !regex.is_match(text.as_ref()) {
                return Ok(Vec::new());
            }
            collect_line_matches(path, &text, max_matches_per_file, |line| {
                regex.is_match(line)
            })
        }
        Verifier::FixedBytes {
            needle,
            case_insensitive,
        } => {
            if !contains_fixed_bytes(&bytes, needle, *case_insensitive) {
                return Ok(Vec::new());
            }
            let text = String::from_utf8_lossy(&bytes);
            let normalized_needle = case_insensitive.then(|| normalize_for_index(needle));
            collect_line_matches(path, &text, max_matches_per_file, |line| {
                if *case_insensitive {
                    normalize_for_index(line.as_bytes())
                        .windows(needle.len())
                        .any(|window| window == normalized_needle.as_deref().unwrap_or_default())
                } else {
                    line.as_bytes()
                        .windows(needle.len())
                        .any(|window| window == needle.as_slice())
                }
            })
        }
    }
}

pub(crate) fn collect_line_matches<F>(
    path: &str,
    text: &str,
    max_matches_per_file: Option<usize>,
    mut predicate: F,
) -> Result<Vec<SearchMatch>>
where
    F: FnMut(&str) -> bool,
{
    let mut matches = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if !predicate(line) {
            continue;
        }
        matches.push(SearchMatch {
            path: path.to_string(),
            line: idx + 1,
            text: line.to_string(),
        });
        if max_matches_per_file.is_some_and(|limit| matches.len() >= limit) {
            break;
        }
    }
    Ok(matches)
}

pub(crate) fn contains_fixed_bytes(bytes: &[u8], needle: &[u8], case_insensitive: bool) -> bool {
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    if case_insensitive {
        let haystack = normalize_for_index(bytes);
        let normalized_needle = normalize_for_index(needle);
        haystack
            .windows(normalized_needle.len())
            .any(|window| window == normalized_needle.as_slice())
    } else {
        bytes.windows(needle.len()).any(|window| window == needle)
    }
}
pub(crate) fn should_fallback_to_rg(candidate_count: usize, all_path_count: usize) -> bool {
    if candidate_count == 0 {
        return true;
    }
    if all_path_count == 0 {
        return false;
    }
    candidate_count > MAX_INDEX_VERIFY_CANDIDATES
        || candidate_count.saturating_mul(MAX_INDEX_VERIFY_DENOMINATOR)
            > all_path_count.saturating_mul(MAX_INDEX_VERIFY_NUMERATOR)
}

pub(crate) fn should_stop_literal_refinement(
    candidate_count: usize,
    all_path_count: usize,
    covered_all: bool,
    selected_count: usize,
    materially_reduced: bool,
) -> bool {
    if candidate_count == 0 || selected_count >= MAX_LITERAL_COVER {
        return true;
    }
    if !covered_all {
        return false;
    }
    if !materially_reduced && selected_count >= 3 {
        return true;
    }
    candidate_count <= 8
        || candidate_count.saturating_mul(8) <= all_path_count
        || selected_count >= 4
}
pub(crate) fn prune_candidates_with_positions(
    loaded: &LoadedIndex,
    plan: &SearchPlan,
    candidates: &BTreeSet<String>,
    cache: &mut QueryCache,
) -> BTreeSet<String> {
    candidates
        .iter()
        .filter(|path| plan_window_for_path(loaded, path, plan, cache).is_some())
        .cloned()
        .collect()
}

pub(crate) fn plan_window_for_path(
    loaded: &LoadedIndex,
    path: &str,
    plan: &SearchPlan,
    cache: &mut QueryCache,
) -> Option<LiteralWindow> {
    match plan {
        SearchPlan::All => Some(LiteralWindow {
            earliest_bucket: 0,
            latest_bucket: (POSITION_BUCKET_COUNT as u8).saturating_sub(1),
        }),
        SearchPlan::Literal(literal) => literal_window_for_path(loaded, path, literal, cache),
        SearchPlan::Or(children) => {
            let mut earliest = u8::MAX;
            let mut latest = 0u8;
            let mut matched = false;
            for child in children {
                let Some(window) = plan_window_for_path(loaded, path, child, cache) else {
                    continue;
                };
                earliest = earliest.min(window.earliest_bucket);
                latest = latest.max(window.latest_bucket);
                matched = true;
            }
            matched.then_some(LiteralWindow {
                earliest_bucket: earliest,
                latest_bucket: latest,
            })
        }
        SearchPlan::And(children) => {
            let mut current: Option<LiteralWindow> = None;
            for child in children {
                let child_window = plan_window_for_path(loaded, path, child, cache)?;
                current = Some(match current {
                    None => child_window,
                    Some(existing) => {
                        if existing.earliest_bucket > child_window.latest_bucket {
                            return None;
                        }
                        LiteralWindow {
                            earliest_bucket: existing
                                .earliest_bucket
                                .min(child_window.earliest_bucket),
                            latest_bucket: existing.latest_bucket.max(child_window.latest_bucket),
                        }
                    }
                });
            }
            current
        }
    }
}

pub(crate) fn literal_window_for_path(
    loaded: &LoadedIndex,
    path: &str,
    literal: &[u8],
    cache: &mut QueryCache,
) -> Option<LiteralWindow> {
    let cache_key = (path.to_string(), literal.to_vec());
    if let Some(cached) = cache.literal_windows.get(&cache_key) {
        return *cached;
    }
    let hashes = cache
        .literal_hashes
        .get(literal)
        .cloned()
        .unwrap_or_else(|| {
            select_covering_candidates(loaded, literal)
                .into_iter()
                .map(|candidate| candidate.hash)
                .collect()
        });
    let repeat_requirements = cache
        .literal_repeat_requirements
        .get(literal)
        .cloned()
        .unwrap_or_else(|| repeat_requirements_for_literal(literal));
    if hashes.is_empty() {
        cache.literal_windows.insert(cache_key, None);
        return None;
    }
    let mut overlap_earliest = 0u8;
    let mut overlap_latest = (POSITION_BUCKET_COUNT as u8).saturating_sub(1);
    let mut union_earliest = u8::MAX;
    let mut union_latest = 0u8;
    for hash in hashes {
        let summary = lookup_summary_for_path(loaded, hash, path, cache)?;
        if repeat_requirements.get(&hash).copied().unwrap_or(false) && !summary.repeated() {
            cache.literal_windows.insert(cache_key, None);
            return None;
        }
        overlap_earliest = overlap_earliest.max(summary.first_bucket());
        overlap_latest = overlap_latest.min(summary.last_bucket());
        union_earliest = union_earliest.min(summary.first_bucket());
        union_latest = union_latest.max(summary.last_bucket());
    }
    let (earliest_bucket, latest_bucket) = if overlap_earliest <= overlap_latest {
        (overlap_earliest, overlap_latest)
    } else {
        (union_earliest, union_latest)
    };
    let window = Some(LiteralWindow {
        earliest_bucket,
        latest_bucket,
    });
    cache.literal_windows.insert(cache_key, window);
    window
}

pub(crate) fn repeat_requirements_for_literal(literal: &[u8]) -> HashMap<u64, bool> {
    let mut counts = HashMap::<u64, usize>::new();
    let normalized = normalize_for_index(literal);
    for gram in normalized.windows(SHORT_GRAM_BYTES) {
        *counts.entry(hash_bytes(gram)).or_insert(0) += 1;
    }
    for gram in normalized.windows(MIN_GRAM_BYTES) {
        *counts.entry(hash_bytes(gram)).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter_map(|(hash, count)| (count > 1).then_some((hash, true)))
        .collect()
}

pub(crate) fn hir_has_line_anchors(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Look(look) => matches!(
            look,
            regex_syntax::hir::Look::Start
                | regex_syntax::hir::Look::End
                | regex_syntax::hir::Look::StartLF
                | regex_syntax::hir::Look::EndLF
                | regex_syntax::hir::Look::StartCRLF
                | regex_syntax::hir::Look::EndCRLF
        ),
        HirKind::Capture(capture) => hir_has_line_anchors(&capture.sub),
        HirKind::Concat(children) | HirKind::Alternation(children) => {
            children.iter().any(hir_has_line_anchors)
        }
        HirKind::Repetition(repetition) => hir_has_line_anchors(&repetition.sub),
        HirKind::Empty | HirKind::Literal(_) | HirKind::Class(_) => false,
    }
}

pub(crate) fn lookup_summary_for_path(
    loaded: &LoadedIndex,
    hash: u64,
    path: &str,
    cache: &mut QueryCache,
) -> Option<PositionSummary> {
    if loaded.overlay_state.deleted_paths.contains(path) {
        return None;
    }
    if let Some(owner) = loaded.overlay_state.owners.get(path).copied() {
        let segment = loaded
            .overlays
            .iter()
            .find(|segment| segment.generation == owner)?;
        let doc_id = segment.layer.doc_ids_by_path.get(path).copied()?;
        return lookup_posting_entry(
            &segment.layer,
            LayerKind::Overlay(owner),
            hash,
            doc_id,
            cache,
        )
        .map(|entry| entry.summary);
    }
    if loaded.overlay_state.shadowed_paths.contains(path) {
        return None;
    }
    let doc_id = loaded.base.doc_ids_by_path.get(path).copied()?;
    lookup_posting_entry(&loaded.base, LayerKind::Base, hash, doc_id, cache)
        .map(|entry| entry.summary)
}

pub(crate) fn lookup_posting_entry(
    layer: &LoadedLayer,
    layer_kind: LayerKind,
    hash: u64,
    doc_id: u32,
    cache: &mut QueryCache,
) -> Option<PostingEntry> {
    let entries = cache
        .postings
        .entry((layer_kind, hash))
        .or_insert_with(|| lookup_doc_ids_quiet(layer, hash))
        .clone()?;
    entries.into_iter().find(|entry| entry.doc_id == doc_id)
}
pub(crate) fn requested_filter_set(paths: &[String]) -> Option<BTreeSet<String>> {
    (!paths.is_empty()).then(|| paths.iter().cloned().collect())
}

pub(crate) fn all_indexed_paths(
    loaded: &LoadedIndex,
    requested_filter: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for doc in &loaded.base.docs {
        if loaded.overlay_state.shadowed_paths.contains(&doc.path) {
            continue;
        }
        if path_allowed(&doc.path, requested_filter) {
            paths.insert(doc.path.clone());
        }
    }
    for segment in &loaded.overlays {
        for doc in &segment.layer.docs {
            if !overlay_doc_is_active(loaded, segment.generation, &doc.path) {
                continue;
            }
            if path_allowed(&doc.path, requested_filter) {
                paths.insert(doc.path.clone());
            }
        }
    }
    paths
}

pub(crate) fn overlay_doc_is_active(loaded: &LoadedIndex, generation: u64, path: &str) -> bool {
    !loaded.overlay_state.deleted_paths.contains(path)
        && loaded.overlay_state.owners.get(path) == Some(&generation)
}

pub(crate) fn overlay_posting_count(loaded: &LoadedIndex, hash: u64) -> u32 {
    loaded.overlays.iter().fold(0u32, |count, segment| {
        count.saturating_add(lookup_posting_count(&segment.layer, hash).unwrap_or(0))
    })
}

pub(crate) fn path_allowed(path: &str, requested_filter: Option<&BTreeSet<String>>) -> bool {
    requested_filter.is_none_or(|filters| {
        filters
            .iter()
            .any(|filter| path == filter || path.starts_with(&format!("{filter}/")))
    })
}
pub(crate) fn render_compact_preview(total_match_count: usize, groups: &[SearchGroup]) -> String {
    if total_match_count == 0 {
        return "Search found 0 matches.".to_string();
    }
    let mut lines = vec![format!(
        "Search found {} matches in {} files.",
        total_match_count,
        groups.len()
    )];
    for group in groups.iter().take(12) {
        lines.push(format!("- {} ({})", group.path, group.match_count));
    }
    if groups.len() > 12 {
        lines.push(format!("+{} more files", groups.len() - 12));
    }
    lines.join("\n")
}
