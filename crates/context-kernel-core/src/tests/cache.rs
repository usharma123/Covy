use super::*;

#[test]
fn caches_reducer_packets_by_request_hash() {
    let mut kernel = Kernel::new();
    let calls = Arc::new(AtomicU64::new(0));
    let calls_ref = calls.clone();
    kernel.register_reducer("count.reducer", move |_ctx, _packets| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: json!({"source":"reducer"}),
        })
    });

    let request = KernelRequest {
        target: "count.reducer".to_string(),
        reducer_input: json!({"task":"same"}),
        ..KernelRequest::default()
    };

    let first = kernel.execute(request.clone()).unwrap();
    let second = kernel.execute(request).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(first.output_packets.len(), 1);
    assert_eq!(second.output_packets.len(), 1);
    assert_eq!(
        second
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn cache_fingerprint_changes_force_cache_miss() {
    let mut kernel = Kernel::new();
    let calls = Arc::new(AtomicU64::new(0));
    let calls_ref = calls.clone();
    kernel.register_reducer("count.reducer", move |_ctx, _packets| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: json!({"source":"reducer"}),
        })
    });

    let mut request = KernelRequest {
        target: "count.reducer".to_string(),
        reducer_input: json!({"task":"same"}),
        policy_context: json!({"cache_fingerprint":"fp-1"}),
        ..KernelRequest::default()
    };

    let first = kernel.execute(request.clone()).unwrap();
    assert_eq!(
        first
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(false)
    );

    let second = kernel.execute(request.clone()).unwrap();
    assert_eq!(
        second
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(true)
    );

    request.policy_context = json!({"cache_fingerprint":"fp-2"});
    let third = kernel.execute(request).unwrap();
    assert_eq!(
        third
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[test]
fn persistent_kernel_reuses_cache_across_instances() {
    let dir = tempdir().unwrap();
    let config = PersistConfig::new(dir.path().to_path_buf());

    let first_calls = Arc::new(AtomicU64::new(0));
    let first_calls_ref = first_calls.clone();
    let mut first_kernel = Kernel::with_v1_reducers_and_persistence(config.clone());
    first_kernel.register_reducer("count.reducer", move |_ctx, _packets| {
        first_calls_ref.fetch_add(1, Ordering::Relaxed);
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: json!({"source":"reducer"}),
        })
    });

    let request = KernelRequest {
        target: "count.reducer".to_string(),
        reducer_input: json!({"task":"persisted"}),
        ..KernelRequest::default()
    };

    let first = first_kernel.execute(request.clone()).unwrap();
    assert_eq!(first_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        first
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(false)
    );
    drop(first_kernel);

    let second_calls = Arc::new(AtomicU64::new(0));
    let second_calls_ref = second_calls.clone();
    let mut second_kernel = Kernel::with_v1_reducers_and_persistence(config);
    second_kernel.register_reducer("count.reducer", move |_ctx, _packets| {
        second_calls_ref.fetch_add(1, Ordering::Relaxed);
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: json!({"source":"reducer"}),
        })
    });

    let second = second_kernel.execute(request).unwrap();
    assert_eq!(second_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        second
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(dir.path().join(".packet28/packet-cache-v3.bin").exists());
}

#[test]
fn persistent_kernel_flushes_deltas_after_releasing_cache_lock() {
    let dir = tempdir().unwrap();
    let config = PersistConfig::new(dir.path().to_path_buf());
    let mut kernel = Kernel::with_persistence(config.clone());
    kernel.register_reducer("count.reducer", |_ctx, _packets| {
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: Value::Null,
        })
    });

    kernel
        .execute(KernelRequest {
            target: "count.reducer".to_string(),
            reducer_input: json!({"task":"flush"}),
            ..KernelRequest::default()
        })
        .unwrap();
    let persistence = kernel
        .flush_cache_persistence(Duration::from_secs(2))
        .unwrap();
    let runtime = kernel.cache_runtime_metrics();
    let loaded = context_memory_core::PacketCache::load_from_disk(&config);

    assert_eq!(loaded.len(), 1);
    assert_eq!(runtime.mutation_lock_operations, 1);
    assert!(runtime.mutation_lock_nanos > 0);
    assert_eq!(persistence.persisted_deltas, 1);
    assert!(persistence.wal_bytes > 0);
    assert_eq!(persistence.checkpoints, 0);
}

