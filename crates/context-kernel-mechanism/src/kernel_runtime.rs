use super::*;

pub struct ExecutionContext {
    pub request_id: u64,
    pub target: String,
    pub budget: ExecutionBudget,
    pub policy_context: Value,
    pub reducer_input: Value,
    pub(crate) memory: Arc<Mutex<PacketCache>>,
    shared: Map<String, Value>,
}

impl ExecutionContext {
    pub fn set_shared(&mut self, key: impl Into<String>, value: Value) {
        self.shared.insert(key.into(), value);
    }

    pub fn shared_value(&self, key: &str) -> Option<&Value> {
        self.shared.get(key)
    }

    pub fn shared_json(&self) -> Value {
        Value::Object(self.shared.clone())
    }

    pub fn cache_entries(&self) -> Result<Vec<context_memory_core::PacketCacheEntry>, KernelError> {
        let cache = self
            .memory
            .lock()
            .map_err(|source| KernelError::CacheLock {
                detail: source.to_string(),
            })?;
        Ok(cache.entries())
    }

    pub fn cache_recall(
        &self,
        query: &str,
        options: &RecallOptions,
    ) -> Result<Vec<RecallHit>, KernelError> {
        let cache = self
            .memory
            .lock()
            .map_err(|source| KernelError::CacheLock {
                detail: source.to_string(),
            })?;
        Ok(cache.recall(query, options))
    }

    /// Returns cache entries related to the supplied task and packet
    /// references.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::CacheLock`] when the shared cache lock is
    /// poisoned.
    pub fn cache_related_entries(
        &self,
        task_id: Option<&str>,
        canonical_paths: &[String],
        symbols: &[String],
        tests: &[String],
    ) -> Result<Vec<RelatedEntryMatch>, KernelError> {
        let cache = self
            .memory
            .lock()
            .map_err(|source| KernelError::CacheLock {
                detail: source.to_string(),
            })?;
        Ok(cache.related_entries(task_id, canonical_paths, symbols, tests))
    }
}

type ReducerFn = dyn Fn(&mut ExecutionContext, &[KernelPacket]) -> Result<ReducerResult, KernelError>
    + Send
    + Sync;

enum PendingCacheUpdate {
    Volatile(VolatileCacheUpdate),
    Persistent(PreparedPersistentCacheMutation),
}

#[derive(Clone, Copy)]
enum CacheMutationTime {
    Current,
    #[cfg(test)]
    Fixed(u64),
}

struct VolatileCacheUpdate {
    target: String,
    lookup: context_memory_core::CacheLookup,
    packets: Vec<CachePacket>,
    metadata: Value,
}

pub struct KernelMechanism {
    reducers: HashMap<String, Arc<ReducerFn>>,
    next_request_id: AtomicU64,
    pub(crate) memory: Arc<Mutex<PacketCache>>,
    persistence: Option<CachePersistence>,
    persist_ttl_secs: Option<u64>,
    persistence_error: Mutex<Option<String>>,
    cache_mutation_lock_operations: AtomicU64,
    cache_mutation_lock_nanos: AtomicU64,
    services: KernelServices,
}

impl Default for KernelMechanism {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelMechanism {
    pub fn new() -> Self {
        Self::with_services(KernelServices::default())
    }

    pub fn with_services(services: KernelServices) -> Self {
        Self {
            reducers: HashMap::new(),
            next_request_id: AtomicU64::new(1),
            memory: Arc::new(Mutex::new(PacketCache::new())),
            persistence: None,
            persist_ttl_secs: None,
            persistence_error: Mutex::new(None),
            cache_mutation_lock_operations: AtomicU64::new(0),
            cache_mutation_lock_nanos: AtomicU64::new(0),
            services,
        }
    }

    pub fn with_persistence(config: PersistConfig) -> Self {
        Self::with_persistence_and_services(config, KernelServices::default())
    }

    /// Creates a persistent kernel or returns the persistence-owner startup
    /// failure without constructing an in-memory fallback.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::CachePersistence`] when the persistence owner
    /// cannot be opened.
    pub fn try_with_persistence(config: PersistConfig) -> Result<Self, KernelError> {
        Self::try_with_persistence_and_services(config, KernelServices::default())
    }

    pub fn with_persistence_and_services(config: PersistConfig, services: KernelServices) -> Self {
        let persist_ttl_secs = config.ttl_secs;
        let fallback_config = config.clone();
        let (memory, persistence, persistence_error) = match CachePersistence::open(config) {
            Ok(persistence) => (persistence.shared_cache(), Some(persistence), None),
            Err(error) => (
                Arc::new(Mutex::new(PacketCache::load_from_disk(&fallback_config))),
                None,
                Some(error.to_string()),
            ),
        };
        Self {
            reducers: HashMap::new(),
            next_request_id: AtomicU64::new(1),
            memory,
            persistence,
            persist_ttl_secs: Some(persist_ttl_secs),
            persistence_error: Mutex::new(persistence_error),
            cache_mutation_lock_operations: AtomicU64::new(0),
            cache_mutation_lock_nanos: AtomicU64::new(0),
            services,
        }
    }

