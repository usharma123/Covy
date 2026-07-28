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
    assert!(dir.path().join(".packet28/packet-cache-v2.bin").exists());
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
