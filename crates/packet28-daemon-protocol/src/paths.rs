//! Deterministic daemon endpoint and artifact paths.

use std::fmt;
use std::str::FromStr;

use super::*;

pub const DAEMON_DIR_NAME: &str = ".packet28/daemon";
pub const SOCKET_FILE_NAME: &str = "packet28d.sock";
pub const PID_FILE_NAME: &str = "pid";
pub const RUNTIME_FILE_NAME: &str = "runtime.json";
pub const READY_FILE_NAME: &str = "ready";
pub const LOG_FILE_NAME: &str = "packet28d.log";
pub const WATCH_REGISTRY_FILE_NAME: &str = "watch-registry-v1.json";
pub const TASK_REGISTRY_FILE_NAME: &str = "task-registry-v1.json";
pub const TASK_EVENTS_DIR_NAME: &str = "tasks";
pub const TASK_EVENT_LOG_SUFFIX: &str = ".events.jsonl";
const JSON_FILE_SUFFIX: &str = ".json";
/// Maximum byte length of a portable task storage identifier.
///
/// The bound leaves room for [`TASK_EVENT_LOG_SUFFIX`] within the 255-byte
/// component limit shared by supported Linux and Apple filesystems.
pub const MAX_TASK_STORAGE_ID_BYTES: usize = 255 - TASK_EVENT_LOG_SUFFIX.len();
/// Maximum byte length of a portable context-version storage identifier.
///
/// The bound leaves room for the `.json` suffix within a 255-byte component.
pub const MAX_CONTEXT_VERSION_STORAGE_ID_BYTES: usize = 255 - JSON_FILE_SUFFIX.len();
pub const TASK_ARTIFACTS_DIR_NAME: &str = "task";
pub const TASK_BRIEF_MARKDOWN_FILE_NAME: &str = "brief.md";
pub const TASK_BRIEF_JSON_FILE_NAME: &str = "brief.json";
pub const TASK_STATE_JSON_FILE_NAME: &str = "state.json";
pub const HOOK_RUNTIME_CONFIG_FILE_NAME: &str = "hook-runtime-v1.json";
pub const AGENT_ACTIVE_TASK_FILE_NAME: &str = "active-task.json";
pub const INDEX_DIR_NAME: &str = ".packet28/index";
pub const INDEX_MANIFEST_FILE_NAME: &str = "manifest.json";
pub const INDEX_SNAPSHOT_FILE_NAME: &str = "repo-index-v1.bin";
const SOCKET_DIR_NAME: &str = "packet28d-sockets";

/// Error returned when an identifier cannot safely name persistent storage.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum StorageIdentifierError {
    /// Empty identifiers cannot name a task or context version.
    #[error("storage identifier must not be empty")]
    Empty,
    /// The encoded identifier would exceed the portable component budget.
    #[error("storage identifier is {observed} bytes; maximum supported size is {max} bytes")]
    TooLong {
        /// Encoded byte length of the rejected identifier.
        observed: usize,
        /// Maximum accepted encoded byte length.
        max: usize,
    },
    /// Only the injective lowercase portable ASCII domain is accepted.
    #[error(
        "storage identifier contains byte 0x{byte:02x} at offset {offset}; \
         expected only lowercase ASCII letters, digits, '-' or '_'"
    )]
    InvalidByte {
        /// Zero-based byte offset of the rejected byte.
        offset: usize,
        /// Rejected byte.
        byte: u8,
    },
    /// DOS device names are ambiguous path components on Windows.
    #[error("storage identifier {identifier:?} is a reserved DOS device name")]
    ReservedDosDeviceName {
        /// Rejected identifier.
        identifier: String,
    },
}

