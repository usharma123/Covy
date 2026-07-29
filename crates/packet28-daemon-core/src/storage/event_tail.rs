use super::*;

/// Maximum suffix read while reconciling the authoritative event sequence.
///
/// The window holds the last two maximum-size complete frames plus one
/// maximum-size crash-partial frame.
pub const MAX_TASK_EVENT_TAIL_SCAN_BYTES: usize = 3 * (MAX_TASK_EVENT_LINE_BYTES + 1);

/// Loads the strict durable task registry and every authenticated event tail
/// under one shared registry authority lock.
///
/// This is the daemon-startup reconciliation primitive. On Unix targets, the
/// registry is decoded once, then each admitted task's bounded event suffix is
/// inspected while the same registry binding remains locked. That avoids
/// decoding an O(tasks)-sized registry once per task and prevents a writer
/// from changing admission between the registry snapshot and its tail
/// observations. The portable fallback preserves the same validation
/// contract through the existing per-task tail API.
///
/// # Errors
///
/// Returns the same typed registry, identifier, event-tail, authority-limit,
/// and filesystem errors as [`load_task_registry`] and
/// [`task_event_log_tail_sequence`].
pub fn load_task_registry_with_event_tails(
    root: &Path,
) -> Result<(TaskRegistry, BTreeMap<String, Option<u64>>)> {
    #[cfg(unix)]
    {
        let registry_path = task_registry_path(root);
        let writer_lease = acquire_task_store_writer_lease(root)?;
        with_anchored_task_registry_lock(
            root,
            RegistryLockMode::Shared,
            || Ok(()),
            |daemon| {
                let registry = match daemon
                    .read_file_limited(OsStr::new(TASK_REGISTRY_FILE_NAME), MAX_TASK_REGISTRY_BYTES)
                {
                    Ok(raw) => decode_task_registry(&registry_path, &raw)?,
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                        TaskRegistry::default()
                    }
                    Err(source) => {
                        return Err(task_registry_read_error(daemon, &registry_path, source));
                    }
                };
                let mut tails = BTreeMap::new();
                for task_id in registry.tasks.keys() {
                    let storage_id = checked_task_storage_id(root, task_id)?;
                    let tail =
                        task_event_log_tail_sequence_admitted(root, &storage_id, &writer_lease)?;
                    tails.insert(task_id.clone(), tail);
                }
                Ok((registry, tails))
            },
        )
    }
    #[cfg(not(unix))]
    {
        let registry = load_task_registry(root)?;
        let mut tails = BTreeMap::new();
        for task_id in registry.tasks.keys() {
            tails.insert(
                task_id.clone(),
                task_event_log_tail_sequence(root, task_id)?,
            );
        }
        Ok((registry, tails))
    }
}

/// Returns the authenticated sequence of the last complete durable event.
///
/// Only a bounded suffix containing the last two complete frames and an
/// optional crash-partial frame is read. A malformed complete frame,
/// cross-task frame, zero sequence, or non-contiguous final pair is rejected
/// instead of skipped. A trailing non-newline suffix is ignored by this
/// read-only reconciliation operation and will be truncated by
/// [`append_next_task_event`] while holding the same exclusive event lock.
///
/// # Errors
///
/// Returns [`DaemonCoreError::InvalidTaskRegistry`] when the task is not
/// durably admitted. Returns [`DaemonCoreError::InvalidTaskEventFrame`] for an
/// invalid authoritative tail, [`DaemonCoreError::AuthorityJsonLimitExceeded`]
/// for a structurally excessive frame, or [`DaemonCoreError::Io`] for bounded
/// suffix, lock, descriptor, or namespace failures.
pub fn task_event_log_tail_sequence(root: &Path, task_id: &str) -> Result<Option<u64>> {
    let task_id = checked_task_storage_id(root, task_id)?;
    let writer_lease = acquire_task_store_writer_lease(root)?;
    with_registered_task_storage_id(root, &task_id, || {
        #[cfg(unix)]
        {
            task_event_log_tail_sequence_admitted(root, &task_id, &writer_lease)
        }
        #[cfg(not(unix))]
        {
            let _ = writer_lease;
            task_event_log_tail_sequence_portable(root, &task_id)
        }
    })
}

