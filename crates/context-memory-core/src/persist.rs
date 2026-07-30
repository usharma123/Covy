use super::*;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use crate::cache_file::{create_unique_cache_temp_with, CacheFileError};

const PERSIST_WAL_VERSION: u32 = 1;
const PERSIST_WAL_MAGIC: &[u8; 8] = b"P28CWAL1";
const PERSIST_WAL_HEADER_LEN: usize = 8 + 8 + 32;
const MAX_PERSIST_WAL_RECORD_BYTES: usize = 64 * 1024 * 1024;
const PERSIST_CHECKPOINT_MAGIC: &[u8; 8] = b"P28CCP31";
const PERSIST_CHECKPOINT_HEADER_LEN: usize = 8 + 8 + 32;
const MAX_PERSIST_CHECKPOINT_BYTES: usize = 512 * 1024 * 1024;
const PERSIST_CACHE_BACKUP_FILE_V3: &str = "packet-cache-v3.backup.bin";

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
pub(crate) struct PersistEnvelopeV1 {
    pub(crate) version: u32,
    pub(crate) entries: Vec<PersistPacketCacheEntry>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
#[serde(default)]
pub(crate) struct PersistEnvelopeV2 {
    pub(crate) version: u32,
    pub(crate) entries: Vec<PersistPacketCacheEntry>,
    pub(crate) recall_docs: Vec<RecallDocument>,
    pub(crate) recall_postings: HashMap<String, Vec<(String, usize)>>,
    pub(crate) recall_avg_doc_length: f64,
    pub(crate) file_ref_index: HashMap<String, BTreeSet<String>>,
    pub(crate) basename_alias_index: HashMap<String, BTreeSet<String>>,
    pub(crate) symbol_index: HashMap<String, BTreeSet<String>>,
    pub(crate) test_index: HashMap<String, BTreeSet<String>>,
    pub(crate) task_index: HashMap<String, BTreeSet<String>>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
#[serde(default)]
pub(crate) struct PersistEnvelopeV3 {
    pub(crate) version: u32,
    pub(crate) applied_wal_sequence: u64,
    pub(crate) entries: Vec<PersistPacketCacheEntry>,
    pub(crate) recall_docs: Vec<RecallDocument>,
    pub(crate) recall_postings: HashMap<String, Vec<(String, usize)>>,
    pub(crate) recall_avg_doc_length: f64,
    pub(crate) file_ref_index: HashMap<String, BTreeSet<String>>,
    pub(crate) basename_alias_index: HashMap<String, BTreeSet<String>>,
    pub(crate) symbol_index: HashMap<String, BTreeSet<String>>,
    pub(crate) test_index: HashMap<String, BTreeSet<String>>,
    pub(crate) task_index: HashMap<String, BTreeSet<String>>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
pub(crate) struct PersistPacketCacheEntry {
    pub(crate) cache_key: String,
    pub(crate) target: String,
    pub(crate) input_hash: String,
    pub(crate) created_at_unix: u64,
    pub(crate) packets: Vec<PersistCachePacket>,
    pub(crate) metadata_json: String,
    pub(crate) delta_reuse: DeltaReuse,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
pub(crate) struct PersistCachePacket {
    pub(crate) packet_id: Option<String>,
    pub(crate) body_json: String,
    pub(crate) token_usage: Option<u64>,
    pub(crate) runtime_ms: Option<u64>,
    pub(crate) metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PersistDelta {
    Upsert { entry: PersistPacketCacheEntry },
    Remove { cache_key: String },
}

impl PersistDelta {
    pub(crate) fn upsert(entry: &PacketCacheEntry) -> Self {
        Self::Upsert {
            entry: PersistPacketCacheEntry::from_entry(entry),
        }
    }

    pub(crate) fn remove(cache_key: String) -> Self {
        Self::Remove { cache_key }
    }

