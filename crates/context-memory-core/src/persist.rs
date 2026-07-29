use super::*;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

const PERSIST_WAL_VERSION: u32 = 1;
const PERSIST_WAL_MAGIC: &[u8; 8] = b"P28CWAL1";
const PERSIST_WAL_HEADER_LEN: usize = 8 + 8 + 32;
const MAX_PERSIST_WAL_RECORD_BYTES: usize = 64 * 1024 * 1024;

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
    cache_key: String,
    target: String,
    input_hash: String,
    created_at_unix: u64,
    packets: Vec<PersistCachePacket>,
    metadata_json: String,
    delta_reuse: DeltaReuse,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Default, wincode::SchemaRead, wincode::SchemaWrite,
)]
pub(crate) struct PersistCachePacket {
    packet_id: Option<String>,
    body_json: String,
    token_usage: Option<u64>,
    runtime_ms: Option<u64>,
    metadata_json: String,
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

        if v3_cache.try_load_v3(config).is_some() {
            cache = v3_cache;
        } else if v2_cache.try_load_v2(config).is_some() {
            merge_eviction_counters(&mut v2_cache.eviction_counters, &v3_cache.eviction_counters);
            cache = v2_cache;
        } else {
            merge_eviction_counters(&mut cache.eviction_counters, &v3_cache.eviction_counters);
            merge_eviction_counters(&mut cache.eviction_counters, &v2_cache.eviction_counters);
            let _ = cache.try_load_v1(config);
        }

        match replay_wal(&mut cache, config) {
            Ok(replay) => {
                cache.persisted_sequence = replay.highest_sequence;
                if replay.recovered_corruption {
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

        let encoded = wincode::serialize(&envelope).map_err(|source| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to serialize cache envelope: {source}"),
            )
        })?;

        write_atomically(&path, &encoded)?;
        Ok(encoded.len() as u64)
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
        let raw = fs::read(persist_cache_path_v3(&config.root_dir)).ok()?;
        let envelope = match wincode::deserialize::<PersistEnvelopeV3>(&raw) {
            Ok(envelope) => envelope,
            Err(_) => {
                self.evict_reason(EvictionReason::CorruptLoadRecovery, 1);
                return None;
            }
        };
        if envelope.version != PERSIST_CACHE_VERSION {
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
        self.persisted_sequence = envelope.applied_wal_sequence;
        Some(())
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

pub(crate) fn persist_cache_path_v1(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_FILE_V1)
}

pub(crate) fn persist_cache_path_v2(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_FILE_V2)
}

pub(crate) fn persist_cache_path_v3(root: &Path) -> PathBuf {
    root.join(PERSIST_CACHE_DIR).join(PERSIST_CACHE_FILE_V3)
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
                recovered_corruption = true;
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
    let temp_path = path.with_extension("tmp");
    let mut temp_file = File::create(&temp_path)?;
    temp_file.write_all(bytes)?;
    temp_file.sync_all()?;
    drop(temp_file);

    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let mut destination = File::create(path)?;
            destination.write_all(bytes)?;
            destination.sync_all()?;
            let _ = fs::remove_file(&temp_path);
            Ok(())
        }
    }
}
