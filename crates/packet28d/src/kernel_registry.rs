use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use context_kernel_core::{Kernel, PersistConfig};

/// A bounded, daemon-owned set of persistent kernels keyed by canonical root.
///
/// Kernel persistence is single-owner state. Sharing one instance per root
/// prevents concurrent requests from loading stale checkpoints and publishing
/// competing WAL/checkpoint histories.
pub(crate) struct PersistentKernelRegistry {
    capacity: usize,
    kernels: Mutex<BTreeMap<PathBuf, Arc<Kernel>>>,
}

#[derive(Debug)]
pub(crate) enum PersistentKernelRegistryError {
    InvalidRoot { root: PathBuf, source: io::Error },
    InvalidCapacity,
    CapacityExceeded { root: PathBuf, capacity: usize },
    Poisoned,
}

impl fmt::Display for PersistentKernelRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot { root, source } => {
                write!(
                    formatter,
                    "failed to canonicalize persistent kernel root '{}': {source}",
                    root.display()
                )
            }
            Self::InvalidCapacity => {
                formatter.write_str("persistent kernel root capacity must be greater than zero")
            }
            Self::CapacityExceeded { root, capacity } => write!(
                formatter,
                "persistent kernel root capacity {capacity} reached; refusing root '{}'",
                root.display()
            ),
            Self::Poisoned => formatter.write_str("persistent kernel registry lock poisoned"),
        }
    }
}

impl Error for PersistentKernelRegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidRoot { source, .. } => Some(source),
            Self::InvalidCapacity | Self::CapacityExceeded { .. } | Self::Poisoned => None,
        }
    }
}

impl PersistentKernelRegistry {
    pub(crate) fn new(
        primary_root: &Path,
        primary: Arc<Kernel>,
        capacity: usize,
    ) -> Result<Self, PersistentKernelRegistryError> {
        if capacity == 0 {
            return Err(PersistentKernelRegistryError::InvalidCapacity);
        }
        let primary_root = canonical_root(primary_root)?;
        let mut kernels = BTreeMap::new();
        kernels.insert(primary_root, primary);
        Ok(Self {
            capacity,
            kernels: Mutex::new(kernels),
        })
    }

    pub(crate) fn get(&self, root: &Path) -> Result<Arc<Kernel>, PersistentKernelRegistryError> {
        let root = canonical_root(root)?;
        let mut kernels = self
            .kernels
            .lock()
            .map_err(|_| PersistentKernelRegistryError::Poisoned)?;
        if let Some(kernel) = kernels.get(&root) {
            return Ok(kernel.clone());
        }
        if kernels.len() >= self.capacity {
            return Err(PersistentKernelRegistryError::CapacityExceeded {
                root,
                capacity: self.capacity,
            });
        }
        let kernel = Arc::new(Kernel::with_v1_reducers_and_persistence(
            PersistConfig::new(root.clone()),
        ));
        kernels.insert(root, kernel.clone());
        Ok(kernel)
    }

    pub(crate) fn kernels(&self) -> Result<Vec<Arc<Kernel>>, PersistentKernelRegistryError> {
        self.kernels
            .lock()
            .map(|kernels| kernels.values().cloned().collect())
            .map_err(|_| PersistentKernelRegistryError::Poisoned)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.kernels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf, PersistentKernelRegistryError> {
    let absolute =
        lexical_absolute(root).map_err(|source| PersistentKernelRegistryError::InvalidRoot {
            root: root.to_path_buf(),
            source,
        })?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(PersistentKernelRegistryError::InvalidRoot {
                        root: root.to_path_buf(),
                        source,
                    });
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(PersistentKernelRegistryError::InvalidRoot {
                        root: root.to_path_buf(),
                        source,
                    });
                };
                existing = parent;
            }
            Err(source) => {
                return Err(PersistentKernelRegistryError::InvalidRoot {
                    root: root.to_path_buf(),
                    source,
                });
            }
        }
    }
}

