use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn multi_packet_fixture_dedupes_overlapping_refs() {
    let assembled = contextq_core::assemble_packet_files(
        &[fixture_path("packet_a.json"), fixture_path("packet_b.json")],
        contextq_core::AssembleOptions {
            budget_tokens: 1200,
            budget_bytes: 24_000,
            ..contextq_core::AssembleOptions::default()
        },
    )
    .unwrap();

    let payload: contextq_core::AssembledPayload =
        serde_json::from_value(assembled.payload.clone()).unwrap();

    let src_lib_refs = payload
        .refs
        .iter()
        .filter(|reference| reference.kind == "file" && reference.value == "src/lib.rs")
        .count();
    let foo_symbol_refs = payload
        .refs
        .iter()
        .filter(|reference| reference.kind == "symbol" && reference.value == "foo::bar")
        .count();

    assert_eq!(assembled.assembly.input_packets, 2);
    assert_eq!(src_lib_refs, 1);
    assert_eq!(foo_symbol_refs, 1);
}

#[test]
fn budget_fixture_is_trimmed_to_caps() {
    let assembled = contextq_core::assemble_packet_files(
        &[fixture_path("packet_budget.json")],
        contextq_core::AssembleOptions {
            budget_tokens: 40,
            budget_bytes: 400,
            ..contextq_core::AssembleOptions::default()
        },
    )
    .unwrap();

    assert!(assembled.assembly.truncated);
    assert!(assembled.assembly.estimated_tokens <= 40);
    assert!(assembled.assembly.estimated_bytes <= 400);
}

#[test]
fn rejected_sections_do_not_consume_reference_deduplication_state() {
    use contextq_core::{
        AssembleOptions, AssembledPayload, ContextRef, ContextSection, InputPacket,
    };

    let reference = ContextRef {
        kind: "file".to_string(),
        value: "src/lib.rs".to_string(),
        ..ContextRef::default()
    };
    let sections = [
        ("oversized", "x".repeat(20_000), 3.0),
        ("kept", "failure".to_string(), 2.0),
        ("also kept", "follow-up".to_string(), 1.0),
    ]
    .into_iter()
    .map(|(title, body, relevance)| ContextSection {
        title: title.to_string(),
        body,
        refs: vec![reference.clone()],
        relevance: Some(relevance),
        ..ContextSection::default()
    })
    .collect::<Vec<_>>();

    for compact_assembly in [false, true] {
        for (budget_tokens, budget_bytes) in [(2_000, usize::MAX), (u64::MAX, 2_000)] {
            let assembled = contextq_core::assemble_packets(
                vec![InputPacket {
                    packet_id: Some("evidence".to_string()),
                    sections: sections.clone(),
                    ..InputPacket::default()
                }],
                AssembleOptions {
                    budget_tokens,
                    budget_bytes,
                    compact_assembly,
                    ..AssembleOptions::default()
                },
            );
            let payload: AssembledPayload = serde_json::from_value(assembled.payload).unwrap();

            assert_eq!(payload.sections.len(), 2);
            assert_eq!(payload.sections[0].title, "kept");
            assert_eq!(
                payload.sections[0].refs.len(),
                usize::from(!compact_assembly)
            );
            assert!(payload.sections[1].refs.is_empty());
            assert_eq!(payload.refs.len(), 1);
            assert_eq!(payload.refs[0].value, "src/lib.rs");
            assert_eq!(assembled.assembly.refs_input, 1);
            assert_eq!(assembled.assembly.refs_dropped, 0);
            assert_eq!(assembled.assembly.sections_dropped, 1);
            assert!(assembled.assembly.estimated_tokens <= budget_tokens);
            assert!(assembled.assembly.estimated_bytes <= budget_bytes);
        }
    }
}

#[test]
fn compact_sections_do_not_double_charge_global_references() {
    use contextq_core::{
        AssembleOptions, AssembledPayload, ContextRef, ContextSection, InputPacket,
    };

    let reference = ContextRef {
        kind: "file".to_string(),
        value: format!("src/{}.rs", "a".repeat(600)),
        ..ContextRef::default()
    };
    for (budget_tokens, budget_bytes) in [(260, usize::MAX), (u64::MAX, 1_000)] {
        let assembled = contextq_core::assemble_packets(
            vec![InputPacket {
                packet_id: Some("source".to_string()),
                sections: vec![ContextSection {
                    title: "Evidence".to_string(),
                    body: "failure".to_string(),
                    refs: vec![reference.clone()],
                    ..ContextSection::default()
                }],
                ..InputPacket::default()
            }],
            AssembleOptions {
                budget_tokens,
                budget_bytes,
                compact_assembly: true,
                ..AssembleOptions::default()
            },
        );
        let payload: AssembledPayload = serde_json::from_value(assembled.payload).unwrap();
        assert_eq!(payload.sections.len(), 1);
        assert!(payload.sections[0].refs.is_empty());
        assert_eq!(payload.refs.len(), 1);
        assert_eq!(payload.refs[0].value, reference.value);
        assert_eq!(assembled.assembly.refs_dropped, 0);
        assert!(assembled.assembly.estimated_bytes <= budget_bytes);
        assert!(assembled.assembly.estimated_tokens <= budget_tokens);
    }
}
