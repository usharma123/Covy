use std::error::Error as _;
use std::io::ErrorKind;

use serde_json::Value;
use testy_cli_common::impact::{
    run_impact_command, run_legacy_impact, ImpactArgs, ImpactCommand, ImpactRunArgs,
    ImpactRunnerOptions, LegacyImpactArgs,
};
use testy_cli_common::shard::{
    run_shard_command, run_shard_plan_command, run_shard_update_command, ShardArgs, ShardPlanArgs,
    ShardUpdateArgs,
};
use testy_cli_common::support::deserialize_json_with_example;
use testy_cli_common::testmap::{
    run_testmap_build, run_testmap_command, TestmapArgs, TestmapBuildArgs, TestmapRunnerOptions,
};
use testy_cli_common::{Result, TestyCliError};
use testy_core::error::TestyError;

#[test]
fn public_helpers_expose_the_typed_result_alias() {
    type ImpactCommandFn =
        for<'a, 'b> fn(ImpactArgs, &'a str, &'b ImpactRunnerOptions) -> Result<i32>;
    type LegacyImpactFn = for<'a> fn(LegacyImpactArgs, &'a str) -> Result<i32>;
    type ShardCommandFn = for<'a> fn(ShardArgs, &'a str) -> Result<i32>;
    type ShardPlanFn = for<'a> fn(ShardPlanArgs, &'a str) -> Result<i32>;
    type ShardUpdateFn = for<'a> fn(ShardUpdateArgs, &'a str) -> Result<i32>;
    type TestmapCommandFn = for<'a> fn(TestmapArgs, &'a TestmapRunnerOptions) -> Result<i32>;
    type TestmapBuildFn = for<'a> fn(TestmapBuildArgs, &'a TestmapRunnerOptions) -> Result<i32>;

    let _: ImpactCommandFn = run_impact_command;
    let _: LegacyImpactFn = run_legacy_impact;
    let _: ShardCommandFn = run_shard_command;
    let _: ShardPlanFn = run_shard_plan_command;
    let _: ShardUpdateFn = run_shard_update_command;
    let _: TestmapCommandFn = run_testmap_command;
    let _: TestmapBuildFn = run_testmap_build;

    let parsed: Result<Value> =
        deserialize_json_with_example(r#"{"ok":true}"#, "Widget", r#"{"ok": boolean}"#);
    assert_eq!(parsed.unwrap()["ok"], true);
}

#[test]
fn core_and_io_errors_remain_concrete_and_chained() {
    let directory = tempfile::tempdir().unwrap();
    let missing_plan = directory.path().join("missing-plan.json");
    let error = run_impact_command(
        ImpactArgs {
            command: Some(ImpactCommand::Run(ImpactRunArgs {
                plan: Some(missing_plan.to_string_lossy().into_owned()),
                schema: false,
                command: vec!["true".to_string()],
            })),
            legacy: LegacyImpactArgs::default(),
        },
        "unused.toml",
        &ImpactRunnerOptions::for_binary("testy"),
    )
    .unwrap_err();

    match &error {
        TestyCliError::Core(TestyError::Io { path, source, .. }) => {
            assert_eq!(path, &missing_plan);
            assert_eq!(source.kind(), ErrorKind::NotFound);
        }
        other => panic!("expected a typed core I/O error, got {other:?}"),
    }

    let mut source = error.source();
    let mut retained_io = false;
    while let Some(cause) = source {
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>() {
            retained_io = io_error.kind() == ErrorKind::NotFound;
            break;
        }
        source = cause.source();
    }
    assert!(retained_io, "I/O source was missing from the error chain");
}

#[test]
fn contextual_json_error_retains_decoder_source_and_legacy_display() {
    let error =
        deserialize_json_with_example::<Value>("{", "Widget", r#"{"ok": boolean}"#).unwrap_err();

    let TestyCliError::JsonWithExample {
        type_name,
        source,
        example,
    } = &error
    else {
        panic!("expected contextual JSON error, got {error:?}");
    };
    assert_eq!(type_name, "Widget");
    assert_eq!(example, r#"{"ok": boolean}"#);
    assert_eq!(
        error.to_string(),
        format!("Failed to parse Widget: {source}\n\nExpected JSON shape:\n{{\"ok\": boolean}}")
    );
    assert!(error
        .source()
        .is_some_and(|cause| cause.downcast_ref::<serde_json::Error>().is_some()));
}

#[test]
fn transparent_json_conversion_retains_the_concrete_error() {
    let source = serde_json::from_str::<Value>("{").unwrap_err();
    let expected = source.to_string();
    let error = TestyCliError::from(source);

    match &error {
        TestyCliError::Json(source) => assert_eq!(source.to_string(), expected),
        other => panic!("expected a transparent JSON error, got {other:?}"),
    }
    assert_eq!(error.to_string(), expected);
}

#[test]
fn public_error_is_send_sync_and_static() {
    fn assert_error<T: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<TestyCliError>();
}
