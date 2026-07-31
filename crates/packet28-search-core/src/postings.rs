//! Gram derivation and postings-table codec.

use std::collections::{BTreeSet, HashMap};

use crate::error::{Result, SearchError};
use crate::model::{
    IndexedGram, LookupPostingMeta, PositionSummary, PostingEntry, SparseCandidate,
    LOOKUP_ROW_BYTES, MAX_GRAM_BYTES, MIN_GRAM_BYTES, POSITION_BUCKET_COUNT, SHORT_GRAM_BYTES,
};
use crate::support::{ensure_valid_index, ResultContext};
use crate::weights::pair_weight;

pub(crate) fn checked_posting_bounds(
    offset: u64,
    len: u32,
    postings_len: usize,
) -> Result<(usize, usize)> {
    let end = offset.checked_add(u64::from(len)).ok_or_else(|| {
        SearchError::corrupt(format!(
            "posting range offset {offset} + length {len} overflows u64"
        ))
    })?;
    let postings_len =
        u64::try_from(postings_len).context("postings file length does not fit u64")?;
    ensure_valid_index!(
        end <= postings_len,
        "posting range {offset}..{end} exceeds postings length {postings_len}"
    );
    Ok((
        usize::try_from(offset).context("posting offset does not fit usize")?,
        usize::try_from(end).context("posting end does not fit usize")?,
    ))
}