/// Allocates and appends the next durable task-event sequence under one
/// exclusive event-file lock.
///
/// The event log is the sequence owner. The last two complete frames are
/// validated, `tail + 1` is assigned with overflow checking, and the returned
/// frame is synchronized before the lock is released. A bounded trailing
/// crash-partial suffix is truncated and synchronized under that same lock
/// before the new complete frame is appended.
///
/// # Errors
///
/// Returns [`DaemonCoreError::InvalidTaskRegistry`] when the task is not
/// admitted, [`DaemonCoreError::InvalidTaskEventFrame`] for malformed,
/// conflicting, cross-task, or exhausted sequence authority,
/// [`DaemonCoreError::AuthorityJsonLimitExceeded`] for an excessive event, or
/// [`DaemonCoreError::StorageMutationAuthorityLost`] when synchronized bytes
/// cannot be reauthenticated before return.
pub fn append_next_task_event(
    root: &Path,
    task_id: &str,
    event: &DaemonEvent,
) -> Result<DaemonEventFrame> {
    let task_id = checked_task_storage_id(root, task_id)?;
    // Use the widest possible decimal sequence during preflight so successful
    // admission guarantees that the final encoded line fits the same bound.
    let preflight = DaemonEventFrame {
        seq: u64::MAX,
        task_id: task_id.as_str().to_string(),
        event: event.clone(),
    };
    let _ = encode_task_event_frame(root, &task_id, &preflight)?;

    let writer_lease = acquire_task_store_writer_lease(root)?;
    with_registered_task_storage_id(root, &task_id, || {
        #[cfg(unix)]
        {
            append_next_task_event_admitted(root, &task_id, &writer_lease, event)
        }
        #[cfg(not(unix))]
        {
            let _ = writer_lease;
            append_next_task_event_portable(root, &task_id, event)
        }
    })
}

#[derive(Debug)]
struct TaskEventTailInspection {
    tail: Option<DaemonEventFrame>,
    complete_len: u64,
    has_partial_suffix: bool,
}

#[cfg(unix)]
fn task_event_log_tail_sequence_admitted(
    root: &Path,
    task_id: &TaskStorageId,
    lease: &crate::task_store_lease::TaskStoreLease,
) -> Result<Option<u64>> {
    task_event_log_tail_sequence_admitted_with_observer(root, task_id, lease, || Ok(()))
}

#[cfg(unix)]
pub(super) fn task_event_log_tail_sequence_admitted_with_observer(
    root: &Path,
    task_id: &TaskStorageId,
    lease: &crate::task_store_lease::TaskStoreLease,
    after_read: impl FnOnce() -> Result<()>,
) -> Result<Option<u64>> {
    let path = task_event_log_path(root, task_id);
    let Some(events) = open_task_events_capability_for_read(root)? else {
        return Ok(None);
    };
    let file_name = event_log_file_name(task_id);
    validate_anchored_event_namespace_aliases(&events, &file_name, &path)?;
    let file = match events.open_read_file(OsStr::new(&file_name)) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DaemonCoreError::io(
                "failed to open task event log tail",
                &path,
                source,
            ));
        }
    };
    let mut lock = AnchoredFileLock::lock_existing(
        &events,
        OsStr::new(&file_name),
        path.clone(),
        file,
        AnchoredFileLockMode::Shared,
    )
    .map_err(|source| {
        DaemonCoreError::io(
            "failed to acquire authenticated task event tail lock",
            &path,
            source,
        )
    })?;
    let result = lease
        .validate_namespace_attachment()
        .and_then(|()| {
            events.validate_display_path_attachment().map_err(|source| {
                DaemonCoreError::io("task event namespace is detached", &path, source)
            })
        })
        .and_then(|()| validate_anchored_event_namespace_aliases(&events, &file_name, &path))
        .and_then(|()| inspect_locked_task_event_tail(lock.file_mut(), &path, task_id))
        .and_then(|inspection| after_read().map(|()| inspection))
        .and_then(|inspection| {
            lease
                .validate_namespace_attachment()
                .and_then(|()| {
                    events.validate_display_path_attachment().map_err(|source| {
                        DaemonCoreError::io("task event namespace is detached", &path, source)
                    })
                })
                .and_then(|()| {
                    validate_anchored_event_namespace_aliases(&events, &file_name, &path)
                })
                .and_then(|()| {
                    lock.validate_attachment().map_err(|source| {
                        DaemonCoreError::io(
                            "task event tail binding changed while locked",
                            &path,
                            source,
                        )
                    })
                })
                .map(|()| inspection.tail.map(|frame| frame.seq))
        });
    let finish = lock.finish();
    match (result, finish) {
        (Ok(sequence), Ok(())) => Ok(sequence),
        (Ok(_), Err(AnchoredFileLockFinishError::Attachment(source))) => Err(DaemonCoreError::io(
            "task event tail lock changed before unlock",
            &path,
            source,
        )),
        (Ok(_), Err(AnchoredFileLockFinishError::Unlock(source))) => Err(DaemonCoreError::io(
            "failed to unlock task event tail",
            &path,
            source,
        )),
        (Err(error), _) => Err(error),
    }
}

