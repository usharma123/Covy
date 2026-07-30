extern crate packet28_binary_codec as wincode;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod cache;
mod cache_file;
mod normalize;
mod persist;
mod persistence_owner;
mod recall;
mod recall_document;
mod types;

pub use cache::PacketCache;
pub(crate) use cache::*;
pub use cache_file::CachePathViolation;
pub(crate) use normalize::*;
pub use normalize::{basename_alias, normalize_context_path};
#[cfg(test)]
pub(crate) use persist::*;
pub use persistence_owner::{
    CacheMutationReservation, CachePersistence, CachePersistenceError, CachePersistenceMetrics,
};
pub(crate) use recall_document::*;
pub use types::*;

const PERSIST_CACHE_VERSION: u32 = 3;
const PERSIST_CACHE_DIR: &str = ".packet28";
const PERSIST_CACHE_FILE_V1: &str = "packet-cache-v1.bin";
const PERSIST_CACHE_FILE_V2: &str = "packet-cache-v2.bin";
const PERSIST_CACHE_FILE_V3: &str = "packet-cache-v3.bin";
const PERSIST_CACHE_WAL_FILE_V3: &str = "packet-cache-v3.wal";