pub(crate) fn encode_postings(entries: &[PostingEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut previous = 0u32;
    for entry in entries {
        let delta = entry.doc_id.saturating_sub(previous);
        encode_varint(delta, &mut out);
        previous = entry.doc_id;
    }
    for entry in entries {
        out.extend_from_slice(&entry.summary.encode());
    }
    out
}

pub(crate) fn decode_postings(bytes: &[u8]) -> Result<Vec<PostingEntry>> {
    if bytes.len() < 4 {
        return Err(SearchError::corrupt("invalid posting block"));
    }
    let count =
        u32::from_le_bytes(read_fixed_width::<4>(bytes, 0, "posting document count")?) as usize;
    let minimum_len = count
        .checked_mul(3)
        .and_then(|len| len.checked_add(4))
        .ok_or_else(|| {
            SearchError::corrupt("posting block document count overflows its encoded size")
        })?;
    ensure_valid_index!(
        bytes.len() >= minimum_len,
        "posting block declares {count} documents but is only {} bytes",
        bytes.len()
    );
    let mut doc_ids = Vec::with_capacity(count);
    let mut index = 4usize;
    let mut current = 0u32;
    for position in 0..count {
        let (delta, consumed) = decode_varint(&bytes[index..])?;
        ensure_valid_index!(
            position == 0 || delta > 0,
            "posting block document ids are not strictly increasing"
        );
        current = current
            .checked_add(delta)
            .ok_or_else(|| SearchError::corrupt("posting document id delta overflows u32"))?;
        doc_ids.push(current);
        index += consumed;
    }
    let summary_len = count
        .checked_mul(2)
        .ok_or_else(|| SearchError::corrupt("posting summary length overflows usize"))?;
    let summary_end = index
        .checked_add(summary_len)
        .ok_or_else(|| SearchError::corrupt("posting summary range overflows usize"))?;
    ensure_valid_index!(
        bytes.len() >= summary_end,
        "posting block missing positional summaries"
    );
    ensure_valid_index!(
        bytes.len() == summary_end,
        "posting block has {} trailing bytes",
        bytes.len() - summary_end
    );
    ensure_valid_index!(
        bytes[index..summary_end]
            .chunks_exact(2)
            .all(|summary| summary[1] <= 1),
        "posting block contains an invalid repeated-position flag"
    );
    Ok(doc_ids
        .into_iter()
        .enumerate()
        .map(|(offset, doc_id)| PostingEntry {
            doc_id,
            summary: PositionSummary::decode([
                bytes[index + (offset * 2)],
                bytes[index + (offset * 2) + 1],
            ]),
        })
        .collect())
}

pub(crate) fn encode_varint(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

pub(crate) fn decode_varint(bytes: &[u8]) -> Result<(u32, usize)> {
    let mut result = 0u32;
    for (idx, byte) in bytes.iter().take(5).enumerate() {
        if idx == 4 && *byte > 0x0f {
            return Err(SearchError::corrupt("varint overflows u32"));
        }
        let value = u32::from(byte & 0x7f);
        result |= value << (idx * 7);
        if byte & 0x80 == 0 {
            return Ok((result, idx + 1));
        }
    }
    if bytes.len() >= 5 {
        return Err(SearchError::corrupt("varint overflows u32"));
    }
    Err(SearchError::corrupt("unterminated varint"))
}

pub(crate) fn lookup_posting_range(lookup: &[u8], hash: u64) -> Option<LookupPostingMeta> {
    debug_assert_eq!(
        lookup.len() % LOOKUP_ROW_BYTES,
        0,
        "lookup bytes must be validated before querying"
    );
    let rows = lookup.len() / LOOKUP_ROW_BYTES;
    let mut low = 0usize;
    let mut high = rows;
    while low < high {
        let mid = low + (high - low) / 2;
        let start = mid * LOOKUP_ROW_BYTES;
        let current = u64::from_le_bytes(lookup[start..start + 8].try_into().ok()?);
        if current == hash {
            let offset = u64::from_le_bytes(lookup[start + 8..start + 16].try_into().ok()?);
            let len = u32::from_le_bytes(lookup[start + 16..start + 20].try_into().ok()?);
            let doc_count = u32::from_le_bytes(lookup[start + 20..start + 24].try_into().ok()?);
            return Some(LookupPostingMeta {
                offset,
                len,
                doc_count,
            });
        }
        if current < hash {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    None
}

pub(crate) fn build_indexed_grams(bytes: &[u8]) -> Vec<IndexedGram> {
    let normalized = normalize_for_index(bytes);
    let mut by_hash = HashMap::<u64, PositionSummary>::new();
    for (start, gram) in contiguous_short_grams(&normalized) {
        add_indexed_gram(&mut by_hash, hash_bytes(&gram), start, normalized.len());
    }
    for (start, gram) in contiguous_trigrams(&normalized) {
        add_indexed_gram(&mut by_hash, hash_bytes(&gram), start, normalized.len());
    }
    for candidate in collect_sparse_candidates(&normalized) {
        add_indexed_gram(
            &mut by_hash,
            candidate.hash,
            candidate.start,
            normalized.len(),
        );
    }
    let mut grams = by_hash
        .into_iter()
        .map(|(hash, summary)| IndexedGram { hash, summary })
        .collect::<Vec<_>>();
    grams.sort_by_key(|gram| gram.hash);
    grams
}

pub(crate) fn add_indexed_gram(
    by_hash: &mut HashMap<u64, PositionSummary>,
    hash: u64,
    start: usize,
    byte_len: usize,
) {
    let bucket = bucket_for_offset(start, byte_len);
    by_hash
        .entry(hash)
        .and_modify(|summary| summary.update(bucket))
        .or_insert_with(|| PositionSummary::new(bucket));
}

pub(crate) fn build_covering_hashes(literal: &[u8]) -> Vec<u64> {
    build_covering_candidates(literal)
        .into_iter()
        .map(|candidate| candidate.hash)
        .collect()
}

pub(crate) fn build_covering_candidates(literal: &[u8]) -> Vec<SparseCandidate> {
    let normalized = normalize_for_index(literal);
    if normalized.len() == SHORT_GRAM_BYTES {
        return vec![SparseCandidate {
            hash: hash_bytes(&normalized),
            score: literal_score(&normalized),
            start: 0,
            end: normalized.len(),
        }];
    }
    if normalized.len() < MIN_GRAM_BYTES {
        return Vec::new();
    }
    let mut candidates = collect_sparse_candidates(&normalized);
    if candidates.is_empty() {
        candidates = contiguous_trigrams(&normalized)
            .into_iter()
            .map(|(start, gram)| SparseCandidate {
                hash: hash_bytes(&gram),
                score: literal_score(&gram),
                start,
                end: start + gram.len(),
            })
            .collect();
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.hash.cmp(&right.hash))
    });
    candidates
        .into_iter()
        .fold(
            (BTreeSet::new(), Vec::new()),
            |(mut seen, mut items), candidate| {
                if seen.insert(candidate.hash) {
                    items.push(candidate);
                }
                (seen, items)
            },
        )
        .1
}

pub(crate) fn collect_sparse_candidates(bytes: &[u8]) -> Vec<SparseCandidate> {
    if bytes.len() < MIN_GRAM_BYTES + 1 {
        return Vec::new();
    }
    let weights = pair_weights_for_bytes(bytes);
    let prefixes = pair_weight_prefix_sums(&weights);
    let mut grams = Vec::new();
    for start in 0..=bytes.len() - MIN_GRAM_BYTES {
        let limit = (start + MAX_GRAM_BYTES).min(bytes.len());
        for end in (start + MIN_GRAM_BYTES + 1)..=limit {
            if !is_sparse_candidate_range(&weights, start, end) {
                continue;
            }
            grams.push(SparseCandidate {
                hash: hash_bytes(&bytes[start..end]),
                score: literal_score_range(&prefixes, start, end),
                start,
                end,
            });
        }
    }
    grams
}

pub(crate) fn contiguous_trigrams(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    if bytes.len() < MIN_GRAM_BYTES {
        return Vec::new();
    }
    bytes
        .windows(MIN_GRAM_BYTES)
        .enumerate()
        .map(|(start, window)| (start, window.to_vec()))
        .collect()
}

pub(crate) fn contiguous_short_grams(bytes: &[u8]) -> Vec<(usize, Vec<u8>)> {
    if bytes.len() < SHORT_GRAM_BYTES {
        return Vec::new();
    }
    bytes
        .windows(SHORT_GRAM_BYTES)
        .enumerate()
        .map(|(start, window)| (start, window.to_vec()))
        .collect()
}

pub(crate) fn pair_weights_for_bytes(bytes: &[u8]) -> Vec<u32> {
    bytes
        .windows(2)
        .map(|pair| pair_weight(pair[0], pair[1]))
        .collect()
}

pub(crate) fn pair_weight_prefix_sums(weights: &[u32]) -> Vec<u32> {
    let mut prefix = Vec::with_capacity(weights.len() + 1);
    prefix.push(0u32);
    for weight in weights {
        prefix.push(
            prefix
                .last()
                .copied()
                .unwrap_or_default()
                .saturating_add(*weight),
        );
    }
    prefix
}

pub(crate) fn is_sparse_candidate_range(weights: &[u32], start: usize, end: usize) -> bool {
    if end.saturating_sub(start) < MIN_GRAM_BYTES + 1 {
        return false;
    }
    let edge_left = weights[start];
    let edge_right = weights[end - 2];
    let interior_max = weights[start + 1..end - 2]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);
    edge_left > interior_max && edge_right > interior_max
}

pub(crate) fn literal_score_range(prefixes: &[u32], start: usize, end: usize) -> u32 {
    let pair_score = prefixes[end - 1].saturating_sub(prefixes[start]);
    pair_score.saturating_add((end - start) as u32 * 32)
}

pub(crate) fn literal_score(bytes: &[u8]) -> u32 {
    let pair_score = bytes
        .windows(2)
        .map(|pair| pair_weight(pair[0], pair[1]))
        .sum::<u32>();
    pair_score.saturating_add((bytes.len() as u32) * 32)
}

pub(crate) fn normalize_for_index(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|byte| byte.to_ascii_lowercase()).collect()
}
pub(crate) fn bucket_for_offset(offset: usize, byte_len: usize) -> u8 {
    if byte_len <= 1 {
        return 0;
    }
    ((offset.saturating_mul(POSITION_BUCKET_COUNT)) / byte_len)
        .min(POSITION_BUCKET_COUNT.saturating_sub(1)) as u8
}
pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    let digest = blake3::hash(bytes);
    let bytes = digest.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

pub(crate) fn read_fixed_width<const WIDTH: usize>(
    bytes: &[u8],
    offset: usize,
    field: &str,
) -> Result<[u8; WIDTH]> {
    let end = offset
        .checked_add(WIDTH)
        .ok_or_else(|| SearchError::corrupt(format!("{field} offset overflow")))?;
    let raw = bytes.get(offset..end).ok_or_else(|| {
        SearchError::corrupt(format!(
            "{field} requires {WIDTH} bytes at offset {offset}, but input has {} bytes",
            bytes.len()
        ))
    })?;
    raw.try_into()
        .map_err(|_| SearchError::corrupt(format!("{field} has an invalid encoded width")))
}