    /// Creates a persistent kernel with custom services, failing if the
    /// persistence owner cannot be opened.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::CachePersistence`] when the persistence owner
    /// cannot be opened.
    pub fn try_with_persistence_and_services(
        config: PersistConfig,
        services: KernelServices,
    ) -> Result<Self, KernelError> {
        let persist_ttl_secs = config.ttl_secs;
        let persistence =
            CachePersistence::open(config).map_err(|source| KernelError::CachePersistence {
                detail: source.to_string(),
            })?;
        Ok(Self {
            reducers: HashMap::new(),
            next_request_id: AtomicU64::new(1),
            memory: persistence.shared_cache(),
            persistence: Some(persistence),
            persist_ttl_secs: Some(persist_ttl_secs),
            persistence_error: Mutex::new(None),
            cache_mutation_lock_operations: AtomicU64::new(0),
            cache_mutation_lock_nanos: AtomicU64::new(0),
            services,
        })
    }

    pub fn flush_cache_persistence(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, KernelError> {
        let Some(persistence) = self.persistence.as_ref() else {
            let detail = self
                .persistence_error
                .lock()
                .map_err(|source| KernelError::CacheLock {
                    detail: source.to_string(),
                })?
                .clone()
                .unwrap_or_else(|| "persistence is not configured".to_string());
            return Err(KernelError::CachePersistence { detail });
        };
        persistence
            .flush(timeout)
            .map_err(|source| KernelError::CachePersistence {
                detail: source.to_string(),
            })
    }

    /// Flushes, checkpoints, and joins the root persistence owner within a
    /// caller-supplied lifecycle bound.
    pub fn shutdown_cache_persistence(
        &self,
        timeout: Duration,
    ) -> Result<CachePersistenceMetrics, KernelError> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Err(self.persistence_unavailable_error()?);
        };
        persistence
            .shutdown(timeout)
            .map_err(|source| KernelError::CachePersistence {
                detail: source.to_string(),
            })
    }

    pub fn context_store_list(
        &self,
        filter: &ContextStoreListFilter,
        paging: &ContextStorePaging,
    ) -> Result<Vec<ContextStoreEntrySummary>, KernelError> {
        let cache = self.lock_memory()?;
        Ok(cache.list_entries(filter, paging))
    }

    pub fn context_store_get(
        &self,
        cache_key: &str,
    ) -> Result<Option<ContextStoreEntryDetail>, KernelError> {
        let cache = self.lock_memory()?;
        Ok(cache.get_entry(cache_key))
    }

    pub fn context_store_stats(&self) -> Result<ContextStoreStats, KernelError> {
        let cache = self.lock_memory()?;
        Ok(cache.stats())
    }

    pub fn context_store_recall(
        &self,
        query: &str,
        options: &RecallOptions,
    ) -> Result<Vec<RecallHit>, KernelError> {
        let cache = self.lock_memory()?;
        Ok(cache.recall(query, options))
    }

