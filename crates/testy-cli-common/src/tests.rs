use std::io::ErrorKind;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde_json::Value;
use suite_packet_core::{CoverageFormat, CovyError};
use testy_core::error::{AdapterError, TestyError};

use crate::adapters::{default_impact_adapters, default_testmap_adapters};
use crate::impact::{
    run_impact_command, ImpactArgs, ImpactCommand, ImpactRunArgs, ImpactRunnerOptions,
    LegacyImpactArgs,
};
use crate::shard::{
    run_shard_command, JunitIdGranularityArg, PlannerAlgorithmArg, ShardArgs, ShardCommands,
    ShardPlanArgs,
};
use crate::support::deserialize_json_with_example;
use crate::testmap::{
    run_testmap_command, TestmapArgs, TestmapBuildArgs, TestmapCommands, TestmapRunnerOptions,
};
use crate::TestyCliError;

#[derive(Parser)]
#[command(name = "testy")]
struct TestCli {
    #[command(subcommand)]
    command: TestCommand,
}

#[derive(Subcommand)]
enum TestCommand {
    Impact(ImpactArgs),
    Shard(ShardArgs),
    Testmap(TestmapArgs),
}

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures")
        .join(relative)
}

#[test]
fn impact_plan_parses_defaults_and_selection_limits() {
    let cli = TestCli::try_parse_from([
        "testy",
        "impact",
        "plan",
        "--max-tests",
        "7",
        "--target-coverage",
        "0.75",
    ])
    .expect("parse impact plan");

    let TestCommand::Impact(ImpactArgs {
        command: Some(ImpactCommand::Plan(plan)),
        ..
    }) = cli.command
    else {
        panic!("expected impact plan command");
    };

    assert_eq!(plan.base_ref, "origin/main");
    assert_eq!(plan.head_ref, "HEAD");
    assert_eq!(plan.testmap, ".covy/state/testmap.bin");
    assert_eq!(plan.max_tests, Some(7));
    assert_eq!(plan.target_coverage, Some(0.75));
    assert_eq!(plan.format, "json");
}

#[test]
fn impact_run_preserves_trailing_command_arguments() {
    let cli = TestCli::try_parse_from([
        "testy",
        "impact",
        "run",
        "--plan",
        "plan.json",
        "--",
        "cargo",
        "test",
        "-p",
        "demo",
    ])
    .expect("parse impact run");

    let TestCommand::Impact(ImpactArgs {
        command: Some(ImpactCommand::Run(run)),
        ..
    }) = cli.command
    else {
        panic!("expected impact run command");
    };

    assert_eq!(run.plan.as_deref(), Some("plan.json"));
    assert_eq!(run.command, ["cargo", "test", "-p", "demo"]);
}

#[test]
fn shard_plan_requires_shard_count_without_schema() {
    let error = TestCli::try_parse_from(["testy", "shard", "plan"])
        .err()
        .expect("missing --shards must fail");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
    assert!(error.to_string().contains("--shards <SHARDS>"));
}

#[test]
fn shard_plan_schema_mode_does_not_require_shard_count() {
    let cli = TestCli::try_parse_from(["testy", "shard", "plan", "--schema"])
        .expect("schema mode should parse without --shards");

    let TestCommand::Shard(ShardArgs {
        command: ShardCommands::Plan(plan),
    }) = cli.command
    else {
        panic!("expected shard plan command");
    };

    assert!(plan.schema);
    assert_eq!(plan.shards, None);
    assert_eq!(plan.tier, "nightly");
}

#[test]
fn shard_value_enums_map_to_core_types() {
    assert_eq!(
        testy_core::command_shard::PlannerAlgorithmArg::from(PlannerAlgorithmArg::WhaleLpt),
        testy_core::command_shard::PlannerAlgorithmArg::WhaleLpt
    );
    assert_eq!(
        testy_core::shard_timing::JunitIdGranularity::from(JunitIdGranularityArg::Class),
        testy_core::shard_timing::JunitIdGranularity::Class
    );
}

#[test]
fn testmap_build_parses_public_defaults() {
    let cli =
        TestCli::try_parse_from(["testy", "testmap", "build", "--manifest", "reports/*.jsonl"])
            .expect("parse testmap build");

    let TestCommand::Testmap(TestmapArgs {
        command: TestmapCommands::Build(build),
    }) = cli.command
    else {
        panic!("expected testmap build command");
    };

    assert_eq!(build.manifest, ["reports/*.jsonl"]);
    assert_eq!(build.output, ".covy/state/testmap.bin");
    assert_eq!(build.timings_output, ".covy/state/testtimings.bin");
    assert!(!build.json);
    assert!(!build.schema);
}

