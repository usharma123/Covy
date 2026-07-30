//! Durable generation reservation, writer serialization, and manifest snapshots.

use std::fs::{self, File, OpenOptions};
use std::path::Path;

use fs2::FileExt;

use crate::error::{Result, SearchError};
use crate::layer::{artifact_digest, write_atomic, write_immutable};
use crate::model::{RegexGenerationRecord, RegexIndexManifest, WRITER_LOCK_FILE_NAME};
use crate::paths::{
    generation_high_water_path, generation_record_path, manifest_path, previous_manifest_path,
    regex_index_dir,
};
use crate::support::{ensure_valid_index, ResultContext};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ManifestFilesSnapshot {
    pub(crate) current: Option<Vec<u8>>,
    pub(crate) previous: Option<Vec<u8>>,
}

pub(crate) fn capture_manifest_files(root: &Path) -> Result<ManifestFilesSnapshot> {
    Ok(ManifestFilesSnapshot {
        current: read_optional_file(&manifest_path(root))?,
        previous: read_optional_file(&previous_manifest_path(root))?,
    })
}

pub(crate) fn ensure_manifest_files_unchanged(
    root: &Path,
    expected: &ManifestFilesSnapshot,
) -> Result<()> {
    if capture_manifest_files(root)? == *expected {
        return Ok(());
    }
    Err(SearchError::corrupt(
        "regex index manifests changed while the writer lock was held",
    ))
}

pub(crate) fn restore_owned_manifest_files(
    root: &Path,
    owned: &ManifestFilesSnapshot,
    target: &ManifestFilesSnapshot,
) -> Result<()> {
    let actual = capture_manifest_files(root)?;
    ensure_valid_index!(
        (actual.current == owned.current || actual.current == target.current)
            && (actual.previous == owned.previous || actual.previous == target.previous),
        "regex manifests changed before rollback"
    );
    restore_owned_optional_file(
        manifest_path(root),
        owned.current.as_deref(),
        target.current.as_deref(),
    )?;
    restore_owned_optional_file(
        previous_manifest_path(root),
        owned.previous.as_deref(),
        target.previous.as_deref(),
    )
}

fn restore_owned_optional_file(
    path: std::path::PathBuf,
    owned: Option<&[u8]>,
    target: Option<&[u8]>,
) -> Result<()> {
    let actual = read_optional_file(&path)?;
    ensure_valid_index!(
        actual.as_deref() == owned || actual.as_deref() == target,
        "regex manifest '{}' changed before rollback",
        path.display()
    );
    if actual.as_deref() != target {
        restore_optional_file(path, target)?;
    }
    Ok(())
}

fn restore_optional_file(path: std::path::PathBuf, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => write_atomic(path, bytes),
        None => match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!("failed to remove rolled-back manifest '{}'", path.display())
            }),
        },
    }
}

pub(crate) fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read regex metadata '{}'", path.display())),
    }
}

pub(crate) fn save_generation_record(root: &Path, record: &RegexGenerationRecord) -> Result<()> {
    fs::create_dir_all(regex_index_dir(root))?;
    write_immutable(
        generation_record_path(root, record.generation),
        &serde_json::to_vec_pretty(record)?,
    )
}

pub(crate) fn load_generation_record(
    root: &Path,
    generation: u64,
) -> Result<RegexGenerationRecord> {
    let path = generation_record_path(root, generation);
    let raw = fs::read(&path).with_context(|| {
        format!(
            "failed to read regex generation record '{}'",
            path.display()
        )
    })?;
    serde_json::from_slice(&raw).with_context(|| {
        format!(
            "failed to decode regex generation record '{}'",
            path.display()
        )
    })
}

pub(crate) fn generation_record_fingerprint(record: &RegexGenerationRecord) -> Result<String> {
    let mut canonical = record.clone();
    canonical.manifest.publication_fingerprint = None;
    Ok(artifact_digest(&serde_json::to_vec(&canonical)?))
}

pub(crate) fn seal_generation_record(
    manifest: &mut RegexIndexManifest,
    record: &mut RegexGenerationRecord,
) -> Result<String> {
    manifest.publication_fingerprint = None;
    record.manifest = manifest.clone();
    let fingerprint = generation_record_fingerprint(record)?;
    manifest.publication_fingerprint = Some(fingerprint.clone());
    record.manifest = manifest.clone();
    Ok(fingerprint)
}

pub(crate) fn reserve_generation(root: &Path, _writer: &GenerationWriterLock) -> Result<u64> {
    let high_water = reconcile_generation_high_water(root)?;
    let generation = high_water.checked_add(1).ok_or_else(|| {
        SearchError::corrupt(format!(
            "regex index generation space is exhausted at {high_water}"
        ))
    })?;
    write_atomic(
        generation_high_water_path(root),
        &serde_json::to_vec(&generation)?,
    )?;
    Ok(generation)
}

pub(crate) fn fence_generation_before_clear(
    root: &Path,
    _writer: &GenerationWriterLock,
) -> Result<()> {
    reconcile_generation_high_water(root).map(|_| ())
}

fn reconcile_generation_high_water(root: &Path) -> Result<u64> {
    let persisted = read_generation_high_water(root)?;
    let observed = observed_generation_high_water(root)?;
    let high_water = persisted.unwrap_or(0).max(observed);
    if observed > persisted.unwrap_or(0) {
        write_atomic(
            generation_high_water_path(root),
            &serde_json::to_vec(&high_water)?,
        )?;
    }
    Ok(high_water)
}

pub(crate) fn read_generation_high_water(root: &Path) -> Result<Option<u64>> {
    let path = generation_high_water_path(root);
    let Some(raw) = read_optional_file(&path)? else {
        return Ok(None);
    };
    let generation = serde_json::from_slice(&raw).with_context(|| {
        format!(
            "failed to decode regex generation high-water mark '{}'",
            path.display()
        )
    })?;
    Ok(Some(generation))
}

fn observed_generation_high_water(root: &Path) -> Result<u64> {
    let mut high_water = 0;
    for path in [manifest_path(root), previous_manifest_path(root)] {
        match fs::read(&path) {
            Ok(raw) => {
                if let Ok(manifest) = serde_json::from_slice::<RegexIndexManifest>(&raw) {
                    high_water = high_water.max(manifest.generation);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect regex generation metadata '{}'",
                        path.display()
                    )
                });
            }
        }
    }
    let directory = regex_index_dir(root);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(high_water),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect regex generation directory '{}'",
                    directory.display()
                )
            });
        }
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(generation) = generation_from_artifact_name(name) {
            high_water = high_water.max(generation);
        }
    }
    Ok(high_water)
}

fn generation_from_artifact_name(name: &str) -> Option<u64> {
    ["generation-", "base-", "overlay-"]
        .into_iter()
        .find_map(|prefix| {
            let rest = name.strip_prefix(prefix)?;
            let digits = rest.get(..20)?;
            (digits.bytes().all(|byte| byte.is_ascii_digit())
                && rest
                    .as_bytes()
                    .get(20)
                    .is_none_or(|byte| !byte.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
        })
}

pub(crate) struct GenerationWriterLock(File);

impl Drop for GenerationWriterLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(crate) fn acquire_writer_lock(root: &Path) -> Result<GenerationWriterLock> {
    let parent = root.join(".packet28").join("index");
    fs::create_dir_all(&parent)?;
    let path = parent.join(WRITER_LOCK_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("failed to open regex writer lock '{}'", path.display()))?;
    FileExt::lock_exclusive(&file)
        .with_context(|| format!("failed to acquire regex writer lock '{}'", path.display()))?;
    Ok(GenerationWriterLock(file))
}