    /// Applies a prune to the live root cache and orders its tombstones with
    /// concurrent cache writes using the revision reserved under the same
    /// memory lock.
    pub fn context_store_prune(
        &self,
        request: ContextStorePruneRequest,
        timeout: Duration,
    ) -> Result<ContextStorePruneReport, KernelError> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Err(self.persistence_unavailable_error()?);
        };
        persistence
            .prune(request, timeout)
            .map_err(|source| self.map_cache_persistence_error(source))
    }

    #[cfg(test)]
    fn context_store_prune_at(
        &self,
        request: ContextStorePruneRequest,
        timeout: Duration,
        now_unix: u64,
    ) -> Result<ContextStorePruneReport, KernelError> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Err(self.persistence_unavailable_error()?);
        };
        persistence
            .prune_at(request, timeout, now_unix)
            .map_err(|source| self.map_cache_persistence_error(source))
    }
    fn lock_memory(&self) -> Result<std::sync::MutexGuard<'_, PacketCache>, KernelError> {
        self.memory.lock().map_err(|source| KernelError::CacheLock {
            detail: source.to_string(),
        })
    }

    fn persistence_unavailable_error(&self) -> Result<KernelError, KernelError> {
        let detail = self
            .persistence_error
            .lock()
            .map_err(|source| KernelError::CacheLock {
                detail: source.to_string(),
            })?
            .clone()
            .unwrap_or_else(|| "persistence is not configured".to_string());
        Ok(KernelError::CachePersistence { detail })
    }

    fn cache_persistence_error(&self, source: impl ToString) -> KernelError {
        let detail = source.to_string();
        if let Ok(mut last_error) = self.persistence_error.lock() {
            *last_error = Some(detail.clone());
        }
        KernelError::CachePersistence { detail }
    }

    fn map_cache_persistence_error(&self, source: CachePersistenceError) -> KernelError {
        match source {
            CachePersistenceError::CacheLock { detail } => KernelError::CacheLock { detail },
            source => self.cache_persistence_error(source),
        }
    }

    fn prepare_cache_update(
        &self,
        target: &str,
        lookup: &context_memory_core::CacheLookup,
        packets: Vec<CachePacket>,
        metadata: Value,
    ) -> PendingCacheUpdate {
        match self.persistence.as_ref() {
            Some(persistence) => PendingCacheUpdate::Persistent(persistence.prepare_update(
                PacketCache::prepare_mutation(target, lookup, packets, metadata),
            )),
            None => PendingCacheUpdate::Volatile(VolatileCacheUpdate {
                target: target.to_string(),
                lookup: lookup.clone(),
                packets,
                metadata,
            }),
        }
    }

    pub fn cache_runtime_metrics(&self) -> CacheRuntimeMetrics {
        let owner_error = self
            .persistence
            .as_ref()
            .and_then(CachePersistence::last_error)
            .map(|error| error.to_string());
        let persistence_error = self
            .persistence_error
            .lock()
            .map(|error| error.clone())
            .unwrap_or_else(|source| Some(source.to_string()))
            .or(owner_error);
        CacheRuntimeMetrics {
            mutation_lock_operations: self.cache_mutation_lock_operations.load(Ordering::Relaxed),
            mutation_lock_nanos: self.cache_mutation_lock_nanos.load(Ordering::Relaxed),
            persistence: self.persistence.as_ref().map(CachePersistence::metrics),
            persistence_error,
        }
    }

    pub fn register_reducer<F>(&mut self, target: impl Into<String>, reducer: F)
    where
        F: Fn(&mut ExecutionContext, &[KernelPacket]) -> Result<ReducerResult, KernelError>
            + Send
            + Sync
            + 'static,
    {
        self.reducers.insert(target.into(), Arc::new(reducer));
    }

    pub fn reducer_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.reducers.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn execute(&self, req: KernelRequest) -> Result<KernelResponse, KernelError> {
        let mut hooks = NoopDeltaReuseHooks;
        self.execute_with_hooks(req, &mut hooks)
    }

    pub fn execute_with_hooks(
        &self,
        req: KernelRequest,
        hooks: &mut dyn DeltaReuseHooks,
    ) -> Result<KernelResponse, KernelError> {
        let target = req.target.trim().to_string();
        if target.is_empty() {
            return Err(KernelError::EmptyTarget);
        }

        let reducer = self
            .reducers
            .get(&target)
            .ok_or_else(|| KernelError::UnknownTarget {
                target: target.clone(),
                registered: self.reducer_names(),
            })?;

        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let input_usage = usage_for_packets(&req.input_packets);
        let mut policy_run = self.services.execution_policy.begin(&target, &req)?;

        enforce_budget(&target, BudgetStage::Input, req.budget, input_usage)?;

        let cache_lookup = if policy_run.cache_enabled() {
            let cache_input = policy_run.cache_input();
            Some({
                let cache = self
                    .memory
                    .lock()
                    .map_err(|source| KernelError::CacheLock {
                        detail: source.to_string(),
                    })?;
                cache.lookup_with_hooks(&target, cache_input, hooks)
            })
        } else {
            None
        };

        if let Some(entry) = cache_lookup
            .as_ref()
            .and_then(|lookup| lookup.entry.clone())
        {
            let output_packets = entry
                .packets
                .into_iter()
                .map(|packet| KernelPacket {
                    packet_id: packet.packet_id,
                    format: default_packet_format(),
                    body: packet.body,
                    token_usage: packet.token_usage,
                    runtime_ms: packet.runtime_ms,
                    metadata: packet.metadata,
                })
                .collect::<Vec<_>>();
            let output_packet_count = output_packets.len();

            policy_run.audit_output(&output_packets)?;

            let output_usage = usage_for_packets(&output_packets);
            let total_usage = BudgetUsage {
                tokens: input_usage.tokens.saturating_add(output_usage.tokens),
                bytes: input_usage.bytes.saturating_add(output_usage.bytes),
                runtime_ms: input_usage
                    .runtime_ms
                    .saturating_add(output_usage.runtime_ms),
            };
            enforce_budget(&target, BudgetStage::Total, req.budget, total_usage)?;
            let entry_age_secs = now_unix().saturating_sub(entry.created_at_unix);

            return Ok(KernelResponse {
                request_id,
                target: target.clone(),
                output_packets,
                audit: KernelAudit {
                    reducer: target,
                    input_packets: req.input_packets.len(),
                    output_packets: output_packet_count,
                    budget: req.budget,
                    input_usage,
                    output_usage,
                    total_usage,
                    governance: policy_run.governance_audit(),
                },
                metadata: merge_json(
                    entry.metadata,
                    json!({
                        "cache": {
                            "hit": true,
                            "key": cache_lookup
                                .as_ref()
                                .map(|lookup| lookup.cache_key.clone())
                                .unwrap_or_default(),
                            "entry_age_secs": entry_age_secs,
                            "miss_reason": Value::Null,
                        }
                    }),
                ),
            });
        }

        let mut ctx = ExecutionContext {
            request_id,
            target: target.clone(),
            budget: req.budget,
            policy_context: req.policy_context.clone(),
            reducer_input: req.reducer_input,
            memory: self.memory.clone(),
            shared: Map::new(),
        };

        let started_at = Instant::now();
        let reducer_result = reducer(&mut ctx, &req.input_packets)?;
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        policy_run.audit_output(&reducer_result.output_packets)?;
        let output_packet_count = reducer_result.output_packets.len();

        let output_usage = usage_for_packets(&reducer_result.output_packets);
        let total_usage = BudgetUsage {
            tokens: input_usage.tokens.saturating_add(output_usage.tokens),
            bytes: input_usage.bytes.saturating_add(output_usage.bytes),
            runtime_ms: elapsed_ms,
        };

        enforce_budget(&target, BudgetStage::Total, req.budget, total_usage)?;

        let output_packets = reducer_result.output_packets;
        let cache_miss_reason = if cache_lookup.is_some() {
            "not_found"
        } else {
            "disabled"
        };
        let mut response = KernelResponse {
            request_id,
            target: target.clone(),
            output_packets: output_packets.clone(),
            audit: KernelAudit {
                reducer: target.clone(),
                input_packets: req.input_packets.len(),
                output_packets: output_packet_count,
                budget: req.budget,
                input_usage,
                output_usage,
                total_usage,
                governance: policy_run.governance_audit(),
            },
            metadata: merge_json(
                merge_json(ctx.shared_json(), reducer_result.metadata),
                json!({
                    "cache": {
                        "hit": false,
                        "key": cache_lookup
                            .as_ref()
                            .map(|lookup| lookup.cache_key.clone())
                            .unwrap_or_default(),
                        "entry_age_secs": Value::Null,
                        "miss_reason": cache_miss_reason,
                    }
                }),
            ),
        };

        if let Some(cache_lookup) = cache_lookup {
            let packets = output_packets
                .iter()
                .map(|packet| CachePacket {
                    packet_id: packet.packet_id.clone(),
                    body: packet.body.clone(),
                    token_usage: packet.token_usage,
                    runtime_ms: packet.runtime_ms,
                    metadata: packet.metadata.clone(),
                })
                .collect();

            let metadata = response.metadata.clone();
            let update = self.prepare_cache_update(&target, &cache_lookup, packets, metadata);
            let (_, stats, lock_nanos) =
                self.commit_cache_update(update, hooks, CacheMutationTime::Current)?;
            self.cache_mutation_lock_operations
                .fetch_add(1, Ordering::Relaxed);
            self.cache_mutation_lock_nanos
                .fetch_add(lock_nanos, Ordering::Relaxed);

            if let Some(cache_obj) = response
                .metadata
                .as_object_mut()
                .and_then(|metadata| metadata.get_mut("cache"))
                .and_then(Value::as_object_mut)
            {
                cache_obj.insert("evictions".to_string(), json!(stats.evictions));
            } else {
                response.metadata = merge_json(
                    response.metadata,
                    json!({
                        "cache": {
                            "evictions": stats.evictions,
                        }
                    }),
                );
            }
        }

        Ok(response)
    }

    fn commit_cache_update(
        &self,
        update: PendingCacheUpdate,
        hooks: &mut dyn DeltaReuseHooks,
        mutation_time: CacheMutationTime,
    ) -> Result<(String, ContextStoreStats, u64), KernelError> {
        match update {
            PendingCacheUpdate::Volatile(mutation) => {
                let mut cache = self.lock_memory()?;
                let lock_started = Instant::now();
                let mutation_now = match mutation_time {
                    CacheMutationTime::Current => now_unix(),
                    #[cfg(test)]
                    CacheMutationTime::Fixed(created_at_unix) => created_at_unix,
                };
                let cache_key = mutation.lookup.cache_key.clone();
                let mut removed_cache_keys = self
                    .persist_ttl_secs
                    .map(|ttl_secs| cache.expired_entry_keys_at(ttl_secs, mutation_now))
                    .unwrap_or_default();
                removed_cache_keys.retain(|removed| removed != &cache_key);
                let entry = cache.put_at_with_hooks(
                    &mutation.target,
                    &mutation.lookup,
                    mutation.packets,
                    mutation.metadata,
                    mutation_now,
                    hooks,
                );
                let evicted_cache_keys = self
                    .persist_ttl_secs
                    .map(|ttl_secs| cache.evict_expired_entries_at(ttl_secs, mutation_now))
                    .unwrap_or_default();
                debug_assert_eq!(evicted_cache_keys, removed_cache_keys);
                let stats = cache.stats();
                let lock_nanos = lock_started
                    .elapsed()
                    .as_nanos()
                    .try_into()
                    .unwrap_or(u64::MAX);
                Ok((entry.cache_key, stats, lock_nanos))
            }
            PendingCacheUpdate::Persistent(mutation) => {
                let persistence = self.persistence.as_ref().ok_or_else(|| {
                    self.cache_persistence_error("prepared persistence owner is unavailable")
                })?;
                let outcome = match mutation_time {
                    CacheMutationTime::Current => {
                        persistence.commit_prepared_update(mutation, hooks)
                    }
                    #[cfg(test)]
                    CacheMutationTime::Fixed(created_at_unix) => {
                        persistence.commit_prepared_update_at(mutation, hooks, created_at_unix)
                    }
                }
                .map_err(|source| self.map_cache_persistence_error(source))?;
                Ok((outcome.cache_key, outcome.stats, outcome.lock_nanos))
            }
        }
    }

    pub fn execute_sequence(
        &self,
        req: KernelSequenceRequest,
    ) -> Result<KernelSequenceResponse, KernelError> {
        let mut observer = NoopSequenceObserver;
        self.execute_sequence_with_observer(req, &mut observer)
    }

    pub fn execute_sequence_with_observer(
        &self,
        req: KernelSequenceRequest,
        observer: &mut dyn SequenceObserver,
    ) -> Result<KernelSequenceResponse, KernelError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let req = normalize_sequence_request(req)?;
        let task_id = resolve_sequence_task_id(&req);
        let budget = req.budget;
        let reactive = req.reactive;
        let original_steps = req.steps;

        let mut remaining = original_steps.clone();
        let mut step_results = Vec::new();
        let mut scheduled = Vec::new();
        let mut skipped = Vec::new();
        let mut consumed_estimate = context_scheduler_core::StepEstimate::default();
        let mut budget_exhausted = false;
        let mut completed_success = BTreeSet::<String>::new();
        let mut last_event_count = 0usize;
        let mut replans = Vec::<Value>::new();

        ensure_sequence_active(observer, task_id.as_deref())?;

        if reactive.enabled {
            if let Some(task_id) = task_id.as_deref() {
                let entries = self.lock_memory()?.entries();
                let plan = self.services.reactive_planner.plan(ReactivePlanRequest {
                    task_id,
                    remaining: &remaining,
                    original_steps: &original_steps,
                    completed_success: &completed_success,
                    mode: reactive.mode,
                    append_focused_map: reactive.append_focused_map,
                    anchor_step_id: None,
                    cache_entries: &entries,
                })?;
                last_event_count = plan.event_count;
                if !plan.mutations.is_empty() {
                    let schedule_mutations = to_schedule_mutations(&plan.mutations);
                    let applied = context_scheduler_core::apply_mutations(
                        &remaining
                            .iter()
                            .map(schedule_step_from_kernel)
                            .collect::<Vec<_>>(),
                        &schedule_mutations,
                    )
                    .map_err(|source| KernelError::SchedulerFailed {
                        detail: source.to_string(),
                    })?;
                    record_replan_cancellations(
                        &remaining,
                        &applied.applied,
                        &mut skipped,
                        &mut step_results,
                    );
                    remaining = apply_kernel_mutations(&remaining, &plan.mutations);
                    replans.push(json!({
                        "trigger": "initial_state",
                        "event_count": plan.event_count,
                        "applied_mutations": applied.applied,
                    }));
                    observer.on_replan_applied(
                        None,
                        plan.event_count,
                        replans.last().unwrap_or(&Value::Null),
                    );
                    ensure_sequence_active(observer, Some(task_id))?;
                }
            }
        }

        while !remaining.is_empty() {
            ensure_sequence_active(observer, task_id.as_deref())?;
            let schedule =
                context_scheduler_core::schedule(context_scheduler_core::ScheduleRequest {
                    steps: remaining
                        .iter()
                        .map(schedule_step_from_kernel)
                        .collect::<Vec<_>>(),
                    budget: schedule_budget_remaining(budget, consumed_estimate),
                })
                .map_err(|source| KernelError::SchedulerFailed {
                    detail: source.to_string(),
                })?;

            let Some(next_step_id) = schedule.ordered_steps.first().map(|step| step.id.clone())
            else {
                budget_exhausted = schedule.budget_exhausted;
                for step in remaining.drain(..) {
                    skipped.push(step.id.clone());
                    step_results.push(KernelStepResponse {
                        id: step.id,
                        target: step.target,
                        status: "skipped".to_string(),
                        response: None,
                        failure: Some(KernelFailure {
                            code: if budget_exhausted {
                                "budget_exceeded".to_string()
                            } else {
                                "dependency_not_satisfied".to_string()
                            },
                            message: if budget_exhausted {
                                "step skipped: budget_exceeded".to_string()
                            } else {
                                "step skipped: dependency_not_satisfied".to_string()
                            },
                            target: None,
                        }),
                    });
                }
                break;
            };

            let original = take_scheduled_step(&mut remaining, &next_step_id)?;
            let position = scheduled.len() + 1;
            observer.on_step_started(position, &original);
            ensure_sequence_active(observer, task_id.as_deref())?;
            let estimate = kernel_step_estimate(&original);
            consumed_estimate = context_scheduler_core::StepEstimate {
                tokens: consumed_estimate.tokens.saturating_add(estimate.tokens),
                bytes: consumed_estimate.bytes.saturating_add(estimate.bytes),
                runtime_ms: consumed_estimate
                    .runtime_ms
                    .saturating_add(estimate.runtime_ms),
            };

            let response = self.execute(KernelRequest {
                target: original.target.clone(),
                input_packets: original.input_packets.clone(),
                budget: if original.budget == ExecutionBudget::default() {
                    budget
                } else {
                    original.budget
                },
                policy_context: policy_context_with_task_id(
                    original.policy_context.clone(),
                    task_id.as_deref(),
                ),
                reducer_input: original.reducer_input.clone(),
            });

            match response {
                Ok(response) => {
                    scheduled.push(original.id.clone());
                    completed_success.insert(original.id.clone());
                    remove_satisfied_dependency(&mut remaining, &original.id);
                    observer.on_step_completed(position, &original, &response);
                    ensure_sequence_active(observer, task_id.as_deref())?;
                    step_results.push(KernelStepResponse {
                        id: original.id.clone(),
                        target: original.target.clone(),
                        status: "ok".to_string(),
                        response: Some(response),
                        failure: None,
                    });

                    if reactive.enabled {
                        if let Some(task_id) = task_id.as_deref() {
                            let entries = self.lock_memory()?.entries();
                            let plan =
                                self.services.reactive_planner.plan(ReactivePlanRequest {
                                    task_id,
                                    remaining: &remaining,
                                    original_steps: &original_steps,
                                    completed_success: &completed_success,
                                    mode: reactive.mode,
                                    append_focused_map: reactive.append_focused_map,
                                    anchor_step_id: Some(&original.id),
                                    cache_entries: &entries,
                                })?;
                            if plan.event_count > last_event_count {
                                if !plan.mutations.is_empty() {
                                    let schedule_mutations = to_schedule_mutations(&plan.mutations);
                                    let applied = context_scheduler_core::apply_mutations(
                                        &remaining
                                            .iter()
                                            .map(schedule_step_from_kernel)
                                            .collect::<Vec<_>>(),
                                        &schedule_mutations,
                                    )
                                    .map_err(|source| KernelError::SchedulerFailed {
                                        detail: source.to_string(),
                                    })?;
                                    record_replan_cancellations(
                                        &remaining,
                                        &applied.applied,
                                        &mut skipped,
                                        &mut step_results,
                                    );
                                    remaining = apply_kernel_mutations(&remaining, &plan.mutations);
                                    replans.push(json!({
                                        "trigger": "task_state_update",
                                        "after_step": original.id,
                                        "event_count": plan.event_count,
                                        "applied_mutations": applied.applied,
                                    }));
                                    observer.on_replan_applied(
                                        Some(&original.id),
                                        plan.event_count,
                                        replans.last().unwrap_or(&Value::Null),
                                    );
                                }
                                last_event_count = plan.event_count;
                            }
                        }
                    }
                }
                Err(err) => {
                    let failure = err.structured();
                    observer.on_step_failed(position, &original, &failure);
                    ensure_sequence_active(observer, task_id.as_deref())?;
                    let failed_dependents = remove_failed_dependents(&mut remaining, &original.id);
                    step_results.push(KernelStepResponse {
                        id: original.id.clone(),
                        target: original.target.clone(),
                        status: "failed".to_string(),
                        response: None,
                        failure: Some(failure),
                    });
                    for skipped_step in failed_dependents {
                        skipped.push(skipped_step.id.clone());
                        step_results.push(KernelStepResponse {
                            id: skipped_step.id,
                            target: skipped_step.target,
                            status: "skipped".to_string(),
                            response: None,
                            failure: Some(KernelFailure {
                                code: "dependency_failed".to_string(),
                                message: "step skipped due to failed dependency".to_string(),
                                target: None,
                            }),
                        });
                    }
                }
            }
        }

        Ok(KernelSequenceResponse {
            request_id,
            scheduled,
            skipped,
            budget_exhausted,
            step_results,
            metadata: json!({
                "estimated_usage": {
                    "tokens": consumed_estimate.tokens,
                    "bytes": consumed_estimate.bytes,
                    "runtime_ms": consumed_estimate.runtime_ms,
                },
                "reactive": {
                    "enabled": reactive.enabled,
                    "task_id": task_id,
                    "replans": replans,
                }
            }),
        })
    }
}