#[cfg(unix)]
fn append_next_task_event_admitted(
    root: &Path,
    task_id: &TaskStorageId,
    lease: &crate::task_store_lease::TaskStoreLease,
    event: &DaemonEvent,
) -> Result<DaemonEventFrame> {
    append_next_task_event_admitted_with_observers(
        root,
        task_id,
        lease,
        event,
        || Ok(()),
        || Ok(()),
    )
}

#[cfg(unix)]
pub(super) fn append_next_task_event_admitted_with_observers(
    root: &Path,
    task_id: &TaskStorageId,
    lease: &crate::task_store_lease::TaskStoreLease,
    event: &DaemonEvent,
    after_tail_read: impl FnOnce() -> Result<()>,
    after_sync: impl FnOnce() -> Result<()>,
) -> Result<DaemonEventFrame> {
    let path = task_event_log_path(root, task_id);
    let file_name = event_log_file_name(task_id);
    let events = open_task_events_capability_for_write(root)?;
    validate_anchored_event_namespace_aliases(&events, &file_name, &path)?;
    let file = events
        .open_append_file(OsStr::new(&file_name))
        .map_err(|source| {
            DaemonCoreError::io(
                "failed to open task event log for sequence allocation",
                &path,
                source,
            )
        })?;
    validate_anchored_event_namespace_aliases(&events, &file_name, &path)?;
    let mut lock = AnchoredFileLock::lock_existing(
        &events,
        OsStr::new(&file_name),
        path.clone(),
        file,
        AnchoredFileLockMode::Exclusive,
    )
    .map_err(|source| {
        DaemonCoreError::io(
            "failed to acquire exclusive task event sequence lock",
            &path,
            source,
        )
    })?;
    let result = (|| -> Result<DaemonEventFrame> {
        lease.validate_namespace_attachment()?;
        events
            .validate_display_path_attachment()
            .map_err(|source| {
                DaemonCoreError::io("task event namespace is detached", &path, source)
            })?;
        validate_anchored_event_namespace_aliases(&events, &file_name, &path)?;
        lock.validate_attachment().map_err(|source| {
            DaemonCoreError::io(
                "task event sequence binding changed while locked",
                &path,
                source,
            )
        })?;

        let inspection = inspect_locked_task_event_tail(lock.file_mut(), &path, task_id)?;
        after_tail_read()?;
        let sequence =
            match inspection.tail {
                Some(ref frame) => frame.seq.checked_add(1).ok_or_else(|| {
                    DaemonCoreError::InvalidTaskEventFrame {
                        path: path.clone(),
                        message: "task event sequence is exhausted at u64::MAX".to_string(),
                    }
                })?,
                None => 1,
            };
        let frame = DaemonEventFrame {
            seq: sequence,
            task_id: task_id.as_str().to_string(),
            event: event.clone(),
        };
        let bytes = encode_task_event_frame(root, task_id, &frame)?;

        lease.validate_namespace_attachment()?;
        events
            .validate_display_path_attachment()
            .map_err(|source| {
                DaemonCoreError::io("task event namespace is detached", &path, source)
            })?;
        validate_anchored_event_namespace_aliases(&events, &file_name, &path)?;
        lock.validate_attachment().map_err(|source| {
            DaemonCoreError::io(
                "task event sequence binding changed before append",
                &path,
                source,
            )
        })?;
        if inspection.has_partial_suffix {
            lock.file_mut()
                .set_len(inspection.complete_len)
                .map_err(|source| {
                    DaemonCoreError::io(
                        "failed to truncate crash-partial task event suffix",
                        &path,
                        source,
                    )
                })?;
            sync_task_event_file(lock.file(), &path)?;
        }
        lock.file_mut().write_all(&bytes).map_err(|source| {
            DaemonCoreError::io("failed to append allocated task event", &path, source)
        })?;
        sync_task_event_file(lock.file(), &path)?;
        after_sync()?;

        let postcheck = lease
            .validate_namespace_attachment()
            .and_then(|()| {
                events.validate_display_path_attachment().map_err(|source| {
                    DaemonCoreError::io("task event namespace is detached", &path, source)
                })
            })
            .and_then(|()| validate_anchored_event_namespace_aliases(&events, &file_name, &path))
            .and_then(|()| {
                lock.validate_attachment().map_err(|source| {
                    DaemonCoreError::io(
                        "task event sequence binding changed after append",
                        &path,
                        source,
                    )
                })
            });
        if let Err(error) = postcheck {
            return Err(storage_mutation_authority_lost(
                "append next task event",
                &path,
                error,
            ));
        }
        Ok(frame)
    })();
    let finish = lock.finish();
    match (result, finish) {
        (Ok(frame), Ok(())) => Ok(frame),
        (Ok(_), Err(AnchoredFileLockFinishError::Attachment(source))) => {
            Err(DaemonCoreError::StorageMutationAuthorityLost {
                operation: "append next task event",
                path,
                source,
            })
        }
        (Ok(_), Err(AnchoredFileLockFinishError::Unlock(source))) => Err(DaemonCoreError::io(
            "failed to unlock allocated task event",
            &path,
            source,
        )),
        (Err(error), _) => Err(error),
    }
}