macro_rules! define_storage_identifier {
    ($name:ident, $description:literal, $max_bytes:expr) => {
        #[doc = $description]
        ///
        /// Values are non-empty lowercase portable ASCII (`[a-z0-9_-]+`), fit
        /// Packet28's portable path-component budget, and are not DOS device
        /// names. Constructing this type validates those invariants once so
        /// storage path helpers cannot receive an unsafe or lossy identifier.
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Returns the validated identifier.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = StorageIdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                validate_storage_identifier(value, $max_bytes)?;
                Ok(Self(value.to_owned()))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = StorageIdentifierError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                value.parse()
            }
        }

        impl TryFrom<String> for $name {
            type Error = StorageIdentifierError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                validate_storage_identifier(&value, $max_bytes)?;
                Ok(Self(value))
            }
        }
    };
}

define_storage_identifier!(
    TaskStorageId,
    "Validated identifier used for task artifact and event-log storage.",
    MAX_TASK_STORAGE_ID_BYTES
);
define_storage_identifier!(
    ContextVersionStorageId,
    "Validated identifier used as a context-version artifact filename stem.",
    MAX_CONTEXT_VERSION_STORAGE_ID_BYTES
);

fn validate_storage_identifier(
    value: &str,
    max_bytes: usize,
) -> Result<(), StorageIdentifierError> {
    if value.is_empty() {
        return Err(StorageIdentifierError::Empty);
    }
    if value.len() > max_bytes {
        return Err(StorageIdentifierError::TooLong {
            observed: value.len(),
            max: max_bytes,
        });
    }
    if let Some((offset, byte)) = value.bytes().enumerate().find(|(_, byte)| {
        !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
    }) {
        return Err(StorageIdentifierError::InvalidByte { offset, byte });
    }
    if is_reserved_dos_device_name(value) {
        return Err(StorageIdentifierError::ReservedDosDeviceName {
            identifier: value.to_owned(),
        });
    }
    Ok(())
}

fn is_reserved_dos_device_name(value: &str) -> bool {
    matches!(value, "con" | "prn" | "aux" | "nul")
        || value
            .strip_prefix("com")
            .or_else(|| value.strip_prefix("lpt"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

pub fn daemon_dir(root: &Path) -> PathBuf {
    root.join(DAEMON_DIR_NAME)
}

fn socket_dir() -> PathBuf {
    std::env::temp_dir().join(SOCKET_DIR_NAME)
}

fn socket_file_name(root: &Path) -> String {
    let digest = blake3::hash(root.to_string_lossy().as_bytes()).to_hex();
    format!("p28-{}.sock", &digest[..16])
}

pub fn index_dir(root: &Path) -> PathBuf {
    root.join(INDEX_DIR_NAME)
}

pub fn index_manifest_path(root: &Path) -> PathBuf {
    index_dir(root).join(INDEX_MANIFEST_FILE_NAME)
}

pub fn index_snapshot_path(root: &Path) -> PathBuf {
    index_dir(root).join(INDEX_SNAPSHOT_FILE_NAME)
}

pub fn socket_path(root: &Path) -> PathBuf {
    socket_dir().join(socket_file_name(root))
}

pub fn workspace_socket_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(SOCKET_FILE_NAME)
}

pub fn pid_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(PID_FILE_NAME)
}

pub fn runtime_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(RUNTIME_FILE_NAME)
}

pub fn ready_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(READY_FILE_NAME)
}

pub fn log_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(LOG_FILE_NAME)
}

pub fn watch_registry_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(WATCH_REGISTRY_FILE_NAME)
}

pub fn task_registry_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(TASK_REGISTRY_FILE_NAME)
}

pub fn task_events_dir(root: &Path) -> PathBuf {
    daemon_dir(root).join(TASK_EVENTS_DIR_NAME)
}

pub fn task_artifacts_dir(root: &Path) -> PathBuf {
    root.join(".packet28").join(TASK_ARTIFACTS_DIR_NAME)
}

pub fn agent_runtime_dir(root: &Path) -> PathBuf {
    root.join(".packet28").join("agent")
}