fn lexical_absolute(root: &Path) -> io::Result<PathBuf> {
    let mut normalized = if root.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir()?
    };
    for component in root.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Barrier};
    use std::thread;

    use context_kernel_core::KernelRequest;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn canonical_primary_root_reuses_the_existing_kernel() {
        let primary_root = tempdir().unwrap();
        let primary = Arc::new(Kernel::with_v1_reducers_and_persistence(
            PersistConfig::new(primary_root.path().to_path_buf()),
        ));
        let registry =
            PersistentKernelRegistry::new(primary_root.path(), primary.clone(), 2).unwrap();

        let resolved = registry.get(&primary_root.path().join(".")).unwrap();

        assert!(Arc::ptr_eq(&primary, &resolved));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn concurrent_cross_root_requests_share_one_owner_and_read_the_latest_write() {
        let primary_root = tempdir().unwrap();
        let cross_root = tempdir().unwrap();
        let primary = Arc::new(Kernel::with_v1_reducers_and_persistence(
            PersistConfig::new(primary_root.path().to_path_buf()),
        ));
        let registry =
            Arc::new(PersistentKernelRegistry::new(primary_root.path(), primary, 2).unwrap());
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let registry = registry.clone();
            let barrier = barrier.clone();
            let root = cross_root.path().to_path_buf();
            workers.push(thread::spawn(move || {
                barrier.wait();
                registry.get(&root).unwrap()
            }));
        }
        let kernels = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(
            kernels
                .iter()
                .all(|kernel| Arc::ptr_eq(kernel, &kernels[0])),
            "concurrent cross-root requests created competing persistence owners"
        );
        assert_eq!(registry.len(), 2);

        let writer = kernels[0].clone();
        let reader = kernels[1].clone();
        let (written_tx, written_rx) = mpsc::channel();
        let write = thread::spawn(move || {
            writer
                .execute(KernelRequest {
                    target: "agenty.state.write".to_string(),
                    reducer_input: json!({
                        "task_id": "task-shared-kernel",
                        "event_id": "event-1",
                        "occurred_at_unix": 1,
                        "actor": "agent",
                        "kind": "focus_set",
                        "paths": ["src/shared.rs"],
                        "data": {"type": "focus_set"}
                    }),
                    ..KernelRequest::default()
                })
                .unwrap();
            written_tx.send(()).unwrap();
        });
        let read = thread::spawn(move || {
            written_rx.recv().unwrap();
            reader
                .execute(KernelRequest {
                    target: "agenty.state.snapshot".to_string(),
                    reducer_input: json!({"task_id": "task-shared-kernel"}),
                    policy_context: json!({"disable_cache": true}),
                    ..KernelRequest::default()
                })
                .unwrap()
        });
        write.join().unwrap();
        let response = read.join().unwrap();
        let packet = response.output_packets.first().unwrap();
        let snapshot: suite_packet_core::EnvelopeV1<suite_packet_core::AgentSnapshotPayload> =
            serde_json::from_value(packet.body.clone()).unwrap();
        assert_eq!(snapshot.payload.event_count, 1);
        assert_eq!(snapshot.payload.focus_paths, vec!["src/shared.rs"]);
    }

    #[test]
    fn distinct_roots_fail_with_a_typed_error_at_capacity() {
        let primary_root = tempdir().unwrap();
        let second_root = tempdir().unwrap();
        let rejected_root = tempdir().unwrap();
        let primary = Arc::new(Kernel::with_v1_reducers_and_persistence(
            PersistConfig::new(primary_root.path().to_path_buf()),
        ));
        let registry = PersistentKernelRegistry::new(primary_root.path(), primary, 2).unwrap();
        registry.get(second_root.path()).unwrap();

        let error = match registry.get(rejected_root.path()) {
            Ok(_) => panic!("registry unexpectedly exceeded its configured root capacity"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            PersistentKernelRegistryError::CapacityExceeded { capacity: 2, .. }
        ));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn zero_capacity_is_rejected_without_constructing_a_registry() {
        let primary_root = tempdir().unwrap();
        let primary = Arc::new(Kernel::with_v1_reducers_and_persistence(
            PersistConfig::new(primary_root.path().to_path_buf()),
        ));

        let error = match PersistentKernelRegistry::new(primary_root.path(), primary, 0) {
            Ok(_) => panic!("zero-capacity registry unexpectedly constructed"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            PersistentKernelRegistryError::InvalidCapacity
        ));
    }

    #[test]
    fn missing_root_aliases_share_the_same_future_owner_key() {
        let primary_root = tempdir().unwrap();
        let primary = Arc::new(Kernel::with_v1_reducers_and_persistence(
            PersistConfig::new(primary_root.path().to_path_buf()),
        ));
        let registry = PersistentKernelRegistry::new(primary_root.path(), primary, 3).unwrap();
        let future_parent = primary_root.path().join("future");
        let first = registry.get(&future_parent.join("nested")).unwrap();
        let second = registry
            .get(&primary_root.path().join("future/./other/../nested"))
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 2);
    }
}