#[test]
fn impact_runner_reports_missing_plan_with_runnable_usage() {
    let error = run_impact_command(
        ImpactArgs {
            command: Some(ImpactCommand::Run(ImpactRunArgs {
                plan: None,
                schema: false,
                command: vec!["cargo".to_string(), "test".to_string()],
            })),
            legacy: LegacyImpactArgs::default(),
        },
        "unused.toml",
        &ImpactRunnerOptions::for_binary("testy"),
    )
    .expect_err("missing plan must fail");

    let message = error.to_string();
    assert!(message.contains("Missing --plan"));
    assert!(message.contains("testy impact run --plan plan.json -- <command>"));
    assert!(matches!(error, TestyCliError::InvalidInput { .. }));
}

#[test]
fn shard_runner_delegates_missing_count_failure() {
    let error = run_shard_command(
        ShardArgs {
            command: ShardCommands::Plan(ShardPlanArgs {
                shards: None,
                tasks_json: None,
                tier: "nightly".to_string(),
                include_tag: Vec::new(),
                exclude_tag: Vec::new(),
                tests_file: None,
                impact_json: None,
                timings: None,
                unknown_test_seconds: None,
                algorithm: None,
                json: false,
                write_files: None,
                schema: false,
            }),
        },
        "unused.toml",
    )
    .expect_err("missing shard count must fail");

    assert_eq!(error.to_string(), "--shards is required");
    assert!(matches!(
        error,
        TestyCliError::Core(TestyError::InvalidInput { .. })
    ));
}

#[test]
fn testmap_runner_delegates_empty_manifest_failure() {
    let error = run_testmap_command(
        TestmapArgs {
            command: TestmapCommands::Build(TestmapBuildArgs {
                manifest: Vec::new(),
                output: "unused-testmap.bin".to_string(),
                timings_output: "unused-timings.bin".to_string(),
                json: false,
                schema: false,
            }),
        },
        &TestmapRunnerOptions::default(),
    )
    .expect_err("an empty manifest set must fail");

    assert_eq!(error.to_string(), "No manifest files found");
    assert!(matches!(
        error,
        TestyCliError::Core(TestyError::InvalidInput { .. })
    ));
}

#[test]
fn json_deserializer_accepts_valid_input() {
    let value: Value =
        deserialize_json_with_example(r#"{"ok":true}"#, "Widget", r#"{"ok": boolean}"#)
            .expect("parse valid JSON");

    assert_eq!(value["ok"], true);
}

#[test]
fn json_deserializer_adds_type_and_example_context() {
    let error = deserialize_json_with_example::<Value>("{", "Widget", r#"{"ok": boolean}"#)
        .expect_err("invalid JSON must fail");

    let message = error.to_string();
    assert!(message.contains("Failed to parse Widget"));
    assert!(message.contains("Expected JSON shape"));
    assert!(message.contains(r#"{"ok": boolean}"#));
}

#[test]
fn testmap_adapter_ingests_coverage_fixture() {
    let adapters = default_testmap_adapters();
    let coverage =
        (adapters.ingest_coverage)(&fixture("lcov/basic.info")).expect("ingest LCOV fixture");

    assert_eq!(coverage.format, Some(CoverageFormat::Lcov));
    assert_eq!(coverage.files.len(), 2);
}

#[test]
fn impact_adapter_honors_explicit_format() {
    let adapters = default_impact_adapters();
    let coverage = (adapters.ingest_coverage_with_format)(
        &fixture("cobertura/basic.xml"),
        CoverageFormat::Cobertura,
    )
    .expect("ingest Cobertura fixture");

    assert_eq!(coverage.format, Some(CoverageFormat::Cobertura));
    assert!(!coverage.files.is_empty());
}

#[test]
fn impact_adapter_preserves_missing_file_error() {
    let adapters = default_impact_adapters();
    let error = (adapters.ingest_coverage_auto)(&fixture("lcov/does-not-exist.info"))
        .expect_err("missing coverage input must fail");

    assert!(matches!(
        error,
        AdapterError::Coverage(CovyError::IoRaw(io_error))
            if io_error.kind() == ErrorKind::NotFound
    ));
}