pub fn hook_runtime_config_path(root: &Path) -> PathBuf {
    daemon_dir(root).join(HOOK_RUNTIME_CONFIG_FILE_NAME)
}

pub fn active_task_path(root: &Path) -> PathBuf {
    agent_runtime_dir(root).join(AGENT_ACTIVE_TASK_FILE_NAME)
}

pub fn task_event_log_path(root: &Path, task_id: &TaskStorageId) -> PathBuf {
    task_events_dir(root).join(format!("{task_id}{TASK_EVENT_LOG_SUFFIX}"))
}

pub fn task_artifact_dir(root: &Path, task_id: &TaskStorageId) -> PathBuf {
    task_artifacts_dir(root).join(task_id.as_str())
}

pub fn task_brief_markdown_path(root: &Path, task_id: &TaskStorageId) -> PathBuf {
    task_artifact_dir(root, task_id).join(TASK_BRIEF_MARKDOWN_FILE_NAME)
}

pub fn task_brief_json_path(root: &Path, task_id: &TaskStorageId) -> PathBuf {
    task_artifact_dir(root, task_id).join(TASK_BRIEF_JSON_FILE_NAME)
}

pub fn task_state_json_path(root: &Path, task_id: &TaskStorageId) -> PathBuf {
    task_artifact_dir(root, task_id).join(TASK_STATE_JSON_FILE_NAME)
}

pub fn task_versions_dir(root: &Path, task_id: &TaskStorageId) -> PathBuf {
    task_artifact_dir(root, task_id).join("versions")
}

pub fn task_version_json_path(
    root: &Path,
    task_id: &TaskStorageId,
    context_version: &ContextVersionStorageId,
) -> PathBuf {
    task_versions_dir(root, task_id).join(format!("{context_version}{JSON_FILE_SUFFIX}"))
}