    pub(crate) fn cache_key(&self) -> &str {
        match self {
            Self::Upsert { entry } => &entry.cache_key,
            Self::Remove { cache_key } => cache_key,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistWalRecord {
    version: u32,
    sequence: u64,
    deltas: Vec<PersistDelta>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WalReplay {
    pub(crate) highest_sequence: u64,
    pub(crate) valid_bytes: u64,
    pub(crate) recovered_corruption: bool,
    pub(crate) baseline_mismatch: bool,
}

impl PersistPacketCacheEntry {
    pub(crate) fn from_entry(entry: &PacketCacheEntry) -> Self {
        Self {
            cache_key: entry.cache_key.clone(),
            target: entry.target.clone(),
            input_hash: entry.input_hash.clone(),
            created_at_unix: entry.created_at_unix,
            packets: entry
                .packets
                .iter()
                .map(PersistCachePacket::from_cache_packet)
                .collect(),
            metadata_json: encode_json_value(&entry.metadata),
            delta_reuse: entry.delta_reuse.clone(),
        }
    }

    pub(crate) fn into_entry(self) -> PacketCacheEntry {
        PacketCacheEntry {
            cache_key: self.cache_key,
            target: self.target,
            input_hash: self.input_hash,
            created_at_unix: self.created_at_unix,
            packets: self
                .packets
                .into_iter()
                .map(PersistCachePacket::into_cache_packet)
                .collect(),
            metadata: decode_json_value(&self.metadata_json),
            delta_reuse: self.delta_reuse,
        }
    }
}

impl PersistCachePacket {
    pub(crate) fn from_cache_packet(packet: &CachePacket) -> Self {
        Self {
            packet_id: packet.packet_id.clone(),
            body_json: encode_json_value(&packet.body),
            token_usage: packet.token_usage,
            runtime_ms: packet.runtime_ms,
            metadata_json: encode_json_value(&packet.metadata),
        }
    }

    pub(crate) fn into_cache_packet(self) -> CachePacket {
        CachePacket {
            packet_id: self.packet_id,
            body: decode_json_value(&self.body_json),
            token_usage: self.token_usage,
            runtime_ms: self.runtime_ms,
            metadata: decode_json_value(&self.metadata_json),
        }
    }
}

impl PacketCache {
    pub fn load_from_disk(config: &PersistConfig) -> Self {
        let mut cache = Self {
            workspace_root: Some(config.root_dir.clone()),
            ..Self::new()
        };
        let mut v3_cache = Self {
            workspace_root: Some(config.root_dir.clone()),
            ..Self::new()
        };
        let mut v2_cache = Self {
            workspace_root: Some(config.root_dir.clone()),
            ..Self::new()
        };

        let loaded_v3 = v3_cache.try_load_v3(config).is_some();
        let wal_is_nonempty = fs::metadata(persist_cache_wal_path_v3(&config.root_dir))
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false);
        if loaded_v3 {
            cache = v3_cache;
        } else if !wal_is_nonempty && v2_cache.try_load_v2(config).is_some() {
            merge_eviction_counters(&mut v2_cache.eviction_counters, &v3_cache.eviction_counters);
            cache = v2_cache;
        } else if !wal_is_nonempty {
            merge_eviction_counters(&mut cache.eviction_counters, &v3_cache.eviction_counters);
            merge_eviction_counters(&mut cache.eviction_counters, &v2_cache.eviction_counters);
            let _ = cache.try_load_v1(config);
        } else {
            merge_eviction_counters(&mut cache.eviction_counters, &v3_cache.eviction_counters);
        }

        match replay_wal(&mut cache, config) {
            Ok(replay) => {
                cache.persisted_sequence = replay.highest_sequence;
                if replay.recovered_corruption || replay.baseline_mismatch {
                    cache.evict_reason(EvictionReason::CorruptLoadRecovery, 1);
                }
            }
            Err(_) => cache.evict_reason(EvictionReason::CorruptLoadRecovery, 1),
        }
        cache.rebuild_latest_request_index();
        cache.evict_expired(config.ttl_secs);
        if cache.recall_docs.is_empty() && !cache.entries_by_hash.is_empty() {
            cache.rebuild_indexes();
        }
        cache
    }

    pub fn save_to_disk(&self, config: &PersistConfig) -> Result<(), io::Error> {
        self.write_checkpoint(config)?;
        reset_wal(config)
    }

    pub(crate) fn write_checkpoint(&self, config: &PersistConfig) -> Result<u64, io::Error> {
        self.write_checkpoint_inner(config, || Ok(()))
    }

    fn write_checkpoint_inner<F>(
        &self,
        config: &PersistConfig,
        after_backup: F,
    ) -> Result<u64, io::Error>
    where
        F: FnOnce() -> Result<(), io::Error>,
    {
        let path = persist_cache_path_v3(&config.root_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let live_entries = self.collect_live_entries(config.ttl_secs);
        let live_keys = live_entries
            .iter()
            .map(|entry| entry.cache_key.clone())
            .collect::<BTreeSet<_>>();
        let envelope = PersistEnvelopeV3 {
            version: PERSIST_CACHE_VERSION,
            applied_wal_sequence: self.persisted_sequence,
            entries: live_entries,
            recall_docs: self
                .recall_docs
                .iter()
                .filter(|(cache_key, _)| live_keys.contains(*cache_key))
                .map(|(_, doc)| doc.clone())
                .collect(),
            recall_postings: filter_postings_for_live_keys(&self.recall_postings, &live_keys),
            recall_avg_doc_length: self.recall_avg_doc_length,
            file_ref_index: filter_ref_index_for_live_keys(&self.file_ref_index, &live_keys),
            basename_alias_index: filter_basename_alias_index_for_live_keys(
                &self.basename_alias_index,
                &live_keys,
                &self.file_ref_index,
            ),
            symbol_index: filter_ref_index_for_live_keys(&self.symbol_index, &live_keys),
            test_index: filter_ref_index_for_live_keys(&self.test_index, &live_keys),
            task_index: filter_ref_index_for_live_keys(&self.task_index, &live_keys),
        };

        validate_v3_envelope(&envelope)?;
        let payload = wincode::serialize(&envelope).map_err(|source| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to serialize cache envelope: {source}"),
            )
        })?;
        let encoded = encode_checkpoint_frame(&payload)?;

        write_atomically(&persist_cache_backup_path_v3(&config.root_dir), &encoded)?;
        after_backup()?;
        write_atomically(&path, &encoded)?;
        Ok(encoded.len() as u64)
    }

    #[cfg(test)]
    pub(crate) fn write_checkpoint_failing_after_backup(
        &self,
        config: &PersistConfig,
    ) -> Result<u64, io::Error> {
        self.write_checkpoint_inner(config, || {
            Err(io::Error::other(
                "injected crash after durable backup and before primary replace",
            ))
        })
    }

    pub fn persist_file_path(root: &Path) -> PathBuf {
        persist_cache_path_v3(root)
    }

    pub(crate) fn collect_live_entries(&self, ttl_secs: u64) -> Vec<PersistPacketCacheEntry> {
        let now = now_unix();
        self.entries_by_hash
            .values()
            .filter(|entry| !is_expired(entry.created_at_unix, ttl_secs, now))
            .map(PersistPacketCacheEntry::from_entry)
            .collect()
    }

    pub(crate) fn apply_persist_deltas(&mut self, sequence: u64, deltas: Vec<PersistDelta>) {
        for delta in deltas {
            match delta {
                PersistDelta::Upsert { entry } => {
                    let entry = entry.into_entry();
                    if entry.cache_key.trim().is_empty() {
                        continue;
                    }
                    if self.entries_by_hash.contains_key(&entry.cache_key) {
                        self.remove_index_for(&entry.cache_key);
                    }
                    self.entries_by_hash
                        .insert(entry.cache_key.clone(), entry.clone());
                    self.index_entry(&entry);
                }
                PersistDelta::Remove { cache_key } => {
                    self.entries_by_hash.remove(&cache_key);
                    self.remove_index_for(&cache_key);
                }
            }
        }
        self.persisted_sequence = sequence;
        self.rebuild_latest_request_index();
    }

    pub(crate) fn try_load_v3(&mut self, config: &PersistConfig) -> Option<()> {
        for path in [
            persist_cache_path_v3(&config.root_dir),
            persist_cache_backup_path_v3(&config.root_dir),
        ] {
            match read_v3_envelope(&path) {
                Ok(Some(envelope)) => {
                    self.load_v3_envelope(envelope);
                    self.has_v3_checkpoint_baseline = true;
                    return Some(());
                }
                Ok(None) => {}
                Err(CheckpointLoadError::VersionMismatch) => {
                    self.evict_reason(EvictionReason::VersionMismatch, 1);
                }
                Err(CheckpointLoadError::Corrupt) => {
                    self.evict_reason(EvictionReason::CorruptLoadRecovery, 1);
                }
            }
        }
        None
    }

    fn load_v3_envelope(&mut self, envelope: PersistEnvelopeV3) {
        self.load_envelope_state(
            envelope.entries,
            envelope.recall_docs,
            envelope.recall_postings,
            envelope.recall_avg_doc_length,
            envelope.file_ref_index,
            envelope.basename_alias_index,
            envelope.symbol_index,
            envelope.test_index,
            envelope.task_index,
        );
        self.persisted_sequence = envelope.applied_wal_sequence;
    }

    pub(crate) fn try_load_v2(&mut self, config: &PersistConfig) -> Option<()> {
        let raw = fs::read(persist_cache_path_v2(&config.root_dir)).ok()?;
        let envelope = match wincode::deserialize::<PersistEnvelopeV2>(&raw) {
            Ok(envelope) => envelope,
            Err(_) => {
                self.evict_reason(EvictionReason::CorruptLoadRecovery, 1);
                return None;
            }
        };
        if envelope.version != 2 {
            self.evict_reason(EvictionReason::VersionMismatch, 1);
            return None;
        }
        self.load_envelope_state(
            envelope.entries,
            envelope.recall_docs,
            envelope.recall_postings,
            envelope.recall_avg_doc_length,
            envelope.file_ref_index,
            envelope.basename_alias_index,
            envelope.symbol_index,
            envelope.test_index,
            envelope.task_index,
        );
        self.has_legacy_checkpoint_baseline = true;
        Some(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the versioned cache envelope"
    )]
    fn load_envelope_state(
        &mut self,
        entries: Vec<PersistPacketCacheEntry>,
        recall_docs: Vec<RecallDocument>,
        recall_postings: HashMap<String, Vec<(String, usize)>>,
        recall_avg_doc_length: f64,
        file_ref_index: HashMap<String, BTreeSet<String>>,
        basename_alias_index: HashMap<String, BTreeSet<String>>,
        symbol_index: HashMap<String, BTreeSet<String>>,
        test_index: HashMap<String, BTreeSet<String>>,
        task_index: HashMap<String, BTreeSet<String>>,
    ) {
        self.entries_by_hash.clear();
        for entry in entries {
            let entry = entry.into_entry();
            if !entry.cache_key.trim().is_empty() {
                self.entries_by_hash.insert(entry.cache_key.clone(), entry);
            }
        }
        self.recall_docs = recall_docs
            .into_iter()
            .map(|doc| (doc.cache_key.clone(), doc))
            .collect();
        self.recall_postings = recall_postings;
        self.recall_avg_doc_length = recall_avg_doc_length;
        self.recall_total_doc_length = self.recall_docs.values().map(|doc| doc.doc_length).sum();
        self.file_ref_index = file_ref_index;
        self.basename_alias_index = basename_alias_index;
        self.symbol_index = symbol_index;
        self.test_index = test_index;
        self.task_index = task_index;
    }

    pub(crate) fn try_load_v1(&mut self, config: &PersistConfig) -> Option<()> {
        let raw = fs::read(persist_cache_path_v1(&config.root_dir)).ok()?;
        let envelope = match wincode::deserialize::<PersistEnvelopeV1>(&raw) {
            Ok(envelope) => envelope,
            Err(_) => {
                self.evict_reason(EvictionReason::CorruptLoadRecovery, 1);
                return None;
            }
        };
        if envelope.version != 1 {
            self.evict_reason(EvictionReason::VersionMismatch, 1);
            return None;
        }
        self.entries_by_hash.clear();
        for entry in envelope.entries {
            let entry = entry.into_entry();
            if !entry.cache_key.trim().is_empty() {
                self.entries_by_hash.insert(entry.cache_key.clone(), entry);
            }
        }
        self.rebuild_indexes();
        self.has_legacy_checkpoint_baseline = true;
        Some(())
    }
}

fn merge_eviction_counters(target: &mut EvictionCounters, source: &EvictionCounters) {
    target.expired_ttl = target.expired_ttl.saturating_add(source.expired_ttl);
    target.manual_prune = target.manual_prune.saturating_add(source.manual_prune);
    target.version_mismatch = target
        .version_mismatch
        .saturating_add(source.version_mismatch);
    target.corrupt_load_recovery = target
        .corrupt_load_recovery
        .saturating_add(source.corrupt_load_recovery);
}

#[derive(Debug)]
enum CheckpointLoadError {
    VersionMismatch,
    Corrupt,
}

fn invalid_checkpoint(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.into())
}

pub(crate) fn encode_checkpoint_frame(payload: &[u8]) -> Result<Vec<u8>, io::Error> {
    if payload.len() > MAX_PERSIST_CHECKPOINT_BYTES {
        return Err(invalid_checkpoint(format!(
            "cache checkpoint is {} bytes; maximum is {MAX_PERSIST_CHECKPOINT_BYTES}",
            payload.len()
        )));
    }
    let mut frame = Vec::with_capacity(PERSIST_CHECKPOINT_HEADER_LEN + payload.len());
    frame.extend_from_slice(PERSIST_CHECKPOINT_MAGIC);
    frame.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    frame.extend_from_slice(blake3::hash(payload).as_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn decode_checkpoint_payload(raw: &[u8]) -> Result<&[u8], CheckpointLoadError> {
    if !raw.starts_with(PERSIST_CHECKPOINT_MAGIC) {
        // Cache state is disposable. Unframed V3 files cannot authenticate
        // structurally decodable payload changes, so they are rejected and
        // never trusted as a migration baseline.
        return Err(CheckpointLoadError::Corrupt);
    }
    if raw.len() < PERSIST_CHECKPOINT_HEADER_LEN {
        return Err(CheckpointLoadError::Corrupt);
    }
    let length_start = PERSIST_CHECKPOINT_MAGIC.len();
    let length_end = length_start + std::mem::size_of::<u64>();
    let mut length_bytes = [0u8; std::mem::size_of::<u64>()];
    length_bytes.copy_from_slice(&raw[length_start..length_end]);
    let payload_len = usize::try_from(u64::from_le_bytes(length_bytes))
        .ok()
        .filter(|length| *length <= MAX_PERSIST_CHECKPOINT_BYTES)
        .ok_or(CheckpointLoadError::Corrupt)?;
    let frame_len = PERSIST_CHECKPOINT_HEADER_LEN
        .checked_add(payload_len)
        .ok_or(CheckpointLoadError::Corrupt)?;
    if raw.len() != frame_len {
        return Err(CheckpointLoadError::Corrupt);
    }
    let checksum_start = length_end;
    let checksum_end = checksum_start + 32;
    let payload = &raw[PERSIST_CHECKPOINT_HEADER_LEN..];
    if blake3::hash(payload).as_bytes() != &raw[checksum_start..checksum_end] {
        return Err(CheckpointLoadError::Corrupt);
    }
    Ok(payload)
}

fn read_v3_envelope(path: &Path) -> Result<Option<PersistEnvelopeV3>, CheckpointLoadError> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CheckpointLoadError::Corrupt),
    };
    let envelope = decode_checkpoint_envelope_v3(&raw).map_err(|_| CheckpointLoadError::Corrupt)?;
    if envelope.version != PERSIST_CACHE_VERSION {
        return Err(CheckpointLoadError::VersionMismatch);
    }
    Ok(Some(envelope))
}

pub(crate) fn decode_checkpoint_envelope_v3(raw: &[u8]) -> Result<PersistEnvelopeV3, io::Error> {
    let payload = decode_checkpoint_payload(raw)
        .map_err(|_| invalid_checkpoint("cache checkpoint frame is corrupt"))?;
    let envelope = wincode::deserialize::<PersistEnvelopeV3>(payload)
        .map_err(|source| invalid_checkpoint(format!("cache checkpoint is corrupt: {source}")))?;
    validate_v3_envelope(&envelope)?;
    Ok(envelope)
}

fn validate_v3_envelope(envelope: &PersistEnvelopeV3) -> Result<(), io::Error> {
    let mut entries = HashMap::new();
    for entry in &envelope.entries {
        if entry.cache_key.trim().is_empty() {
            return Err(invalid_checkpoint("cache checkpoint contains an empty key"));
        }
        if entries.insert(entry.cache_key.as_str(), entry).is_some() {
            return Err(invalid_checkpoint(format!(
                "cache checkpoint contains duplicate key {}",
                entry.cache_key
            )));
        }
        serde_json::from_str::<Value>(&entry.metadata_json).map_err(|source| {
            invalid_checkpoint(format!(
                "cache checkpoint entry {} has invalid metadata JSON: {source}",
                entry.cache_key
            ))
        })?;
        for packet in &entry.packets {
            serde_json::from_str::<Value>(&packet.body_json).map_err(|source| {
                invalid_checkpoint(format!(
                    "cache checkpoint entry {} has invalid packet body JSON: {source}",
                    entry.cache_key
                ))
            })?;
            serde_json::from_str::<Value>(&packet.metadata_json).map_err(|source| {
                invalid_checkpoint(format!(
                    "cache checkpoint entry {} has invalid packet metadata JSON: {source}",
                    entry.cache_key
                ))
            })?;
        }
    }

    let indexes_are_empty = envelope.recall_docs.is_empty()
        && envelope.recall_postings.is_empty()
        && envelope.file_ref_index.is_empty()
        && envelope.basename_alias_index.is_empty()
        && envelope.symbol_index.is_empty()
        && envelope.test_index.is_empty()
        && envelope.task_index.is_empty();
    if indexes_are_empty {
        if !entries.is_empty() {
            return Err(invalid_checkpoint(
                "cache checkpoint with live entries is missing recall indexes",
            ));
        }
        if envelope.recall_avg_doc_length != 0.0 {
            return Err(invalid_checkpoint(
                "cache checkpoint has an average document length without recall indexes",
            ));
        }
        return Ok(());
    }

    let mut docs = HashMap::new();
    for doc in &envelope.recall_docs {
        let Some(entry) = entries.get(doc.cache_key.as_str()) else {
            return Err(invalid_checkpoint(format!(
                "recall document {} does not reference a live entry",
                doc.cache_key
            )));
        };
        if docs.insert(doc.cache_key.as_str(), doc).is_some() {
            return Err(invalid_checkpoint(format!(
                "cache checkpoint contains duplicate recall document {}",
                doc.cache_key
            )));
        }
        if doc.target != entry.target || doc.created_at_unix != entry.created_at_unix {
            return Err(invalid_checkpoint(format!(
                "recall document {} disagrees with its cache entry",
                doc.cache_key
            )));
        }
        let computed_length = doc.terms.values().try_fold(0usize, |total, count| {
            if *count == 0 {
                None
            } else {
                total.checked_add(*count)
            }
        });
        if computed_length != Some(doc.doc_length)
            || doc.terms.keys().any(|term| term.trim().is_empty())
        {
            return Err(invalid_checkpoint(format!(
                "recall document {} has invalid term frequencies",
                doc.cache_key
            )));
        }
    }
    if docs.len() != entries.len() {
        return Err(invalid_checkpoint(
            "cache checkpoint recall documents do not cover all live entries",
        ));
    }

    validate_recall_postings(&envelope.recall_postings, &docs)?;
    validate_ref_index(&envelope.file_ref_index, &docs, |doc| &doc.paths)?;
    validate_ref_index(&envelope.symbol_index, &docs, |doc| &doc.symbols)?;
    validate_ref_index(&envelope.test_index, &docs, |doc| &doc.tests)?;
    validate_ref_index(&envelope.task_index, &docs, |doc| &doc.task_ids)?;
    validate_basename_alias_index(&envelope.basename_alias_index, &envelope.file_ref_index)?;

    if !envelope.recall_avg_doc_length.is_finite()
        || envelope.recall_avg_doc_length.is_sign_negative()
    {
        return Err(invalid_checkpoint(
            "cache checkpoint has an invalid average document length",
        ));
    }
    let expected_average = envelope
        .recall_docs
        .iter()
        .map(|doc| doc.doc_length)
        .sum::<usize>() as f64
        / envelope.recall_docs.len() as f64;
    if (envelope.recall_avg_doc_length - expected_average).abs() > f64::EPSILON {
        return Err(invalid_checkpoint(
            "cache checkpoint average document length does not match recall documents",
        ));
    }
    Ok(())
}

fn validate_recall_postings(
    actual: &HashMap<String, Vec<(String, usize)>>,
    docs: &HashMap<&str, &RecallDocument>,
) -> Result<(), io::Error> {
    let mut expected = HashMap::<String, BTreeSet<(String, usize)>>::new();
    for doc in docs.values() {
        for (term, count) in &doc.terms {
            expected
                .entry(term.clone())
                .or_default()
                .insert((doc.cache_key.clone(), *count));
        }
    }
    let mut normalized = HashMap::<String, BTreeSet<(String, usize)>>::new();
    for (term, postings) in actual {
        if term.trim().is_empty() || postings.is_empty() {
            return Err(invalid_checkpoint(
                "cache checkpoint has an empty recall posting",
            ));
        }
        let values = normalized.entry(term.clone()).or_default();
        for (cache_key, count) in postings {
            if *count == 0
                || !docs.contains_key(cache_key.as_str())
                || !values.insert((cache_key.clone(), *count))
            {
                return Err(invalid_checkpoint(format!(
                    "cache checkpoint has an invalid recall posting for {term}"
                )));
            }
        }
    }
    if normalized != expected {
        return Err(invalid_checkpoint(
            "cache checkpoint recall postings disagree with recall documents",
        ));
    }
    Ok(())
}

fn validate_ref_index<F>(
    actual: &HashMap<String, BTreeSet<String>>,
    docs: &HashMap<&str, &RecallDocument>,
    values: F,
) -> Result<(), io::Error>
where
    F: Fn(&RecallDocument) -> &[String],
{
    let mut expected = HashMap::<String, BTreeSet<String>>::new();
    for doc in docs.values() {
        for value in values(doc) {
            expected
                .entry(value.clone())
                .or_default()
                .insert(doc.cache_key.clone());
        }
    }
    if actual != &expected {
        return Err(invalid_checkpoint(
            "cache checkpoint reference index disagrees with recall documents",
        ));
    }
    Ok(())
}

fn validate_basename_alias_index(
    actual: &HashMap<String, BTreeSet<String>>,
    file_ref_index: &HashMap<String, BTreeSet<String>>,
) -> Result<(), io::Error> {
    let mut expected = HashMap::<String, BTreeSet<String>>::new();
    for canonical in file_ref_index.keys() {
        if let Some(basename) = basename_alias(canonical) {
            expected
                .entry(basename)
                .or_default()
                .insert(canonical.clone());
        }
    }
    if actual != &expected {
        return Err(invalid_checkpoint(
            "cache checkpoint basename aliases disagree with file references",
        ));
    }
    Ok(())
}

pub(crate) fn persist_cache_path_v1(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_FILE_V1)
}

pub(crate) fn persist_cache_path_v2(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_FILE_V2)
}

pub(crate) fn persist_cache_path_v3(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_FILE_V3)
}

pub(crate) fn persist_cache_backup_path_v3(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR)
        .join(PERSIST_CACHE_BACKUP_FILE_V3)
}

pub(crate) fn persist_cache_wal_path_v3(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_WAL_FILE_V3)
}