#[test]
fn persistent_kernel_concurrent_writes_replay_every_entry() {
    let dir = tempdir().unwrap();
    let config = PersistConfig::new(dir.path().to_path_buf());
    let mut kernel = Kernel::with_persistence(config.clone());
    kernel.register_reducer("echo.reducer", |ctx, _packets| {
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(ctx.reducer_input.clone(), None)],
            metadata: Value::Null,
        })
    });
    let kernel = Arc::new(kernel);
    let mut workers = Vec::new();
    for worker_id in 0..8 {
        let kernel = kernel.clone();
        workers.push(std::thread::spawn(move || {
            for offset in 0..8 {
                kernel
                    .execute(KernelRequest {
                        target: "echo.reducer".to_string(),
                        reducer_input: json!({
                            "worker": worker_id,
                            "offset": offset,
                        }),
                        ..KernelRequest::default()
                    })
                    .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    kernel
        .flush_cache_persistence(Duration::from_secs(5))
        .unwrap();
    let loaded = context_memory_core::PacketCache::load_from_disk(&config);
    assert_eq!(loaded.len(), 64);
}

#[test]
fn persistent_kernel_reports_background_filesystem_failure_on_flush() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let mut kernel = Kernel::with_persistence(PersistConfig::new(root.clone()));
    std::fs::remove_dir(&root).unwrap();
    std::fs::write(&root, b"file").unwrap();
    kernel.register_reducer("count.reducer", |_ctx, _packets| {
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: Value::Null,
        })
    });

    kernel
        .execute(KernelRequest {
            target: "count.reducer".to_string(),
            reducer_input: json!({"task":"failure"}),
            ..KernelRequest::default()
        })
        .unwrap();
    let error = kernel
        .flush_cache_persistence(Duration::from_secs(2))
        .unwrap_err();

    assert!(matches!(error, KernelError::CachePersistence { .. }));
    assert!(kernel.cache_runtime_metrics().persistence_error.is_some());
}

#[test]
fn governed_cache_reuses_entries_for_same_policy_content_across_paths() {
    let dir = tempdir().unwrap();
    let persist = PersistConfig::new(dir.path().to_path_buf());
    let config_a = dir.path().join("policy-a.yaml");
    let config_b = dir.path().join("policy-b.yaml");
    write_policy_file(&config_a, &["diffy"], &[]);
    write_policy_file(&config_b, &["diffy"], &[]);

    let calls = Arc::new(AtomicU64::new(0));
    let calls_ref = calls.clone();
    let mut kernel = Kernel::with_v1_reducers_and_persistence(persist);
    kernel.register_reducer("count.reducer", move |_ctx, _packets| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(
                json!({
                    "tool": "diffy",
                    "reducer": "analyze",
                    "paths": ["src/lib.rs"],
                    "payload": {"message": "ok"},
                }),
                None,
            )],
            metadata: json!({"source":"reducer"}),
        })
    });

    let mut request = KernelRequest {
        target: "count.reducer".to_string(),
        reducer_input: json!({"task":"governed-cache"}),
        policy_context: json!({
            "config_path": config_a.to_string_lossy().to_string()
        }),
        ..KernelRequest::default()
    };
    let first = kernel.execute(request.clone()).unwrap();
    assert_eq!(
        first
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(false)
    );
    assert!(first.audit.governance.enabled);

    request.policy_context = json!({
        "config_path": config_b.to_string_lossy().to_string()
    });
    let second = kernel.execute(request).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        second
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn governed_cache_misses_when_policy_content_changes() {
    let dir = tempdir().unwrap();
    let persist = PersistConfig::new(dir.path().to_path_buf());
    let config_path = dir.path().join("policy.yaml");
    write_policy_file(&config_path, &["diffy"], &[]);

    let calls = Arc::new(AtomicU64::new(0));
    let calls_ref = calls.clone();
    let mut kernel = Kernel::with_v1_reducers_and_persistence(persist);
    kernel.register_reducer("count.reducer", move |_ctx, _packets| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(
                json!({
                    "tool": "diffy",
                    "reducer": "analyze",
                    "paths": ["src/lib.rs"],
                    "payload": {"message": "ok"},
                }),
                None,
            )],
            metadata: json!({"source":"reducer"}),
        })
    });

    let request = KernelRequest {
        target: "count.reducer".to_string(),
        reducer_input: json!({"task":"governed-cache"}),
        policy_context: json!({
            "config_path": config_path.to_string_lossy().to_string()
        }),
        ..KernelRequest::default()
    };
    let first = kernel.execute(request.clone()).unwrap();
    assert_eq!(
        first
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(false)
    );

    std::fs::write(
        &config_path,
        r#"
version: 1
policy:
  allowed_tools: ["diffy"]
  allowed_reducers: []
  paths:
    include: ["src/**"]
    exclude: []
  budgets:
    token_cap: 9000
    runtime_ms_cap: 2000
  redaction:
    forbidden_patterns: []
"#,
    )
    .unwrap();

    let second = kernel.execute(request).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        second
            .metadata
            .get("cache")
            .and_then(|v| v.get("hit"))
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn governed_cache_hit_rechecks_output_policy_audits() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("policy.yaml");
    std::fs::write(
        &config_path,
        r#"
version: 1
policy:
  paths:
    include: ["src/**"]
    exclude: []
  redaction:
    forbidden_patterns: ["secret123"]
"#,
    )
    .unwrap();

    let mut kernel = Kernel::new();
    kernel.register_reducer("count.reducer", |_ctx, _packets| {
        Ok(ReducerResult {
            output_packets: vec![KernelPacket::from_value(json!({"ok": true}), None)],
            metadata: Value::Null,
        })
    });

    let request = KernelRequest {
        target: "count.reducer".to_string(),
        policy_context: json!({
            "config_path": config_path.to_string_lossy().to_string()
        }),
        ..KernelRequest::default()
    };
    let policy_guard = load_policy_guard(&request.policy_context).unwrap().unwrap();
    let cache_input = cache_input_for_request(&request, &request.target, Some(&policy_guard));
    let mut hooks = NoopDeltaReuseHooks;

    let lookup = {
        let cache = kernel.memory.lock().unwrap();
        cache.lookup_with_hooks(&request.target, &cache_input, &mut hooks)
    };
    {
        let mut cache = kernel.memory.lock().unwrap();
        cache.put_with_hooks(
            &request.target,
            &lookup,
            vec![CachePacket {
                packet_id: Some("cached-bad".to_string()),
                body: json!({
                    "tool": "diffy",
                    "reducer": "analyze",
                    "paths": ["src/lib.rs"],
                    "payload": {"secret": "secret123"},
                }),
                token_usage: None,
                runtime_ms: None,
                metadata: Value::Null,
            }],
            Value::Null,
            &mut hooks,
        );
    }

    let err = kernel.execute(request).unwrap_err();
    assert!(matches!(err, KernelError::PolicyViolation { .. }));
}