/// Resolves the nearest ancestor Git workspace, falling back to the input path.
pub fn resolve_workspace_root(start: &Path) -> PathBuf {
    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            return start.to_path_buf();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn socket_path_uses_short_hashed_temp_location() {
        let dir = tempdir().unwrap();
        let root = dir
            .path()
            .join("very")
            .join("long")
            .join("nested")
            .join("workspace")
            .join("path");
        let socket = socket_path(&root);

        assert!(socket.starts_with(std::env::temp_dir()));
        assert_eq!(
            socket.extension().and_then(|ext| ext.to_str()),
            Some("sock")
        );
        assert!(socket.to_string_lossy().len() < 104);
        assert_ne!(socket, daemon_dir(&root).join(SOCKET_FILE_NAME));
    }

    #[test]
    fn workspace_socket_path_uses_daemon_dir() {
        let dir = tempdir().unwrap();
        let socket = workspace_socket_path(dir.path());

        assert_eq!(socket, daemon_dir(dir.path()).join(SOCKET_FILE_NAME));
    }

    #[test]
    fn task_storage_identifiers_accept_the_exact_component_budget() {
        let value = "a".repeat(MAX_TASK_STORAGE_ID_BYTES);
        let task_id = TaskStorageId::try_from(value.as_str()).unwrap();

        assert_eq!(task_id.as_str(), value);
        assert_eq!(
            task_event_log_path(Path::new("/workspace"), &task_id)
                .file_name()
                .unwrap()
                .len(),
            255
        );
    }

    #[test]
    fn task_storage_identifiers_reject_one_byte_over_component_budget() {
        let value = "a".repeat(MAX_TASK_STORAGE_ID_BYTES + 1);
        let expected = StorageIdentifierError::TooLong {
            observed: MAX_TASK_STORAGE_ID_BYTES + 1,
            max: MAX_TASK_STORAGE_ID_BYTES,
        };

        assert_eq!(TaskStorageId::try_from(value.as_str()), Err(expected));
    }

    #[test]
    fn context_version_identifiers_accept_the_exact_component_budget() {
        let value = "a".repeat(MAX_CONTEXT_VERSION_STORAGE_ID_BYTES);
        let task_id = TaskStorageId::try_from("task").unwrap();
        let context_version = ContextVersionStorageId::try_from(value.as_str()).unwrap();

        assert_eq!(context_version.as_str(), value);
        assert_eq!(
            task_version_json_path(Path::new("/workspace"), &task_id, &context_version)
                .file_name()
                .unwrap()
                .len(),
            255
        );
    }

    #[test]
    fn context_version_identifiers_reject_one_byte_over_component_budget() {
        let value = "a".repeat(MAX_CONTEXT_VERSION_STORAGE_ID_BYTES + 1);
        let expected = StorageIdentifierError::TooLong {
            observed: MAX_CONTEXT_VERSION_STORAGE_ID_BYTES + 1,
            max: MAX_CONTEXT_VERSION_STORAGE_ID_BYTES,
        };

        assert_eq!(
            ContextVersionStorageId::try_from(value.as_str()),
            Err(expected)
        );
    }

    #[test]
    fn portable_storage_identifiers_reject_empty_or_nonportable_values() {
        for value in [
            "", "Task", "CON", "Com1", "task.id", "task:id", "task/id", r"task\id", "task id",
            " task", "task ", "task\tid", "task\nid", "λ",
        ] {
            assert!(
                TaskStorageId::try_from(value).is_err(),
                "task identifier {value:?} should be rejected"
            );
            assert!(
                ContextVersionStorageId::try_from(value).is_err(),
                "context version {value:?} should be rejected"
            );
        }
    }

    #[test]
    fn portable_storage_identifiers_reject_all_reserved_dos_device_names() {
        for value in [
            "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
            "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
        ] {
            assert!(matches!(
                TaskStorageId::try_from(value),
                Err(StorageIdentifierError::ReservedDosDeviceName { .. })
            ));
            assert!(matches!(
                ContextVersionStorageId::try_from(value),
                Err(StorageIdentifierError::ReservedDosDeviceName { .. })
            ));
        }
    }

    #[test]
    fn task_storage_paths_preserve_validated_identifiers_without_loss() {
        let root = Path::new("/workspace");
        let task_id = TaskStorageId::try_from("task-1").unwrap();
        let context_version = ContextVersionStorageId::try_from("ctx_2").unwrap();

        assert_eq!(
            task_event_log_path(root, &task_id),
            root.join(".packet28/daemon/tasks/task-1.events.jsonl")
        );
        assert_eq!(
            task_artifact_dir(root, &task_id),
            root.join(".packet28/task/task-1")
        );
        assert_eq!(
            task_brief_markdown_path(root, &task_id),
            root.join(".packet28/task/task-1/brief.md")
        );
        assert_eq!(
            task_brief_json_path(root, &task_id),
            root.join(".packet28/task/task-1/brief.json")
        );
        assert_eq!(
            task_state_json_path(root, &task_id),
            root.join(".packet28/task/task-1/state.json")
        );
        assert_eq!(
            task_versions_dir(root, &task_id),
            root.join(".packet28/task/task-1/versions")
        );
        assert_eq!(
            task_version_json_path(root, &task_id, &context_version),
            root.join(".packet28/task/task-1/versions/ctx_2.json")
        );
    }

    #[test]
    fn task_storage_path_mapping_is_injective_over_the_valid_domain_sample() {
        let root = Path::new("/workspace");
        let mut by_path = std::collections::BTreeMap::new();
        let alphabet = ['a', 'b', '0', '1', '-', '_'];

        for first in alphabet {
            for second in alphabet {
                for third in alphabet {
                    let value = format!("{first}{second}{third}");
                    let task_id = TaskStorageId::try_from(value.as_str()).unwrap();
                    let previous = by_path.insert(task_artifact_dir(root, &task_id), value.clone());
                    assert_eq!(previous, None, "path collision for {value:?}");
                }
            }
        }
    }
}