pub(crate) fn append_wal_record(
    config: &PersistConfig,
    sequence: u64,
    deltas: &[PersistDelta],
) -> Result<u64, io::Error> {
    let path = persist_cache_wal_path_v3(&config.root_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let created = !path.exists();
    let payload = serde_json::to_vec(&PersistWalRecord {
        version: PERSIST_WAL_VERSION,
        sequence,
        deltas: deltas.to_vec(),
    })
    .map_err(|source| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to serialize cache WAL record: {source}"),
        )
    })?;
    if payload.len() > MAX_PERSIST_WAL_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "cache WAL record is {} bytes; maximum is {MAX_PERSIST_WAL_RECORD_BYTES}",
                payload.len()
            ),
        ));
    }

    let mut frame = Vec::with_capacity(PERSIST_WAL_HEADER_LEN + payload.len());
    frame.extend_from_slice(PERSIST_WAL_MAGIC);
    frame.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    frame.extend_from_slice(blake3::hash(&payload).as_bytes());
    frame.extend_from_slice(&payload);

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&frame)?;
    file.sync_data()?;
    if created {
        sync_parent_dir(&persist_cache_wal_path_v3(&config.root_dir))?;
    }
    Ok(frame.len() as u64)
}

pub(crate) fn replay_wal(
    cache: &mut PacketCache,
    config: &PersistConfig,
) -> Result<WalReplay, io::Error> {
    let path = persist_cache_wal_path_v3(&config.root_dir);
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(WalReplay {
                highest_sequence: cache.persisted_sequence,
                ..WalReplay::default()
            });
        }
        Err(source) => return Err(source),
    };
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;

    let mut offset = 0usize;
    let mut highest_sequence = cache.persisted_sequence;
    let mut recovered_corruption = false;
    let mut baseline_mismatch = false;
    while offset < raw.len() {
        let Some(header_end) = offset.checked_add(PERSIST_WAL_HEADER_LEN) else {
            recovered_corruption = true;
            break;
        };
        if header_end > raw.len() {
            recovered_corruption = true;
            break;
        }
        if &raw[offset..offset + PERSIST_WAL_MAGIC.len()] != PERSIST_WAL_MAGIC {
            recovered_corruption = true;
            break;
        }

        let length_start = offset + PERSIST_WAL_MAGIC.len();
        let length_end = length_start + std::mem::size_of::<u64>();
        let mut length_bytes = [0u8; std::mem::size_of::<u64>()];
        length_bytes.copy_from_slice(&raw[length_start..length_end]);
        let payload_len = match usize::try_from(u64::from_le_bytes(length_bytes)) {
            Ok(length) if length <= MAX_PERSIST_WAL_RECORD_BYTES => length,
            _ => {
                recovered_corruption = true;
                break;
            }
        };
        let Some(frame_end) = header_end.checked_add(payload_len) else {
            recovered_corruption = true;
            break;
        };
        if frame_end > raw.len() {
            recovered_corruption = true;
            break;
        }

        let checksum_start = length_end;
        let checksum_end = checksum_start + 32;
        let payload = &raw[header_end..frame_end];
        if blake3::hash(payload).as_bytes() != &raw[checksum_start..checksum_end] {
            recovered_corruption = true;
            break;
        }
        let record = match serde_json::from_slice::<PersistWalRecord>(payload) {
            Ok(record) if record.version == PERSIST_WAL_VERSION => record,
            _ => {
                recovered_corruption = true;
                break;
            }
        };

        if record.sequence > highest_sequence {
            if record.sequence != highest_sequence.saturating_add(1) {
                baseline_mismatch = true;
                break;
            }
            highest_sequence = record.sequence;
            cache.apply_persist_deltas(record.sequence, record.deltas);
        }
        offset = frame_end;
    }

    Ok(WalReplay {
        highest_sequence,
        valid_bytes: offset as u64,
        recovered_corruption,
        baseline_mismatch,
    })
}