#[cfg(not(unix))]
fn task_event_log_tail_sequence_portable(
    root: &Path,
    task_id: &TaskStorageId,
) -> Result<Option<u64>> {
    let path = task_event_log_path(root, task_id);
    let Some(mut file) = open_task_event_file_for_read(root, task_id, &path)? else {
        return Ok(None);
    };
    FileExt::lock_shared(&file)
        .map_err(|source| DaemonCoreError::io("failed to lock task event tail", &path, source))?;
    let result = inspect_locked_task_event_tail(&mut file, &path, task_id)
        .map(|tail| tail.tail.map(|f| f.seq));
    let unlock = FileExt::unlock(&file)
        .map_err(|source| DaemonCoreError::io("failed to unlock task event tail", &path, source));
    match (result, unlock) {
        (Ok(sequence), Ok(())) => Ok(sequence),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg(not(unix))]
fn append_next_task_event_portable(
    root: &Path,
    task_id: &TaskStorageId,
    event: &DaemonEvent,
) -> Result<DaemonEventFrame> {
    let path = task_event_log_path(root, task_id);
    let directory = task_events_dir(root);
    let file_name = event_log_file_name(task_id);
    ensure_portable_real_directory(&directory)?;
    validate_portable_event_namespace_aliases(&directory, &file_name, &path)?;
    validate_portable_event_file_type(&path)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&path)
        .map_err(|source| {
            DaemonCoreError::io(
                "failed to open portable task event sequence log",
                &path,
                source,
            )
        })?;
    FileExt::lock_exclusive(&file).map_err(|source| {
        DaemonCoreError::io(
            "failed to lock portable task event sequence log",
            &path,
            source,
        )
    })?;
    let result = (|| -> Result<DaemonEventFrame> {
        validate_portable_event_namespace_aliases(&directory, &file_name, &path)?;
        validate_portable_event_file_type(&path)?;
        let inspection = inspect_locked_task_event_tail(&mut file, &path, task_id)?;
        let sequence =
            match inspection.tail {
                Some(ref frame) => frame.seq.checked_add(1).ok_or_else(|| {
                    DaemonCoreError::InvalidTaskEventFrame {
                        path: path.clone(),
                        message: "task event sequence is exhausted at u64::MAX".to_string(),
                    }
                })?,
                None => 1,
            };
        let frame = DaemonEventFrame {
            seq: sequence,
            task_id: task_id.as_str().to_string(),
            event: event.clone(),
        };
        let bytes = encode_task_event_frame(root, task_id, &frame)?;
        if inspection.has_partial_suffix {
            file.set_len(inspection.complete_len).map_err(|source| {
                DaemonCoreError::io(
                    "failed to truncate portable crash-partial event suffix",
                    &path,
                    source,
                )
            })?;
            sync_task_event_file(&file, &path)?;
        }
        file.write_all(&bytes).map_err(|source| {
            DaemonCoreError::io(
                "failed to append portable allocated task event",
                &path,
                source,
            )
        })?;
        sync_task_event_file(&file, &path)?;
        Ok(frame)
    })();
    let unlock = FileExt::unlock(&file).map_err(|source| {
        DaemonCoreError::io(
            "failed to unlock portable task event sequence log",
            &path,
            source,
        )
    });
    match (result, unlock) {
        (Ok(frame), Ok(())) => Ok(frame),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

fn inspect_locked_task_event_tail(
    file: &mut fs::File,
    path: &Path,
    task_id: &TaskStorageId,
) -> Result<TaskEventTailInspection> {
    let len = file
        .metadata()
        .map_err(|source| DaemonCoreError::io("failed to inspect task event tail", path, source))?
        .len();
    if len == 0 {
        return Ok(TaskEventTailInspection {
            tail: None,
            complete_len: 0,
            has_partial_suffix: false,
        });
    }
    let scan_bound = MAX_TASK_EVENT_TAIL_SCAN_BYTES as u64;
    let start = len.saturating_sub(scan_bound);
    file.seek(SeekFrom::Start(start))
        .map_err(|source| DaemonCoreError::io("failed to seek task event tail", path, source))?;
    let expected = usize::try_from(len - start).map_err(|_| {
        DaemonCoreError::io(
            "task event tail window does not fit memory",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bounded tail window does not fit usize",
            ),
        )
    })?;
    let mut suffix = Vec::new();
    suffix.try_reserve_exact(expected).map_err(|source| {
        DaemonCoreError::io(
            "failed to reserve task event tail window",
            path,
            std::io::Error::new(std::io::ErrorKind::OutOfMemory, source),
        )
    })?;
    file.take(scan_bound + 1)
        .read_to_end(&mut suffix)
        .map_err(|source| DaemonCoreError::io("failed to read task event tail", path, source))?;
    if suffix.len() != expected
        || file
            .metadata()
            .map_err(|source| {
                DaemonCoreError::io("failed to re-inspect task event tail", path, source)
            })?
            .len()
            != len
    {
        return Err(DaemonCoreError::io(
            "task event log changed during locked tail read",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "locked event length changed during bounded suffix read",
            ),
        ));
    }
    let (complete_end, partial_len) = if suffix.ends_with(b"\n") {
        (suffix.len(), 0)
    } else if let Some(last_newline) = suffix.iter().rposition(|byte| *byte == b'\n') {
        (last_newline + 1, suffix.len() - last_newline - 1)
    } else {
        if start != 0 || suffix.len() > MAX_TASK_EVENT_LINE_BYTES {
            return Err(task_event_limit_error(
                path,
                "crash-partial tail bytes",
                len - start,
                MAX_TASK_EVENT_LINE_BYTES as u64,
            ));
        }
        return Ok(TaskEventTailInspection {
            tail: None,
            complete_len: 0,
            has_partial_suffix: true,
        });
    };
    if partial_len > MAX_TASK_EVENT_LINE_BYTES {
        return Err(task_event_limit_error(
            path,
            "crash-partial tail bytes",
            partial_len as u64,
            MAX_TASK_EVENT_LINE_BYTES as u64,
        ));
    }
    let terminal_newline = complete_end - 1;
    let last_start = suffix[..terminal_newline]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    if start != 0 && last_start == 0 {
        return Err(task_event_limit_error(
            path,
            "authoritative tail frame bytes",
            MAX_TASK_EVENT_LINE_BYTES as u64 + 1,
            MAX_TASK_EVENT_LINE_BYTES as u64,
        ));
    }
    let tail =
        decode_authoritative_tail_frame(path, task_id, &suffix[last_start..terminal_newline])?;
    let (previous, previous_starts_file) = if last_start == 0 {
        (None, false)
    } else {
        let previous_terminal = last_start - 1;
        let previous_start = suffix[..previous_terminal]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        if start != 0 && previous_start == 0 {
            return Err(task_event_limit_error(
                path,
                "previous authoritative tail frame bytes",
                MAX_TASK_EVENT_LINE_BYTES as u64 + 1,
                MAX_TASK_EVENT_LINE_BYTES as u64,
            ));
        }
        (
            Some(decode_authoritative_tail_frame(
                path,
                task_id,
                &suffix[previous_start..previous_terminal],
            )?),
            start == 0 && previous_start == 0,
        )
    };
    if let Some(previous) = previous {
        if previous_starts_file && previous.seq != 1 {
            return Err(DaemonCoreError::InvalidTaskEventFrame {
                path: path.to_path_buf(),
                message: format!(
                    "first authoritative task event sequence must be 1, found {}",
                    previous.seq
                ),
            });
        }
        let expected =
            previous
                .seq
                .checked_add(1)
                .ok_or_else(|| DaemonCoreError::InvalidTaskEventFrame {
                    path: path.to_path_buf(),
                    message: "previous task event tail sequence is u64::MAX".to_string(),
                })?;
        if tail.seq != expected {
            return Err(DaemonCoreError::InvalidTaskEventFrame {
                path: path.to_path_buf(),
                message: format!(
                    "authoritative event tail is not contiguous: previous {}, last {}",
                    previous.seq, tail.seq
                ),
            });
        }
    } else if start == 0 && tail.seq != 1 {
        return Err(DaemonCoreError::InvalidTaskEventFrame {
            path: path.to_path_buf(),
            message: format!(
                "first authoritative task event sequence must be 1, found {}",
                tail.seq
            ),
        });
    }
    Ok(TaskEventTailInspection {
        tail: Some(tail),
        complete_len: start + complete_end as u64,
        has_partial_suffix: partial_len != 0,
    })
}

