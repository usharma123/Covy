use super::*;

const TASK_CANCELLATION_QUIESCE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct WatchIngress {
    sender: tokio::sync::mpsc::Sender<WatchEventMsg>,
    overflowed: Arc<Mutex<HashMap<(String, TaskGenerationId), WatchEventMsg>>>,
    overflow_ready: Arc<tokio::sync::Notify>,
}

impl WatchIngress {
    pub(crate) fn new(capacity: usize) -> (Self, tokio::sync::mpsc::Receiver<WatchEventMsg>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        (
            Self {
                sender,
                overflowed: Arc::new(Mutex::new(HashMap::new())),
                overflow_ready: Arc::new(tokio::sync::Notify::new()),
            },
            receiver,
        )
    }

    pub(crate) fn send(&self, message: WatchEventMsg) {
        match self.sender.try_send(message) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(message)) => {
                let key = (message.watch_id.clone(), message.generation);
                let mut overflowed = self
                    .overflowed
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let pending = overflowed.entry(key).or_insert_with(|| WatchEventMsg {
                    watch_id: message.watch_id.clone(),
                    generation: message.generation,
                    paths: Vec::new(),
                    error: None,
                    overflowed: true,
                });
                pending.overflowed = true;
                if pending.error.is_none() {
                    pending.error = message.error;
                }
                for path in message.paths {
                    if !pending.paths.contains(&path) {
                        pending.paths.push(path);
                    }
                }
                drop(overflowed);
                self.overflow_ready.notify_one();
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    fn drain_overflowed(&self) -> Vec<WatchEventMsg> {
        let mut overflowed = self
            .overflowed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        overflowed.drain().map(|(_, message)| message).collect()
    }

    #[cfg(test)]
    fn overflow_len(&self) -> usize {
        self.overflowed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

pub(crate) fn register_task_and_watches(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    spec: TaskSubmitSpec,
) -> Result<(TaskRecord, Vec<WatchRegistration>)> {
    let root = {
        let guard = state.lock().map_err(lock_err)?;
        guard.root.clone()
    };
    let spec = normalize_task_submit_spec(&root, spec)?;

    let replaces_existing_task = {
        let guard = state.lock().map_err(lock_err)?;
        guard.tasks.tasks.contains_key(&spec.task_id)
    };
    if replaces_existing_task {
        let _ = cancel_task(state.clone(), &spec.task_id)?;
    }

    let mut registrations = Vec::new();
    {
        let mut guard = state.lock().map_err(lock_err)?;
        if guard.tasks.tasks.contains_key(&spec.task_id) {
            anyhow::bail!(
                "task '{}' was concurrently replaced during registration",
                spec.task_id
            );
        }
        guard.task_generations.create(&spec.task_id)?;
        let watch_ids = spec
            .watches
            .iter()
            .map(|watch: &WatchSpec| {
                let mut watch = watch.clone();
                watch.task_id = spec.task_id.clone();
                if watch.root.trim().is_empty() {
                    watch.root = guard.root.to_string_lossy().to_string();
                }
                let registration = WatchRegistration {
                    watch_id: watch_id_for(&watch),
                    spec: watch,
                    active: true,
                    last_event_at_unix: None,
                    last_error: None,
                };
                guard.watches.watches.push(registration.clone());
                registrations.push(registration.clone());
                registration.watch_id
            })
            .collect::<Vec<_>>();
        let task = TaskRecord {
            task_id: spec.task_id.clone(),
            watch_ids,
            sequence_present: true,
            sequence: Some(spec.sequence.clone()),
            ..TaskRecord::default()
        };
        guard.tasks.tasks.insert(spec.task_id.clone(), task.clone());
    }

    let mut installed_watch_ids: Vec<String> = Vec::new();
    for registration in &registrations {
        if let Err(err) = install_watch(
            state.clone(),
            watch_tx.clone(),
            registration.watch_id.clone(),
        ) {
            let _ = remove_watch(state.clone(), &registration.watch_id);
            for watch_id in &installed_watch_ids {
                let _ = remove_watch(state.clone(), watch_id);
            }
            let mut guard = state.lock().map_err(lock_err)?;
            guard.tasks.tasks.remove(&spec.task_id);
            if let Some(generation) = guard.task_generations.current(&spec.task_id) {
                guard
                    .task_generations
                    .remove_if_current(&spec.task_id, generation.id());
            }
            guard.watches.watches.retain(|watch| {
                !registrations
                    .iter()
                    .any(|candidate| candidate.watch_id == watch.watch_id)
            });
            persist_state(&guard)?;
            return Err(err);
        }
        installed_watch_ids.push(registration.watch_id.clone());
    }

    {
        let guard = state.lock().map_err(lock_err)?;
        persist_state(&guard)?;
    }

    let task = state
        .lock()
        .map_err(lock_err)?
        .tasks
        .tasks
        .get(&spec.task_id)
        .cloned()
        .ok_or_else(|| anyhow!("task disappeared after registration"))?;
    Ok((task, registrations))
}

pub(crate) fn run_sequence_for_task(
    state: Arc<Mutex<DaemonState>>,
    task_id: &str,
) -> Result<context_kernel_core::KernelSequenceResponse> {
    loop {
        let (kernel, sequence, generation, _sequence_lease) = {
            let mut guard = state.lock().map_err(lock_err)?;
            if !guard.tasks.tasks.contains_key(task_id) {
                anyhow::bail!("unknown task '{task_id}'");
            }
            let generation = guard.task_generations.ensure(task_id)?;
            let sequence_lease = generation.acquire_operation().ok_or_else(|| {
                context_kernel_core::KernelError::SequenceCancelled {
                    task_id: Some(task_id.to_string()),
                }
            })?;
            let task = guard
                .tasks
                .tasks
                .get_mut(task_id)
                .expect("task existence checked before generation acquisition");
            let sequence = task
                .sequence
                .clone()
                .ok_or_else(|| anyhow!("task '{}' has no stored sequence", task_id))?;
            task.lifecycle.start()?;
            task.last_started_at_unix = Some(now_unix());
            task.last_error = None;
            persist_state(&guard)?;
            (guard.kernel.clone(), sequence, generation, sequence_lease)
        };
        let _ = emit_task_event_for_generation(
            state.clone(),
            task_id,
            generation.id(),
            "task_started",
            json!({"task_id": task_id, "step_count": sequence.steps.len()}),
        );

        let mut observer = TaskSequenceObserver {
            state: state.clone(),
            task_id: task_id.to_string(),
            generation: generation.clone(),
        };
        let result = kernel.execute_sequence_with_observer(sequence, &mut observer);

        let rerun = {
            let mut guard = state.lock().map_err(lock_err)?;
            if !guard.task_generations.matches(task_id, generation.id())
                || generation.is_cancelled()
            {
                return Err(context_kernel_core::KernelError::SequenceCancelled {
                    task_id: Some(task_id.to_string()),
                }
                .into());
            }
            let task = guard
                .tasks
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| anyhow!("unknown task '{task_id}'"))?;
            task.last_completed_at_unix = Some(now_unix());
            match &result {
                Ok(response) => {
                    task.last_request_id = Some(response.request_id);
                    task.last_sequence_metadata = Some(response.metadata.clone());
                    task.last_error = None;
                }
                Err(err) => {
                    task.last_error = Some(err.to_string());
                    daemon_log(&format!("task run failed task_id={} error={err}", task_id));
                }
            }
            let rerun = task.lifecycle.finish_run()?;
            if rerun {
                task.last_replan_at_unix = Some(now_unix());
            }
            persist_state(&guard)?;
            rerun
        };

        if result.is_ok() && !generation.is_cancelled() {
            let mut summary = refresh_task_context_summary_for_generation(
                state.clone(),
                task_id,
                generation.id(),
            )?
            .unwrap_or_else(|| json!({}));
            if generation.is_cancelled() {
                return Err(context_kernel_core::KernelError::SequenceCancelled {
                    task_id: Some(task_id.to_string()),
                }
                .into());
            }
            let _ = set_context_reason_for_generation(
                &state,
                task_id,
                generation.id(),
                "replan_applied",
            )?;
            if !generation.is_cancelled() {
                if let Some(response) = refresh_broker_context_for_task(&state, task_id, None)? {
                    if let Some(object) = summary.as_object_mut() {
                        object.insert(
                            "changed_section_ids".to_string(),
                            Value::Array(
                                response
                                    .delta
                                    .changed_sections
                                    .iter()
                                    .map(|section| Value::String(section.id.clone()))
                                    .collect(),
                            ),
                        );
                        object.insert(
                            "removed_section_ids".to_string(),
                            Value::Array(
                                response
                                    .delta
                                    .removed_section_ids
                                    .iter()
                                    .map(|id| Value::String(id.clone()))
                                    .collect(),
                            ),
                        );
                        object.insert(
                            "reason".to_string(),
                            Value::String("replan_applied".to_string()),
                        );
                        object.insert(
                            "context_version".to_string(),
                            Value::String(response.context_version.clone()),
                        );
                        object.insert(
                            "brief_path".to_string(),
                            Value::String(
                                task_brief_markdown_path(
                                    &state.lock().map_err(lock_err)?.root.clone(),
                                    task_id,
                                )
                                .to_string_lossy()
                                .to_string(),
                            ),
                        );
                    }
                }
            }
            let _ = emit_task_event_for_generation(
                state.clone(),
                task_id,
                generation.id(),
                "context_updated",
                summary,
            )?;
        }

        match result {
            Ok(_) if rerun => {
                if generation.is_cancelled() {
                    return Err(context_kernel_core::KernelError::SequenceCancelled {
                        task_id: Some(task_id.to_string()),
                    }
                    .into());
                }
                continue;
            }
            Ok(response) => {
                let emitted = emit_task_event_for_generation(
                    state.clone(),
                    task_id,
                    generation.id(),
                    "task_completed",
                    json!({"task_id": task_id, "request_id": response.request_id}),
                )?;
                if !emitted {
                    return Err(context_kernel_core::KernelError::SequenceCancelled {
                        task_id: Some(task_id.to_string()),
                    }
                    .into());
                }
                return Ok(response);
            }
            Err(err) => {
                let _ = emit_task_event_for_generation(
                    state.clone(),
                    task_id,
                    generation.id(),
                    "task_failed",
                    json!({"task_id": task_id, "error": err.to_string()}),
                )?;
                return Err(err.into());
            }
        }
    }
}

pub(crate) fn cancel_task(
    state: Arc<Mutex<DaemonState>>,
    task_id: &str,
) -> Result<(Option<TaskRecord>, Vec<String>)> {
    let (generation, watch_ids) = {
        let mut guard = state.lock().map_err(lock_err)?;
        if !guard.tasks.tasks.contains_key(task_id) {
            return Ok((None, Vec::new()));
        }
        let generation = guard.task_generations.ensure(task_id)?;
        generation.request_cancel();
        let task = guard
            .tasks
            .tasks
            .get_mut(task_id)
            .expect("task existence checked before generation cancellation");
        task.lifecycle.request_cancel();
        let watch_ids = task.watch_ids.clone();
        persist_state(&guard)?;
        (generation, watch_ids)
    };
    for watch_id in &watch_ids {
        let _ = remove_watch(state.clone(), watch_id)?;
    }
    crate::launch::terminate_generation_processes(&generation)?;
    if !generation.wait_until_idle(TASK_CANCELLATION_QUIESCE_TIMEOUT) {
        anyhow::bail!(
            "timed out waiting for cancelled task '{}' generation to become idle",
            task_id
        );
    }
    let mut guard = state.lock().map_err(lock_err)?;
    if !guard.task_generations.matches(task_id, generation.id()) {
        return Ok((None, watch_ids));
    }
    let removed = guard.tasks.tasks.remove(task_id);
    guard.subscribers.remove(task_id);
    guard
        .task_generations
        .remove_if_current(task_id, generation.id());
    persist_state(&guard)?;
    Ok((removed, watch_ids))
}

pub(crate) fn remove_watch(
    state: Arc<Mutex<DaemonState>>,
    watch_id: &str,
) -> Result<Option<WatchRegistration>> {
    let mut guard = state.lock().map_err(lock_err)?;
    guard.watcher_handles.remove(watch_id);
    let removed = guard
        .watches
        .watches
        .iter()
        .position(|watch| watch.watch_id == watch_id)
        .map(|index| guard.watches.watches.remove(index));
    for task in guard.tasks.tasks.values_mut() {
        task.watch_ids.retain(|candidate| candidate != watch_id);
    }
    persist_state(&guard)?;
    Ok(removed)
}

pub(crate) fn restore_watchers(
    state: &Arc<Mutex<DaemonState>>,
    watch_tx: &WatchIngress,
) -> Result<()> {
    let watch_ids = state
        .lock()
        .map_err(lock_err)?
        .watches
        .watches
        .iter()
        .map(|watch| watch.watch_id.clone())
        .collect::<Vec<_>>();
    for watch_id in watch_ids {
        if let Err(err) = install_watch(state.clone(), watch_tx.clone(), watch_id.clone()) {
            daemon_log(&format!("failed to restore watch {watch_id}: {err}"));
        }
    }
    Ok(())
}

pub(crate) fn install_watch(
    state: Arc<Mutex<DaemonState>>,
    watch_tx: WatchIngress,
    watch_id: String,
) -> Result<()> {
    let (spec, generation) = {
        let mut guard = state.lock().map_err(lock_err)?;
        let spec = guard
            .watches
            .watches
            .iter()
            .find(|watch| watch.watch_id == watch_id)
            .map(|watch| watch.spec.clone())
            .ok_or_else(|| anyhow!("unknown watch '{watch_id}'"))?;
        if !guard.tasks.tasks.contains_key(&spec.task_id) {
            anyhow::bail!(
                "watch '{}' belongs to unknown task '{}'",
                watch_id,
                spec.task_id
            );
        }
        let generation = guard.task_generations.ensure(&spec.task_id)?.id();
        (spec, generation)
    };

    let callback_watch_id = watch_id.clone();
    let mut watcher = PollWatcher::new(
        move |result: notify::Result<Event>| match result {
            Ok(event) => {
                watch_tx.send(WatchEventMsg {
                    watch_id: callback_watch_id.clone(),
                    generation,
                    paths: event.paths,
                    error: None,
                    overflowed: false,
                });
            }
            Err(err) => {
                watch_tx.send(WatchEventMsg {
                    watch_id: callback_watch_id.clone(),
                    generation,
                    paths: Vec::new(),
                    error: Some(err.to_string()),
                    overflowed: false,
                });
            }
        },
        Config::default()
            .with_poll_interval(Duration::from_millis(spec.debounce_ms.unwrap_or(250))),
    )?;

    let paths = watch_paths(&spec);
    for path in &paths {
        let mode = if matches!(spec.kind, WatchKind::Git | WatchKind::File) {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher.watch(path, mode)?;
    }

    let mut guard = state.lock().map_err(lock_err)?;
    if let Some(watch) = guard
        .watches
        .watches
        .iter_mut()
        .find(|watch| watch.watch_id == watch_id)
    {
        watch.active = true;
        watch.last_error = None;
    }
    guard.watcher_handles.insert(watch_id.clone(), watcher);
    persist_state(&guard)?;
    daemon_log(&format!(
        "installed watch watch_id={watch_id} task_id={} kind={:?}",
        spec.task_id, spec.kind
    ));
    Ok(())
}

pub(crate) async fn run_watch_processor(
    state: Arc<Mutex<DaemonState>>,
    ingress: WatchIngress,
    mut watch_rx: tokio::sync::mpsc::Receiver<WatchEventMsg>,
    blocking_pool: crate::runtime::BlockingPool,
) {
    let mut shutdown = match state.lock().map_err(lock_err) {
        Ok(guard) => guard.shutdown.subscribe(),
        Err(error) => {
            daemon_log(&format!(
                "watch processor could not subscribe to shutdown: {error}"
            ));
            return;
        }
    };
    let mut pending = HashMap::<(String, TaskGenerationId), PendingWatchEvent>::new();
    let mut workers = tokio::task::JoinSet::new();
    loop {
        for message in ingress.drain_overflowed() {
            merge_watch_event(state.clone(), &mut pending, message);
        }
        for message in take_due_watch_events(&mut pending) {
            let worker_state = state.clone();
            let worker_pool = blocking_pool.clone();
            workers.spawn(async move {
                worker_pool
                    .run(move || process_watch_event(worker_state, message))
                    .await
            });
        }

        let next_deadline = next_watch_deadline(&pending);
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            message = watch_rx.recv() => {
                let Some(message) = message else {
                    break;
                };
                merge_watch_event(state.clone(), &mut pending, message);
            }
            () = ingress.overflow_ready.notified() => {}
            () = sleep_until_optional(next_deadline), if next_deadline.is_some() => {}
            joined = workers.join_next(), if !workers.is_empty() => {
                match joined {
                    Some(Err(error)) => {
                        daemon_log(&format!("watch worker failed to join: {error}"));
                    }
                    Some(Ok(Err(error))) => {
                        daemon_log(&format!("watch event processing failed: {error}"));
                    }
                    Some(Ok(Ok(()))) | None => {}
                }
            }
        }
    }

    workers.abort_all();
    while workers.join_next().await.is_some() {}
}

fn process_watch_event(state: Arc<Mutex<DaemonState>>, message: WatchEventMsg) -> Result<()> {
    let (task_id, error_message, generation, _activity_lease) = {
        let guard = state.lock().map_err(lock_err)?;
        let Some(registration) = guard
            .watches
            .watches
            .iter()
            .find(|watch| watch.watch_id == message.watch_id)
        else {
            return Ok(());
        };
        let task_id = registration.spec.task_id.clone();
        let Some(generation) = guard.task_generations.current(&task_id) else {
            return Ok(());
        };
        if generation.id() != message.generation || generation.is_cancelled() {
            return Ok(());
        }
        let Some(activity_lease) = generation.acquire_operation() else {
            return Ok(());
        };
        (task_id, message.error.clone(), generation, activity_lease)
    };

    {
        let mut guard = state.lock().map_err(lock_err)?;
        if !guard.task_generations.matches(&task_id, generation.id()) || generation.is_cancelled() {
            return Ok(());
        }
        if let Some(watch) = guard
            .watches
            .watches
            .iter_mut()
            .find(|watch| watch.watch_id == message.watch_id)
        {
            watch.last_event_at_unix = Some(now_unix());
            watch.last_error = error_message.clone();
        }
        persist_state(&guard)?;
    }

    if let Some(error) = error_message {
        let _ = emit_task_event_for_generation(
            state.clone(),
            &task_id,
            generation.id(),
            "watch_error",
            json!({
                "watch_id": message.watch_id,
                "error": error,
            }),
        );
        return Ok(());
    }

    let _ = emit_task_event_for_generation(
        state.clone(),
        &task_id,
        generation.id(),
        "watch_triggered",
        json!({
            "watch_id": message.watch_id,
            "queue_overflowed": message.overflowed,
            "paths": message
                .paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
        }),
    );

    let _ =
        set_context_reason_for_generation(&state, &task_id, generation.id(), "watch_triggered")?;
    let _ = refresh_task_context_summary_for_generation(state.clone(), &task_id, generation.id())?;
    if generation.is_cancelled() {
        return Ok(());
    }
    let _ = refresh_broker_context_for_task(&state, &task_id, None)?;

    let should_start = {
        let mut guard = state.lock().map_err(lock_err)?;
        if !guard.task_generations.matches(&task_id, generation.id()) || generation.is_cancelled() {
            return Ok(());
        }
        let should_start = guard
            .tasks
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow!("unknown task '{task_id}'"))?
            .lifecycle
            .request_replan()?;
        persist_state(&guard)?;
        should_start
    };

    if should_start {
        let _ = run_sequence_for_task(state, &task_id);
    }
    Ok(())
}

pub(crate) fn watch_paths(spec: &WatchSpec) -> Vec<PathBuf> {
    match spec.kind {
        WatchKind::Git => vec![PathBuf::from(&spec.root).join(".git")],
        WatchKind::File => spec
            .paths
            .iter()
            .map(|path| PathBuf::from(&spec.root).join(path))
            .collect(),
        WatchKind::TestReport => spec
            .paths
            .iter()
            .map(|path| PathBuf::from(&spec.root).join(path))
            .collect(),
    }
}

pub(crate) fn watch_id_for(spec: &WatchSpec) -> String {
    let digest = blake3::hash(
        serde_json::to_string(spec)
            .unwrap_or_else(|_| format!("{:?}", spec))
            .as_bytes(),
    );
    format!("watch-{}", &digest.to_hex()[..16])
}

fn normalize_task_submit_spec(root: &Path, mut spec: TaskSubmitSpec) -> Result<TaskSubmitSpec> {
    if spec.task_id.trim().is_empty() {
        anyhow::bail!("task_id cannot be empty");
    }
    spec.sequence.reactive.enabled = true;
    spec.sequence.reactive.task_id = Some(spec.task_id.clone());
    if spec.sequence.steps.is_empty() {
        anyhow::bail!("sequence must contain at least one step");
    }
    spec.sequence = normalize_sequence_request(spec.sequence).map_err(|source| anyhow!(source))?;

    for watch in &mut spec.watches {
        watch.task_id = spec.task_id.clone();
        if watch.root.trim().is_empty() {
            watch.root = root.to_string_lossy().to_string();
        }
        let watch_root = resolve_root(Path::new(&watch.root));
        if !watch_root.exists() {
            anyhow::bail!("watch root '{}' does not exist", watch_root.display());
        }
        for path in watch_paths(watch) {
            if !path.exists() {
                anyhow::bail!("watch path '{}' does not exist", path.display());
            }
        }
    }

    Ok(spec)
}

fn merge_watch_event(
    state: Arc<Mutex<DaemonState>>,
    pending: &mut HashMap<(String, TaskGenerationId), PendingWatchEvent>,
    message: WatchEventMsg,
) {
    let debounce_ms = watch_debounce_ms(&state, &message.watch_id).unwrap_or(250);
    merge_watch_event_with_debounce(pending, message, debounce_ms);
}

fn merge_watch_event_with_debounce(
    pending: &mut HashMap<(String, TaskGenerationId), PendingWatchEvent>,
    message: WatchEventMsg,
    debounce_ms: u64,
) {
    let due_at = tokio::time::Instant::now() + Duration::from_millis(debounce_ms);
    let key = (message.watch_id.clone(), message.generation);
    let entry = pending.entry(key).or_insert_with(|| PendingWatchEvent {
        watch_id: message.watch_id.clone(),
        generation: message.generation,
        paths: Vec::new(),
        error: None,
        overflowed: false,
        due_at,
    });
    entry.due_at = due_at;
    entry.overflowed |= message.overflowed;
    if entry.error.is_none() {
        entry.error = message.error.clone();
    }
    for path in message.paths {
        if !entry.paths.iter().any(|existing| existing == &path) {
            entry.paths.push(path);
        }
    }
}

fn take_due_watch_events(
    pending: &mut HashMap<(String, TaskGenerationId), PendingWatchEvent>,
) -> Vec<WatchEventMsg> {
    let now = tokio::time::Instant::now();
    let ready_ids = pending
        .iter()
        .filter(|(_, item)| item.due_at <= now)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    ready_ids
        .into_iter()
        .filter_map(|key| {
            pending.remove(&key).map(|item| WatchEventMsg {
                watch_id: item.watch_id,
                generation: item.generation,
                paths: item.paths,
                error: item.error,
                overflowed: item.overflowed,
            })
        })
        .collect()
}

fn next_watch_deadline(
    pending: &HashMap<(String, TaskGenerationId), PendingWatchEvent>,
) -> Option<tokio::time::Instant> {
    pending.values().map(|item| item.due_at).min()
}

async fn sleep_until_optional(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
}

fn watch_debounce_ms(state: &Arc<Mutex<DaemonState>>, watch_id: &str) -> Option<u64> {
    state.lock().ok().and_then(|guard| {
        guard
            .watches
            .watches
            .iter()
            .find(|watch| watch.watch_id == watch_id)
            .and_then(|watch| watch.spec.debounce_ms)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation() -> TaskGenerationId {
        TaskGenerationRegistry::default()
            .create("task-watch")
            .unwrap()
            .id()
    }

    fn message(generation: TaskGenerationId, path: &str) -> WatchEventMsg {
        WatchEventMsg {
            watch_id: "watch-1".to_string(),
            generation,
            paths: vec![PathBuf::from(path)],
            error: None,
            overflowed: false,
        }
    }

    #[test]
    fn bounded_watch_ingress_coalesces_overflow_by_generation() {
        let generation = generation();
        let (ingress, mut receiver) = WatchIngress::new(1);
        ingress.send(message(generation, "src/one.rs"));
        ingress.send(message(generation, "src/two.rs"));
        ingress.send(message(generation, "src/three.rs"));

        assert_eq!(ingress.overflow_len(), 1);
        let queued = receiver.try_recv().unwrap();
        assert_eq!(queued.paths, vec![PathBuf::from("src/one.rs")]);
        let overflowed = ingress.drain_overflowed();
        assert_eq!(overflowed.len(), 1);
        assert!(overflowed[0].overflowed);
        assert_eq!(
            overflowed[0].paths,
            vec![PathBuf::from("src/two.rs"), PathBuf::from("src/three.rs")]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn debounce_deadline_resets_and_fires_once() {
        let generation = generation();
        let mut pending = HashMap::new();
        merge_watch_event_with_debounce(&mut pending, message(generation, "src/one.rs"), 250);
        tokio::time::advance(Duration::from_millis(200)).await;
        merge_watch_event_with_debounce(&mut pending, message(generation, "src/two.rs"), 250);
        tokio::time::advance(Duration::from_millis(249)).await;
        assert!(take_due_watch_events(&mut pending).is_empty());

        tokio::time::advance(Duration::from_millis(1)).await;
        let due = take_due_watch_events(&mut pending);
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].paths,
            vec![PathBuf::from("src/one.rs"), PathBuf::from("src/two.rs")]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_pending_debounce_work() {
        let state = crate::tests::support::daemon_test_state();
        let generation = {
            let mut guard = state.lock().unwrap();
            guard.tasks.tasks.insert(
                "task-watch".to_string(),
                TaskRecord {
                    task_id: "task-watch".to_string(),
                    ..TaskRecord::default()
                },
            );
            let generation = guard.task_generations.create("task-watch").unwrap().id();
            guard.watches.watches.push(WatchRegistration {
                watch_id: "watch-1".to_string(),
                spec: WatchSpec {
                    task_id: "task-watch".to_string(),
                    debounce_ms: Some(1_000),
                    ..WatchSpec::default()
                },
                active: true,
                ..WatchRegistration::default()
            });
            generation
        };
        let root = state.lock().unwrap().root.clone();
        let (ingress, receiver) = WatchIngress::new(1);
        let processor = tokio::spawn(run_watch_processor(
            state.clone(),
            ingress.clone(),
            receiver,
            crate::runtime::BlockingPool::new(1),
        ));
        ingress.send(message(generation, "src/one.rs"));
        tokio::task::yield_now().await;
        state.lock().unwrap().shutdown.request();

        processor.await.unwrap();
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(load_task_events(&root, "task-watch").unwrap().is_empty());
    }
}
