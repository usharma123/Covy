use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use context_kernel_core::{Kernel, KernelError, PersistConfig};

/// A bounded, daemon-owned set of persistent kernels keyed by canonical root.
///
/// Kernel persistence is single-owner state. Sharing one instance per root
/// prevents concurrent requests from loading stale checkpoints and publishing
/// competing WAL/checkpoint histories.
pub(crate) struct PersistentKernelRegistry {
    capacity: usize,
    state: Mutex<PersistentKernelRegistryState>,
    changed: Condvar,
}

struct PersistentKernelRegistryState {
    kernels: BTreeMap<PathBuf, Arc<Kernel>>,
    opening: BTreeSet<PathBuf>,
}

struct KernelOpening<'a> {
    registry: &'a PersistentKernelRegistry,
    root: PathBuf,
    active: bool,
}

#[derive(Debug)]
pub(crate) enum PersistentKernelRegistryError {
    InvalidRoot { root: PathBuf, source: io::Error },
    InvalidCapacity,
    CapacityExceeded { root: PathBuf, capacity: usize },
    Persistence { root: PathBuf, source: KernelError },
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
            Self::Persistence { root, source } => write!(
                formatter,
                "failed to open persistent kernel for '{}': {source}",
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
            Self::Persistence { source, .. } => Some(source),
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
            state: Mutex::new(PersistentKernelRegistryState {
                kernels,
                opening: BTreeSet::new(),
            }),
            changed: Condvar::new(),
        })
    }

    pub(crate) fn get(&self, root: &Path) -> Result<Arc<Kernel>, PersistentKernelRegistryError> {
        let root = canonical_root(root)?;
        let opening = loop {
            let mut state = self
                .state
                .lock()
                .map_err(|_| PersistentKernelRegistryError::Poisoned)?;
            if let Some(kernel) = state.kernels.get(&root) {
                return Ok(kernel.clone());
            }
            if state.opening.contains(&root) {
                drop(
                    self.changed
                        .wait(state)
                        .map_err(|_| PersistentKernelRegistryError::Poisoned)?,
                );
                continue;
            }
            if state.kernels.len().saturating_add(state.opening.len()) >= self.capacity {
                return Err(PersistentKernelRegistryError::CapacityExceeded {
                    root,
                    capacity: self.capacity,
                });
            }
            state.opening.insert(root.clone());
            break KernelOpening {
                registry: self,
                root: root.clone(),
                active: true,
            };
        };

        let kernel = Arc::new(
            Kernel::try_with_v1_reducers_and_persistence(PersistConfig::new(root.clone()))
                .map_err(|source| PersistentKernelRegistryError::Persistence {
                    root: root.clone(),
                    source,
                })?,
        );
        opening.publish(kernel)
    }

    pub(crate) fn kernels(&self) -> Result<Vec<Arc<Kernel>>, PersistentKernelRegistryError> {
        self.state
            .lock()
            .map(|state| state.kernels.values().cloned().collect())
            .map_err(|_| PersistentKernelRegistryError::Poisoned)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .kernels
            .len()
    }
}

impl KernelOpening<'_> {
    fn publish(
        mut self,
        kernel: Arc<Kernel>,
    ) -> Result<Arc<Kernel>, PersistentKernelRegistryError> {
        let mut state = self
            .registry
            .state
            .lock()
            .map_err(|_| PersistentKernelRegistryError::Poisoned)?;
        let removed = state.opening.remove(&self.root);
        debug_assert!(removed, "kernel opening reservation disappeared");
        state.kernels.insert(self.root.clone(), kernel.clone());
        self.active = false;
        drop(state);
        self.registry.changed.notify_all();
        Ok(kernel)
    }
}

impl Drop for KernelOpening<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.opening.remove(&self.root);
        drop(state);
        self.registry.changed.notify_all();
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
    fn unopenable_secondary_root_fails_without_consuming_capacity() {
        let primary_root = tempdir().unwrap();
        let secondary_root = tempdir().unwrap();
        let cache_dir = secondary_root.path().join(".packet28");
        std::fs::create_dir(&cache_dir).unwrap();
        std::fs::create_dir(cache_dir.join("packet-cache-v3.lock")).unwrap();
        let primary = Arc::new(
            Kernel::try_with_v1_reducers_and_persistence(PersistConfig::new(
                primary_root.path().to_path_buf(),
            ))
            .unwrap(),
        );
        let registry = PersistentKernelRegistry::new(primary_root.path(), primary, 2).unwrap();

        let error = match registry.get(secondary_root.path()) {
            Ok(_) => panic!("unopenable secondary kernel unexpectedly opened"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            PersistentKernelRegistryError::Persistence { .. }
        ));
        assert_eq!(registry.len(), 1);
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