pub(crate) fn take_scheduled_step(
    remaining: &mut Vec<KernelStepRequest>,
    scheduled_id: &str,
) -> Result<KernelStepRequest, KernelError> {
    let next_idx = remaining
        .iter()
        .position(|step| step.id == scheduled_id)
        .ok_or_else(|| KernelError::SchedulerFailed {
            detail: format!(
                "scheduler selected step `{scheduled_id}` that is absent from the remaining plan"
            ),
        })?;
    Ok(remaining.remove(next_idx))
}

fn ensure_sequence_active(
    observer: &dyn SequenceObserver,
    task_id: Option<&str>,
) -> Result<(), KernelError> {
    if observer.should_cancel() {
        return Err(KernelError::SequenceCancelled {
            task_id: task_id.map(ToOwned::to_owned),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_lookup(
        kernel: &KernelMechanism,
        target: &str,
        reducer_input: Value,
        hooks: &mut dyn DeltaReuseHooks,
    ) -> context_memory_core::CacheLookup {
        kernel
            .lock_memory()
            .unwrap()
            .lookup_with_hooks(target, &reducer_input, hooks)
    }

    fn commit_test_entry_at(
        kernel: &KernelMechanism,
        target: &str,
        reducer_input: Value,
        created_at_unix: u64,
    ) -> context_memory_core::PacketCacheEntry {
        let mut hooks = NoopDeltaReuseHooks;
        let lookup = cache_lookup(kernel, target, reducer_input, &mut hooks);
        let update =
            kernel.prepare_cache_update(target, &lookup, vec![CachePacket::default()], Value::Null);
        let cache_key = kernel
            .commit_cache_update(
                update,
                &mut hooks,
                CacheMutationTime::Fixed(created_at_unix),
            )
            .unwrap()
            .0;
        kernel
            .lock_memory()
            .unwrap()
            .get(&cache_key)
            .cloned()
            .expect("committed cache entry must be immediately visible")
    }

    #[test]
    fn strict_persistence_constructor_rejects_an_unopenable_owner() {
        let root = tempfile::tempdir().unwrap();
        let cache_dir = root.path().join(".packet28");
        std::fs::create_dir(&cache_dir).unwrap();
        std::fs::create_dir(cache_dir.join("packet-cache-v3.lock")).unwrap();

        let error = match KernelMechanism::try_with_persistence(PersistConfig::new(
            root.path().to_path_buf(),
        )) {
            Ok(_) => panic!("strict persistent kernel unexpectedly opened"),
            Err(error) => error,
        };

        assert!(matches!(error, KernelError::CachePersistence { .. }));
    }

    #[test]
    fn missing_scheduled_step_is_a_typed_scheduler_failure() {
        let mut remaining = vec![KernelStepRequest {
            id: "known".to_string(),
            target: "custom.reducer".to_string(),
            ..KernelStepRequest::default()
        }];

        let error = take_scheduled_step(&mut remaining, "missing").unwrap_err();

        assert_eq!(remaining.len(), 1);
        assert!(matches!(
            error,
            KernelError::SchedulerFailed { detail }
                if detail
                    == "scheduler selected step `missing` that is absent from the remaining plan"
        ));
    }

    #[test]
    fn cache_update_uses_one_timestamp_at_the_ttl_boundary() {
        const TTL_SECS: u64 = 3_600;
        let dir = tempfile::tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf()).with_ttl_secs(TTL_SECS);
        let kernel = KernelMechanism::try_with_persistence(config.clone()).unwrap();
        let boundary = now_unix().saturating_add(600);
        let old_entry = commit_test_entry_at(
            &kernel,
            "test.reducer",
            json!({"entry":"old"}),
            boundary.saturating_sub(TTL_SECS),
        );
        kernel
            .flush_cache_persistence(Duration::from_secs(2))
            .unwrap();

        let mut hooks = NoopDeltaReuseHooks;
        let lookup = cache_lookup(&kernel, "test.reducer", json!({"entry":"new"}), &mut hooks);
        let update = kernel.prepare_cache_update(
            "test.reducer",
            &lookup,
            vec![CachePacket::default()],
            Value::Null,
        );
        let new_cache_key = kernel
            .commit_cache_update(update, &mut hooks, CacheMutationTime::Fixed(boundary))
            .unwrap()
            .0;
        let new_entry = kernel
            .lock_memory()
            .unwrap()
            .get(&new_cache_key)
            .cloned()
            .expect("committed cache entry must be immediately visible");

        assert!(kernel
            .context_store_get(&old_entry.cache_key)
            .unwrap()
            .is_some());
        assert!(kernel
            .context_store_get(&new_entry.cache_key)
            .unwrap()
            .is_some());
        kernel
            .shutdown_cache_persistence(Duration::from_secs(2))
            .unwrap();
        drop(kernel);

        let reopened = KernelMechanism::try_with_persistence(config).unwrap();
        assert_eq!(reopened.context_store_stats().unwrap().entries, 2);
        assert!(reopened
            .context_store_get(&old_entry.cache_key)
            .unwrap()
            .is_some());
        assert!(reopened
            .context_store_get(&new_entry.cache_key)
            .unwrap()
            .is_some());
        reopened
            .shutdown_cache_persistence(Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn rejected_persistence_update_is_absent_from_memory_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf());
        let kernel = KernelMechanism::try_with_persistence(config.clone()).unwrap();
        let baseline = commit_test_entry_at(
            &kernel,
            "test.reducer",
            json!({"entry":"baseline"}),
            now_unix(),
        );
        kernel
            .flush_cache_persistence(Duration::from_secs(2))
            .unwrap();
        kernel
            .persistence
            .as_ref()
            .unwrap()
            .shutdown(Duration::from_secs(2))
            .unwrap();

        let mut hooks = NoopDeltaReuseHooks;
        let lookup = cache_lookup(
            &kernel,
            "test.reducer",
            json!({"entry":"rejected"}),
            &mut hooks,
        );
        let rejected_key = lookup.cache_key.clone();
        let update = kernel.prepare_cache_update(
            "test.reducer",
            &lookup,
            vec![CachePacket::default()],
            Value::Null,
        );
        let error = kernel
            .commit_cache_update(update, &mut hooks, CacheMutationTime::Current)
            .unwrap_err();

        assert!(matches!(error, KernelError::CachePersistence { .. }));
        assert_eq!(kernel.context_store_stats().unwrap().entries, 1);
        assert!(kernel
            .context_store_get(&baseline.cache_key)
            .unwrap()
            .is_some());
        assert!(kernel.context_store_get(&rejected_key).unwrap().is_none());
        assert!(kernel
            .cache_runtime_metrics()
            .persistence_error
            .is_some_and(|detail| detail.contains("worker is unavailable")));
        drop(kernel);

        let reopened = KernelMechanism::try_with_persistence(config).unwrap();
        assert_eq!(reopened.context_store_stats().unwrap().entries, 1);
        assert!(reopened
            .context_store_get(&baseline.cache_key)
            .unwrap()
            .is_some());
        assert!(reopened.context_store_get(&rejected_key).unwrap().is_none());
        reopened
            .shutdown_cache_persistence(Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn prune_uses_one_timestamp_at_the_ttl_boundary() {
        const TTL_SECS: u64 = 3_600;
        let dir = tempfile::tempdir().unwrap();
        let config = PersistConfig::new(dir.path().to_path_buf()).with_ttl_secs(TTL_SECS);
        let kernel = KernelMechanism::try_with_persistence(config.clone()).unwrap();
        let boundary = now_unix().saturating_add(600);
        let entry = commit_test_entry_at(
            &kernel,
            "test.reducer",
            json!({"entry":"prune-boundary"}),
            boundary.saturating_sub(TTL_SECS),
        );
        let request = ContextStorePruneRequest {
            all: false,
            ttl_secs: Some(TTL_SECS),
        };

        let at_boundary = kernel
            .context_store_prune_at(request.clone(), Duration::from_secs(2), boundary)
            .unwrap();
        let after_boundary = kernel
            .context_store_prune_at(request, Duration::from_secs(2), boundary.saturating_add(1))
            .unwrap();

        assert_eq!(at_boundary.removed, 0);
        assert_eq!(after_boundary.removed, 1);
        assert!(kernel
            .context_store_get(&entry.cache_key)
            .unwrap()
            .is_none());
        kernel
            .shutdown_cache_persistence(Duration::from_secs(2))
            .unwrap();
        drop(kernel);

        let reopened = KernelMechanism::try_with_persistence(config).unwrap();
        assert_eq!(reopened.context_store_stats().unwrap().entries, 0);
        reopened
            .shutdown_cache_persistence(Duration::from_secs(2))
            .unwrap();
    }
}