pub(crate) fn reset_wal(config: &PersistConfig) -> Result<(), io::Error> {
    truncate_wal_to(config, 0)
}

pub(crate) fn truncate_wal_to(config: &PersistConfig, length: u64) -> Result<(), io::Error> {
    let path = persist_cache_wal_path_v3(&config.root_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    file.set_len(length)?;
    file.sync_all()
}

pub(crate) fn filter_postings_for_live_keys(
    postings: &HashMap<String, Vec<(String, usize)>>,
    live_keys: &BTreeSet<String>,
) -> HashMap<String, Vec<(String, usize)>> {
    postings
        .iter()
        .filter_map(|(term, values)| {
            let filtered = values
                .iter()
                .filter(|(cache_key, _)| live_keys.contains(cache_key))
                .cloned()
                .collect::<Vec<_>>();
            (!filtered.is_empty()).then(|| (term.clone(), filtered))
        })
        .collect()
}

pub(crate) fn filter_ref_index_for_live_keys(
    index: &HashMap<String, BTreeSet<String>>,
    live_keys: &BTreeSet<String>,
) -> HashMap<String, BTreeSet<String>> {
    index
        .iter()
        .filter_map(|(term, values)| {
            let filtered = values
                .iter()
                .filter(|cache_key| live_keys.contains(*cache_key))
                .cloned()
                .collect::<BTreeSet<_>>();
            (!filtered.is_empty()).then(|| (term.clone(), filtered))
        })
        .collect()
}

pub(crate) fn filter_basename_alias_index_for_live_keys(
    index: &HashMap<String, BTreeSet<String>>,
    live_keys: &BTreeSet<String>,
    file_ref_index: &HashMap<String, BTreeSet<String>>,
) -> HashMap<String, BTreeSet<String>> {
    index
        .iter()
        .filter_map(|(basename, canonicals)| {
            let filtered = canonicals
                .iter()
                .filter(|canonical| {
                    file_ref_index
                        .get(*canonical)
                        .map(|cache_keys| {
                            cache_keys
                                .iter()
                                .any(|cache_key| live_keys.contains(cache_key))
                        })
                        .unwrap_or(false)
                })
                .cloned()
                .collect::<BTreeSet<_>>();
            (!filtered.is_empty()).then(|| (basename.clone(), filtered))
        })
        .collect()
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), io::Error> {
    write_atomically_with(path, bytes, |source, destination| {
        fs::rename(source, destination)
    })
}

fn write_atomically_with<F>(path: &Path, bytes: &[u8], replace: F) -> Result<(), io::Error>
where
    F: FnOnce(&Path, &Path) -> Result<(), io::Error>,
{
    write_atomically_with_observers(path, bytes, |_| Ok(()), |_| Ok(()), replace)
}

fn write_atomically_with_observers<B, A, F>(
    path: &Path,
    bytes: &[u8],
    before_temp_open: B,
    after_temp_sync: A,
    replace: F,
) -> Result<(), io::Error>
where
    B: FnMut(&Path) -> Result<(), io::Error>,
    A: FnOnce(&Path) -> Result<(), io::Error>,
    F: FnOnce(&Path, &Path) -> Result<(), io::Error>,
{
    let mut temporary =
        create_unique_cache_temp_with(path, before_temp_open).map_err(CacheFileError::into_io)?;
    temporary
        .file_mut()
        .write_all(bytes)
        .map_err(|source| CacheFileError::io(temporary.path(), source).into_io())?;
    temporary
        .file_mut()
        .sync_all()
        .map_err(|source| CacheFileError::io(temporary.path(), source).into_io())?;
    after_temp_sync(temporary.path())?;
    temporary
        .validate_attachment()
        .map_err(CacheFileError::into_io)?;
    replace(temporary.path(), path)?;
    temporary.mark_published();
    sync_parent_dir(path)
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> Result<(), io::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn failed_atomic_replace_never_truncates_existing_destination() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("checkpoint.bin");
        fs::write(&path, b"durable-old-checkpoint").unwrap();

        let error = write_atomically_with(&path, b"new-checkpoint", |_source, _destination| {
            Err(io::Error::other("injected atomic replace failure"))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(fs::read(path).unwrap(), b"durable-old-checkpoint");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_retries_a_symlinked_temp_candidate_without_touching_its_target() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let path = dir.path().join("checkpoint.bin");
        let sentinel = outside.path().join("sentinel");
        fs::write(&sentinel, b"outside-must-survive").unwrap();
        let mut planted = None;

        write_atomically_with_observers(
            &path,
            b"new-checkpoint",
            |candidate| {
                if planted.is_none() {
                    symlink(&sentinel, candidate)?;
                    planted = Some(candidate.to_path_buf());
                }
                Ok(())
            },
            |_| Ok(()),
            |source, destination| fs::rename(source, destination),
        )
        .unwrap();

        assert_eq!(
            (
                fs::read(&sentinel).unwrap(),
                fs::read(&path).unwrap(),
                fs::symlink_metadata(planted.unwrap())
                    .unwrap()
                    .file_type()
                    .is_symlink(),
            ),
            (
                b"outside-must-survive".to_vec(),
                b"new-checkpoint".to_vec(),
                true,
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_rejects_a_temp_symlink_swap_without_touching_its_target() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let path = dir.path().join("checkpoint.bin");
        let held = dir.path().join("held-temp");
        let sentinel = outside.path().join("sentinel");
        fs::write(&path, b"durable-old-checkpoint").unwrap();
        fs::write(&sentinel, b"outside-must-survive").unwrap();
        let replace_called = std::cell::Cell::new(false);

        let error = write_atomically_with_observers(
            &path,
            b"new-checkpoint",
            |_| Ok(()),
            |temporary| {
                fs::rename(temporary, &held)?;
                symlink(&sentinel, temporary)
            },
            |_source, _destination| {
                replace_called.set(true);
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(
            (
                error.kind(),
                replace_called.get(),
                fs::read(&sentinel).unwrap(),
                fs::read(&path).unwrap(),
                fs::read(&held).unwrap(),
            ),
            (
                io::ErrorKind::PermissionDenied,
                false,
                b"outside-must-survive".to_vec(),
                b"durable-old-checkpoint".to_vec(),
                b"new-checkpoint".to_vec(),
            )
        );
    }
}