fn decode_authoritative_tail_frame(
    path: &Path,
    task_id: &TaskStorageId,
    encoded: &[u8],
) -> Result<DaemonEventFrame> {
    let encoded = encoded.strip_suffix(b"\r").unwrap_or(encoded);
    if encoded.len() > MAX_TASK_EVENT_LINE_BYTES {
        return Err(task_event_limit_error(
            path,
            "authoritative tail frame bytes",
            encoded.len() as u64,
            MAX_TASK_EVENT_LINE_BYTES as u64,
        ));
    }
    validate_authority_json(encoded, AuthorityJsonProfile::TaskEventFrame).map_err(|error| {
        match error {
            AuthorityJsonError::Json(source) => DaemonCoreError::InvalidTaskEventFrame {
                path: path.to_path_buf(),
                message: format!("malformed authoritative task event JSON: {source}"),
            },
            error @ AuthorityJsonError::Limit { .. } => map_authority_json_error(
                path,
                AuthorityJsonProfile::TaskEventFrame,
                "failed to validate authoritative task event tail from",
                error,
            ),
        }
    })?;
    let frame = serde_json::from_slice::<DaemonEventFrame>(encoded).map_err(|source| {
        DaemonCoreError::InvalidTaskEventFrame {
            path: path.to_path_buf(),
            message: format!("failed to decode authoritative task event frame: {source}"),
        }
    })?;
    if frame.task_id != task_id.as_str() {
        return Err(DaemonCoreError::InvalidTaskEventFrame {
            path: path.to_path_buf(),
            message: format!(
                "authoritative tail task {:?} does not match expected task {:?}",
                frame.task_id,
                task_id.as_str()
            ),
        });
    }
    if frame.seq == 0 {
        return Err(DaemonCoreError::InvalidTaskEventFrame {
            path: path.to_path_buf(),
            message: "authoritative task event sequence must be greater than zero".to_string(),
        });
    }
    Ok(frame)
}

fn storage_mutation_authority_lost(
    operation: &'static str,
    path: &Path,
    error: DaemonCoreError,
) -> DaemonCoreError {
    let source = match error {
        DaemonCoreError::Io { source, .. } => source,
        other => std::io::Error::other(other),
    };
    DaemonCoreError::StorageMutationAuthorityLost {
        operation,
        path: path.to_path_buf(),
        source,
    }
}
