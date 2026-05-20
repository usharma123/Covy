#!/usr/bin/env bash
set -eo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/validate_refactor_batch.sh [--full] [--tests-only] [--parallel-tests] [--timings] [--list] [package[:filter] ...] [package:test-target:filter ...]

Runs the fast validation gate for an incremental refactor batch:
  - cargo fmt --check
  - package-scoped cargo clippy for changed Rust packages
  - targeted cargo test commands for changed Rust packages

Pass --full to run workspace clippy and cargo test --all-features after the
targeted package tests.
Pass --tests-only for the quick edit loop; it skips fmt and clippy but keeps the
same targeted cargo test selection. Do not use it as the pre-commit gate.
Pass --parallel-tests for local feedback when a selected test group is known to
be safe under the Rust test harness' default parallelism. The default remains
serial filtered tests for deterministic pre-commit validation.
Pass --timings to print elapsed seconds for each cargo command.
Pass --list to print the selected commands without running them.
Pass package names or package:filter pairs to override auto-detection.
USAGE
}

full=false
tests_only=false
parallel_tests=false
timings=false
list_only=false
declare -a requested=()
for arg in "$@"; do
  case "$arg" in
    --full)
      full=true
      ;;
    --tests-only)
      tests_only=true
      ;;
    --parallel-tests)
      parallel_tests=true
      ;;
    --timings)
      timings=true
      ;;
    --list)
      list_only=true
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      requested+=("$arg")
      ;;
  esac
done

run_cmd() {
  echo "+ $*"
  if [[ "$list_only" == true ]]; then
    return 0
  fi
  if [[ "$timings" == false ]]; then
    "$@"
    return
  fi

  local started_at="$SECONDS"
  set +e
  "$@"
  local status=$?
  set -e
  local elapsed=$((SECONDS - started_at))
  if ((status == 0)); then
    echo "ok (${elapsed}s): $*"
  else
    echo "failed (${elapsed}s): $*" >&2
  fi
  return "$status"
}

run_filtered_cargo_test() {
  local package="$1"
  local test_target="$2"
  local filter="$3"
  if [[ "$parallel_tests" == true ]]; then
    run_cmd cargo test -p "$package" --test "$test_target" --all-features "$filter"
  else
    run_cmd cargo test -p "$package" --test "$test_target" --all-features "$filter" -- --test-threads=1
  fi
}

run_filtered_package_test() {
  local package="$1"
  local filter="$2"
  if [[ "$parallel_tests" == true ]]; then
    run_cmd cargo test -p "$package" --all-features "$filter"
  else
    run_cmd cargo test -p "$package" --all-features "$filter" -- --test-threads=1
  fi
}

package_name_for_dir() {
  local crate_dir="$1"
  awk -F= '
    $1 ~ /^[[:space:]]*name[[:space:]]*$/ {
      gsub(/[[:space:]"]/, "", $2);
      print $2;
      exit;
    }
  ' "$crate_dir/Cargo.toml"
}

declare -a full_packages=()
declare -a lib_packages=()
declare -a filtered_specs=()
declare -a filtered_test_specs=()
declare -a lint_packages=()
declare -a lint_test_specs=()

has_item() {
  local needle="$1"
  shift
  local item
  for item in "$@"; do
    [[ "$item" == "$needle" ]] && return 0
  done
  return 1
}

add_full_package() {
  local package="$1"
  [[ -n "$package" ]] || return 0
  has_item "$package" "${full_packages[@]}" || full_packages+=("$package")
  add_lint_package "$package"
}

add_lib_package() {
  local package="$1"
  [[ -n "$package" ]] || return 0
  has_item "$package" "${lib_packages[@]}" || lib_packages+=("$package")
  add_lint_package "$package"
}

add_filtered_package() {
  local package="$1"
  local filter="$2"
  [[ -n "$package" && -n "$filter" ]] || return 0
  local spec="$package:$filter"
  has_item "$spec" "${filtered_specs[@]}" || filtered_specs+=("$spec")
  add_lint_package "$package"
}

add_filtered_test() {
  local package="$1"
  local test_target="$2"
  local filter="$3"
  [[ -n "$package" && -n "$test_target" && -n "$filter" ]] || return 0
  local spec="$package:$test_target:$filter"
  has_item "$spec" "${filtered_test_specs[@]}" || filtered_test_specs+=("$spec")
  add_lint_test "$package" "$test_target"
}

add_lint_test() {
  local package="$1"
  local test_target="$2"
  [[ -n "$package" && -n "$test_target" ]] || return 0
  has_item "$package:$test_target" "${lint_test_specs[@]}" || lint_test_specs+=("$package:$test_target")
}

add_lint_package() {
  local package="$1"
  [[ -n "$package" ]] || return 0
  has_item "$package" "${lint_packages[@]}" || lint_packages+=("$package")
}

add_package_spec() {
  local spec="$1"
  if [[ "$spec" == *:*:* ]]; then
    local package="${spec%%:*}"
    local rest="${spec#*:}"
    add_filtered_test "$package" "${rest%%:*}" "${rest#*:}"
  elif [[ "$spec" == *:* ]]; then
    add_filtered_package "${spec%%:*}" "${spec#*:}"
  else
    add_full_package "$spec"
  fi
}

if ((${#requested[@]})); then
  for spec in "${requested[@]}"; do
    add_package_spec "$spec"
  done
else
  while IFS= read -r path; do
    case "$path" in
      Cargo.toml|Cargo.lock)
        full=true
        ;;
      crates/*/*)
        crate_dir="${path%%/src/*}"
        crate_dir="${crate_dir%%/tests/*}"
        crate_dir="${crate_dir%%/benches/*}"
        if [[ -f "$crate_dir/Cargo.toml" ]]; then
          package="$(package_name_for_dir "$crate_dir")"
          case "$path" in
            crates/covy-cli/tests/artifact_e2e.rs)
              add_filtered_test "$package" "artifact_e2e" "test_"
              ;;
            crates/covy-cli/tests/artifact_pr_e2e.rs)
              add_filtered_test "$package" "artifact_pr_e2e" "pr"
              ;;
            crates/covy-cli/tests/e2e_test.rs)
              add_filtered_test "$package" "e2e_test" "test_"
              ;;
            crates/covy-cli/tests/check_e2e.rs)
              add_filtered_test "$package" "check_e2e" "check"
              ;;
            crates/covy-cli/tests/check_issues_e2e.rs)
              add_filtered_test "$package" "check_issues_e2e" "check"
              ;;
            crates/covy-cli/tests/check_input_e2e.rs)
              add_filtered_test "$package" "check_input_e2e" "check_input"
              ;;
            crates/covy-cli/tests/support/check.rs)
              add_filtered_test "$package" "check_e2e" "check"
              add_filtered_test "$package" "check_issues_e2e" "check"
              add_filtered_test "$package" "check_input_e2e" "check_input"
              ;;
            crates/covy-cli/tests/ingest_e2e.rs)
              add_filtered_test "$package" "ingest_e2e" "ingest"
              ;;
            crates/covy-cli/tests/ingest_legacy_e2e.rs)
              add_filtered_test "$package" "ingest_legacy_e2e" "ingest"
              ;;
            crates/covy-cli/tests/impact_e2e.rs)
              add_filtered_test "$package" "impact_e2e" "impact"
              ;;
            crates/covy-cli/tests/impact_legacy_e2e.rs)
              add_filtered_test "$package" "impact_legacy_e2e" "impact"
              ;;
            crates/covy-cli/tests/impact_schema_e2e.rs)
              add_filtered_test "$package" "impact_schema_e2e" "schema_flags"
              ;;
            crates/covy-cli/tests/support/impact.rs)
              add_filtered_test "$package" "impact_e2e" "impact"
              add_filtered_test "$package" "impact_legacy_e2e" "impact"
              ;;
            crates/covy-cli/tests/merge_e2e.rs)
              add_filtered_test "$package" "merge_e2e" "merge"
              ;;
            crates/covy-cli/tests/shard_e2e.rs)
              add_filtered_test "$package" "shard_e2e" "shard"
              ;;
            crates/covy-cli/tests/shard_update_e2e.rs)
              add_filtered_test "$package" "shard_update_e2e" "shard_update"
              ;;
            crates/covy-cli/tests/support/shard.rs)
              add_filtered_test "$package" "shard_e2e" "shard"
              add_filtered_test "$package" "shard_update_e2e" "shard_update"
              ;;
            crates/covy-cli/tests/testmap_e2e.rs)
              add_filtered_test "$package" "testmap_e2e" "testmap"
              ;;
            crates/packet28-search-cli/tests/e2e.rs)
              add_filtered_test "$package" "e2e" "_"
              ;;
            crates/packet28-search-cli/tests/fff_e2e.rs)
              add_filtered_test "$package" "fff_e2e" "fff"
              ;;
            crates/packet28-search-cli/tests/daemon_e2e.rs)
              add_filtered_test "$package" "daemon_e2e" "daemon"
              ;;
            crates/packet28-search-cli/tests/support/daemon.rs)
              add_filtered_test "$package" "daemon_e2e" "daemon"
              ;;
            crates/packet28-search-cli/tests/support/mod.rs)
              add_filtered_test "$package" "e2e" "_"
              add_filtered_test "$package" "fff_e2e" "fff"
              add_filtered_test "$package" "daemon_e2e" "daemon"
              ;;
            crates/suite-cli/src/cmd_verify.rs)
              add_filtered_package "$package" "verify"
              ;;
            crates/suite-cli/tests/verify_e2e.rs)
              add_filtered_test "$package" "verify_e2e" "verify"
              ;;
            crates/suite-cli/tests/verify_filters_e2e.rs)
              add_filtered_test "$package" "verify_filters_e2e" "verify_filters"
              ;;
            crates/suite-cli/tests/verify_handoffs_e2e.rs)
              add_filtered_test "$package" "verify_handoffs_e2e" "verify_handoffs"
              ;;
            crates/suite-cli/tests/support/verify.rs)
              add_filtered_test "$package" "verify_e2e" "verify"
              add_filtered_test "$package" "verify_filters_e2e" "verify_filters"
              add_filtered_test "$package" "verify_handoffs_e2e" "verify_handoffs"
              ;;
            crates/suite-cli/tests/setup_e2e.rs)
              add_filtered_test "$package" "setup_e2e" "test_"
              ;;
            crates/suite-cli/tests/setup_invalid_config_e2e.rs)
              add_filtered_test "$package" "setup_invalid_config_e2e" "setup_refuses"
              ;;
            crates/suite-cli/tests/setup_runtime_hooks_e2e.rs)
              add_filtered_test "$package" "setup_runtime_hooks_e2e" "setup_runtime_hooks"
              ;;
            crates/suite-cli/tests/setup_cursor_e2e.rs)
              add_filtered_test "$package" "setup_cursor_e2e" "setup_cursor"
              ;;
            crates/suite-cli/tests/setup_index_e2e.rs)
              add_filtered_test "$package" "setup_index_e2e" "setup_index"
              ;;
            crates/suite-cli/tests/setup_index_daemon_e2e.rs)
              add_filtered_test "$package" "setup_index_daemon_e2e" "setup_index_daemon"
              ;;
            crates/suite-cli/tests/support/setup_index.rs)
              add_filtered_test "$package" "setup_index_e2e" "setup_index"
              add_filtered_test "$package" "setup_index_daemon_e2e" "setup_index_daemon"
              ;;
            crates/suite-cli/tests/setup_windsurf_e2e.rs)
              add_filtered_test "$package" "setup_windsurf_e2e" "setup_windsurf"
              ;;
            crates/suite-cli/tests/setup_windsurf_mcp_e2e.rs)
              add_filtered_test "$package" "setup_windsurf_mcp_e2e" "setup_windsurf"
              ;;
            crates/suite-cli/tests/governed_workflow_e2e.rs)
              add_filtered_test "$package" "governed_workflow_e2e" "governed_workflow"
              ;;
            crates/suite-cli/tests/support/governed_workflow.rs)
              add_filtered_test "$package" "governed_workflow_e2e" "governed_workflow"
              ;;
            crates/suite-cli/tests/rewrite_e2e.rs)
              add_filtered_test "$package" "rewrite_e2e" "top_level_rewrite"
              ;;
            crates/suite-cli/tests/system_core_e2e.rs)
              add_filtered_test "$package" "system_core_e2e" "system_core"
              ;;
            crates/suite-cli/tests/system_wrapper_e2e.rs)
              add_filtered_test "$package" "system_wrapper_e2e" "system_wrapper"
              ;;
            crates/suite-cli/tests/system_infra_e2e.rs)
              add_filtered_test "$package" "system_infra_e2e" "system_infra"
              ;;
            crates/suite-cli/tests/system_build_e2e.rs)
              add_filtered_test "$package" "system_build_e2e" "system_build"
              ;;
            crates/suite-cli/tests/system_filter_e2e.rs)
              add_filtered_test "$package" "system_filter_e2e" "system_filter_backed"
              ;;
            crates/suite-cli/tests/system_filter_more_e2e.rs)
              add_filtered_test "$package" "system_filter_more_e2e" "system_more_filter_backed"
              ;;
            crates/suite-cli/tests/system_filter_remaining_e2e.rs)
              add_filtered_test "$package" "system_filter_remaining_e2e" "system_remaining_filter_backed"
              ;;
            crates/suite-cli/tests/system_query_e2e.rs)
              add_filtered_test "$package" "system_query_e2e" "system_query"
              ;;
            crates/suite-cli/tests/gain_e2e.rs)
              add_filtered_test "$package" "gain_e2e" "gain"
              ;;
            crates/suite-cli/tests/gain_failures_e2e.rs)
              add_filtered_test "$package" "gain_failures_e2e" "gain"
              ;;
            crates/suite-cli/tests/gain_formats_e2e.rs)
              add_filtered_test "$package" "gain_formats_e2e" "savings_formats"
              ;;
            crates/suite-cli/tests/gain_quota_graph_e2e.rs)
              add_filtered_test "$package" "gain_quota_graph_e2e" "gain_quota_graph"
              ;;
            crates/suite-cli/tests/support/gain.rs)
              add_filtered_test "$package" "gain_formats_e2e" "savings_formats"
              add_filtered_test "$package" "gain_quota_graph_e2e" "gain_quota_graph"
              ;;
            crates/suite-cli/tests/run_raw_artifact_e2e.rs)
              add_filtered_test "$package" "run_raw_artifact_e2e" "run_raw_artifact"
              ;;
            crates/suite-cli/tests/run_raw_artifact_families_e2e.rs)
              add_filtered_test "$package" "run_raw_artifact_families_e2e" "run_raw_artifact_families"
              ;;
            crates/suite-cli/tests/support/run_raw_artifact.rs)
              add_filtered_test "$package" "run_raw_artifact_e2e" "run_raw_artifact"
              add_filtered_test "$package" "run_raw_artifact_families_e2e" "run_raw_artifact_families"
              ;;
            crates/suite-cli/tests/run_filter_e2e.rs)
              add_filtered_test "$package" "run_filter_e2e" "run_filter"
              ;;
            crates/suite-cli/tests/run_filter_builtin_e2e.rs)
              add_filtered_test "$package" "run_filter_builtin_e2e" "run_filter"
              ;;
            crates/suite-cli/tests/run_reducer_e2e.rs)
              add_filtered_test "$package" "run_reducer_e2e" "run_reducer"
              ;;
            crates/suite-cli/tests/run_reducer_runtime_e2e.rs)
              add_filtered_test "$package" "run_reducer_runtime_e2e" "run_reducer_runtime"
              ;;
            crates/suite-cli/tests/run_reducer_runtime_languages_e2e.rs)
              add_filtered_test "$package" "run_reducer_runtime_languages_e2e" "run_reducer_runtime"
              ;;
            crates/suite-cli/tests/runtime_backend_e2e.rs)
              add_filtered_test "$package" "runtime_backend_e2e" "runtime_backend_cli"
              ;;
            crates/suite-cli/tests/runtime_backend_macos_e2e.rs)
              add_filtered_test "$package" "runtime_backend_macos_e2e" "runtime_backend_macos"
              ;;
            crates/suite-cli/tests/support/runtime_backend.rs)
              add_filtered_test "$package" "runtime_backend_macos_e2e" "runtime_backend_macos"
              ;;
            crates/suite-cli/tests/memory_pending_e2e.rs)
              add_filtered_test "$package" "memory_pending_e2e" "memory_pending"
              ;;
            crates/suite-cli/tests/memory_migration_e2e.rs)
              add_filtered_test "$package" "memory_migration_e2e" "memory_migration"
              ;;
            crates/suite-cli/tests/support/memory_migration.rs)
              add_filtered_test "$package" "memory_migration_e2e" "memory_migration"
              ;;
            crates/suite-cli/tests/support/memory_migration_assertions.rs)
              add_filtered_test "$package" "memory_migration_e2e" "memory_migration"
              ;;
            crates/suite-cli/tests/wakeup_scope_e2e.rs)
              add_filtered_test "$package" "wakeup_scope_e2e" "wakeup_scope"
              ;;
            crates/suite-cli/tests/memory_recall_e2e.rs)
              add_filtered_test "$package" "memory_recall_e2e" "memory_recall"
              ;;
            crates/suite-cli/tests/memory_consolidate_e2e.rs)
              add_filtered_test "$package" "memory_consolidate_e2e" "memory_consolidate"
              ;;
            crates/suite-cli/tests/memory_cli_e2e.rs)
              add_filtered_test "$package" "memory_cli_e2e" "memory_cli"
              ;;
            crates/suite-cli/tests/memory_cli_filter_e2e.rs)
              add_filtered_test "$package" "memory_cli_filter_e2e" "memory_cli"
              ;;
            crates/suite-cli/tests/memory_cli_embed_health_e2e.rs)
              add_filtered_test "$package" "memory_cli_embed_health_e2e" "memory_cli"
              ;;
            crates/suite-cli/tests/memory_maintenance_e2e.rs)
              add_filtered_test "$package" "memory_maintenance_e2e" "memory_maintenance"
              ;;
            crates/suite-cli/tests/mcp_memory_e2e.rs)
              add_filtered_test "$package" "mcp_memory_e2e" "mcp_memory"
              ;;
            crates/suite-cli/tests/support/mcp_memory.rs)
              add_filtered_test "$package" "mcp_memory_e2e" "mcp_memory"
              ;;
            crates/suite-cli/tests/mcp_memory_maintenance_e2e.rs)
              add_filtered_test "$package" "mcp_memory_maintenance_e2e" "mcp_memory_maintenance"
              ;;
            crates/suite-cli/tests/mcp_context_e2e.rs)
              add_filtered_test "$package" "mcp_context_e2e" "mcp_context"
              ;;
            crates/suite-cli/tests/mcp_context_transcript_e2e.rs)
              add_filtered_test "$package" "mcp_context_transcript_e2e" "mcp_context_transcript"
              ;;
            crates/suite-cli/tests/support/mcp_context_transcript.rs)
              add_filtered_test "$package" "mcp_context_transcript_e2e" "mcp_context_transcript"
              ;;
            crates/suite-cli/tests/mcp_context_learn_e2e.rs)
              add_filtered_test "$package" "mcp_context_learn_e2e" "mcp_context_learn"
              ;;
            crates/suite-cli/tests/mcp_graph_e2e.rs)
              add_filtered_test "$package" "mcp_graph_e2e" "mcp_graph"
              ;;
            crates/suite-cli/tests/mcp_graph_inspect_e2e.rs)
              add_filtered_test "$package" "mcp_graph_inspect_e2e" "mcp_graph_inspect"
              ;;
            crates/suite-cli/tests/mcp_graph_distill_e2e.rs)
              add_filtered_test "$package" "mcp_graph_distill_e2e" "mcp_graph_distill"
              ;;
            crates/suite-cli/tests/mcp_memory_pending_e2e.rs)
              add_filtered_test "$package" "mcp_memory_pending_e2e" "mcp_memory_pending"
              ;;
            crates/suite-cli/tests/support/mod.rs|crates/suite-cli/tests/support/mcp.rs)
              add_filtered_test "$package" "mcp_memory_e2e" "mcp_memory"
              add_filtered_test "$package" "mcp_memory_maintenance_e2e" "mcp_memory_maintenance"
              add_filtered_test "$package" "mcp_context_e2e" "mcp_context"
              add_filtered_test "$package" "mcp_context_transcript_e2e" "mcp_context_transcript"
              add_filtered_test "$package" "mcp_context_learn_e2e" "mcp_context_learn"
              add_filtered_test "$package" "mcp_graph_e2e" "mcp_graph"
              add_filtered_test "$package" "mcp_graph_inspect_e2e" "mcp_graph_inspect"
              add_filtered_test "$package" "mcp_graph_distill_e2e" "mcp_graph_distill"
              add_filtered_test "$package" "mcp_memory_pending_e2e" "mcp_memory_pending"
              add_filtered_test "$package" "setup_index_e2e" "setup_index"
              ;;
            crates/suite-cli/tests/mcp_handoff_e2e.rs)
              add_filtered_test "$package" "mcp_handoff_e2e" "mcp_handoff"
              ;;
            crates/suite-cli/tests/support/mcp_handoff.rs)
              add_filtered_test "$package" "mcp_handoff_e2e" "mcp_handoff"
              ;;
            crates/suite-cli/tests/mcp_native_e2e.rs)
              add_filtered_test "$package" "mcp_native_e2e" "mcp_native"
              ;;
            crates/suite-cli/tests/mcp_native_artifact_e2e.rs)
              add_filtered_test "$package" "mcp_native_artifact_e2e" "mcp_native_artifact"
              ;;
            crates/suite-cli/tests/mcp_native_read_e2e.rs)
              add_filtered_test "$package" "mcp_native_read_e2e" "mcp_native_read"
              ;;
            crates/suite-cli/tests/support/mcp_native.rs)
              add_filtered_test "$package" "mcp_native_artifact_e2e" "mcp_native_artifact"
              add_filtered_test "$package" "mcp_native_read_e2e" "mcp_native_read"
              ;;
            crates/suite-cli/tests/mcp_native_stdio_e2e.rs)
              add_filtered_test "$package" "mcp_native_stdio_e2e" "mcp_native_stdio"
              ;;
            crates/suite-cli/tests/feedback_cli_e2e.rs)
              add_filtered_test "$package" "feedback_cli_e2e" "feedback_cli"
              ;;
            crates/suite-cli/tests/feedback_graph_e2e.rs)
              add_filtered_test "$package" "feedback_graph_e2e" "feedback_graph"
              ;;
            crates/suite-cli/tests/feedback_graph_learn_e2e.rs)
              add_filtered_test "$package" "feedback_graph_learn_e2e" "feedback_graph_learn"
              ;;
            crates/suite-cli/tests/feedback_graph_distill_e2e.rs)
              add_filtered_test "$package" "feedback_graph_distill_e2e" "feedback_graph_distill"
              ;;
            crates/suite-cli/tests/feedback_graph_transcript_e2e.rs)
              add_filtered_test "$package" "feedback_graph_transcript_e2e" "feedback_graph_transcript"
              ;;
            crates/suite-cli/tests/support/feedback_graph.rs)
              add_filtered_test "$package" "feedback_cli_e2e" "feedback_cli"
              add_filtered_test "$package" "feedback_graph_e2e" "feedback_graph"
              add_filtered_test "$package" "feedback_graph_learn_e2e" "feedback_graph_learn"
              add_filtered_test "$package" "feedback_graph_distill_e2e" "feedback_graph_distill"
              add_filtered_test "$package" "feedback_graph_transcript_e2e" "feedback_graph_transcript"
              ;;
            crates/suite-cli/tests/transcript_e2e.rs)
              add_filtered_test "$package" "transcript_e2e" "transcript_round_trip"
              ;;
            crates/suite-cli/tests/dashboard_e2e.rs)
              add_filtered_test "$package" "dashboard_e2e" "dashboard_local"
              ;;
            crates/suite-cli/tests/support/dashboard.rs)
              add_filtered_test "$package" "dashboard_e2e" "dashboard_local"
              ;;
            crates/suite-cli/tests/doctor_e2e.rs)
              add_filtered_test "$package" "doctor_e2e" "doctor_cli"
              ;;
            crates/suite-cli/tests/agent_cli_e2e.rs)
              add_filtered_test "$package" "agent_cli_e2e" "agent_cli"
              ;;
            crates/suite-cli/tests/agent_handoff_e2e.rs|crates/suite-cli/tests/support/mcp_io.rs)
              add_filtered_test "$package" "agent_handoff_e2e" "agent_handoff"
              ;;
            crates/suite-cli/tests/support/agent.rs)
              add_filtered_test "$package" "agent_cli_e2e" "agent_cli"
              ;;
            crates/suite-cli/tests/support/agent_core.rs)
              add_filtered_test "$package" "agent_cli_e2e" "agent_cli"
              add_filtered_test "$package" "agent_handoff_e2e" "agent_handoff"
              ;;
            crates/suite-cli/tests/daemon_lifecycle_e2e.rs)
              add_filtered_test "$package" "daemon_lifecycle_e2e" "daemon_lifecycle_cli"
              ;;
            crates/suite-cli/tests/daemon_lifecycle_disconnect_e2e.rs)
              add_filtered_test "$package" "daemon_lifecycle_disconnect_e2e" "daemon_lifecycle_cli"
              ;;
            crates/suite-cli/tests/support/daemon_lifecycle.rs)
              add_filtered_test "$package" "daemon_lifecycle_e2e" "daemon_lifecycle_cli"
              add_filtered_test "$package" "daemon_lifecycle_disconnect_e2e" "daemon_lifecycle_cli"
              ;;
            crates/suite-cli/tests/daemon_task_e2e.rs)
              add_filtered_test "$package" "daemon_task_e2e" "daemon_task_cli"
              ;;
            crates/suite-cli/tests/daemon_task_launch_e2e.rs)
              add_filtered_test "$package" "daemon_task_launch_e2e" "daemon_task_cli"
              ;;
            crates/suite-cli/tests/support/daemon_task.rs|crates/suite-cli/tests/support/daemon_task_core.rs|crates/suite-cli/tests/support/daemon_task_mcp.rs|crates/suite-cli/tests/support/daemon_task_seed.rs)
              add_filtered_test "$package" "daemon_task_e2e" "daemon_task_cli"
              add_filtered_test "$package" "daemon_task_launch_e2e" "daemon_task_cli"
              ;;
            crates/suite-cli/tests/support/daemon_task_submit.rs)
              add_filtered_test "$package" "daemon_task_submit_e2e" "daemon_task_submit"
              add_filtered_test "$package" "daemon_task_submit_failure_e2e" "failed_submit"
              add_filtered_test "$package" "daemon_task_submit_normalize_e2e" "daemon_task_submit_normalize"
              ;;
            crates/suite-cli/tests/support/daemon_task_submit_map.rs)
              add_filtered_test "$package" "daemon_task_submit_e2e" "daemon_task_submit"
              add_filtered_test "$package" "daemon_task_submit_normalize_e2e" "daemon_task_submit_normalize"
              ;;
            crates/suite-cli/tests/daemon_task_submit_failure_e2e.rs)
              add_filtered_test "$package" "daemon_task_submit_failure_e2e" "failed_submit"
              ;;
            crates/suite-cli/tests/daemon_task_submit_normalize_e2e.rs)
              add_filtered_test "$package" "daemon_task_submit_normalize_e2e" "daemon_task_submit_normalize"
              ;;
            crates/suite-cli/tests/daemon_task_submit_e2e.rs)
              add_filtered_test "$package" "daemon_task_submit_e2e" "daemon_task_submit"
              ;;
            crates/suite-cli/tests/hypothesis_cli_e2e.rs)
              add_filtered_test "$package" "hypothesis_cli_e2e" "hypothesis_cli"
              ;;
            crates/suite-cli/tests/hypothesis_mcp_e2e.rs)
              add_filtered_test "$package" "hypothesis_mcp_e2e" "hypothesis_mcp"
              ;;
            crates/suite-cli/tests/support/hypothesis.rs)
              add_filtered_test "$package" "hypothesis_cli_e2e" "hypothesis_cli"
              add_filtered_test "$package" "hypothesis_mcp_e2e" "hypothesis_mcp"
              ;;
            crates/suite-cli/tests/discover_e2e.rs)
              add_filtered_test "$package" "discover_e2e" "discover_"
              ;;
            crates/suite-cli/tests/discover_opportunities_e2e.rs)
              add_filtered_test "$package" "discover_opportunities_e2e" "discover_"
              ;;
            crates/suite-cli/tests/support/discover.rs)
              add_filtered_test "$package" "discover_e2e" "discover_"
              add_filtered_test "$package" "discover_opportunities_e2e" "discover_"
              ;;
            crates/suite-cli/tests/discover_sessions_e2e.rs)
              add_filtered_test "$package" "discover_sessions_e2e" "discover_sessions"
              ;;
            crates/suite-cli/tests/session_e2e.rs)
              add_filtered_test "$package" "session_e2e" "session_cli"
              ;;
            crates/suite-cli/tests/learn_e2e.rs)
              add_filtered_test "$package" "learn_e2e" "learn_cli"
              ;;
            crates/suite-cli/tests/hook_telemetry_e2e.rs)
              add_filtered_test "$package" "hook_telemetry_e2e" "hook_telemetry"
              ;;
            crates/suite-cli/tests/hook_telemetry_session_e2e.rs)
              add_filtered_test "$package" "hook_telemetry_session_e2e" "hook_telemetry_session"
              ;;
            crates/suite-cli/tests/support/hook_telemetry.rs)
              add_filtered_test "$package" "hook_telemetry_e2e" "hook_telemetry"
              add_filtered_test "$package" "hook_telemetry_session_e2e" "hook_telemetry_session"
              ;;
            crates/suite-cli/tests/hook_rewrite_e2e.rs)
              add_filtered_test "$package" "hook_rewrite_e2e" "hook_rewrite_cli"
              ;;
            crates/suite-cli/tests/hook_rewrite_runtimes_e2e.rs)
              add_filtered_test "$package" "hook_rewrite_runtimes_e2e" "hook_rewrite_runtimes"
              ;;
            crates/suite-cli/tests/support/hook_rewrite.rs)
              add_filtered_test "$package" "hook_rewrite_e2e" "hook_rewrite_cli"
              add_filtered_test "$package" "hook_rewrite_runtimes_e2e" "hook_rewrite_runtimes"
              ;;
            crates/suite-cli/tests/hook_runner_e2e.rs)
              add_filtered_test "$package" "hook_runner_e2e" "hook_runner_cli"
              ;;
            crates/suite-cli/tests/cover_e2e.rs)
              add_filtered_test "$package" "cover_e2e" "cover_cli"
              ;;
            crates/suite-cli/tests/diff_analyze_e2e.rs)
              add_filtered_test "$package" "diff_analyze_e2e" "diff_analyze_cli"
              ;;
            crates/suite-cli/tests/diff_analyze_task_focus_e2e.rs)
              add_filtered_test "$package" "diff_analyze_task_focus_e2e" "diff_analyze_cli_task_id"
              ;;
            crates/suite-cli/tests/diff_analyze_governed_e2e.rs)
              add_filtered_test "$package" "diff_analyze_governed_e2e" "diff_analyze_governed"
              ;;
            crates/suite-cli/tests/support/diff_analyze.rs)
              add_filtered_test "$package" "diff_analyze_e2e" "diff_analyze_cli"
              add_filtered_test "$package" "diff_analyze_task_focus_e2e" "diff_analyze_cli_task_id"
              ;;
            crates/suite-cli/tests/test_impact_e2e.rs)
              add_filtered_test "$package" "test_impact_e2e" "test_impact_cli"
              ;;
            crates/suite-cli/tests/via_daemon_e2e.rs)
              add_filtered_test "$package" "via_daemon_e2e" "via_daemon_cli"
              ;;
            crates/suite-cli/tests/support/via_daemon.rs)
              add_filtered_test "$package" "via_daemon_e2e" "via_daemon_cli"
              ;;
            crates/suite-cli/tests/support/packet_wrapper.rs)
              add_filtered_test "$package" "via_daemon_e2e" "via_daemon_cli"
              ;;
            crates/suite-cli/tests/via_daemon_root_e2e.rs)
              add_filtered_test "$package" "via_daemon_root_e2e" "via_daemon_root"
              ;;
            crates/suite-cli/tests/via_daemon_test_e2e.rs)
              add_filtered_test "$package" "via_daemon_test_e2e" "via_daemon_test"
              ;;
            crates/suite-cli/tests/guard_e2e.rs)
              add_filtered_test "$package" "guard_e2e" "guard_cli"
              ;;
            crates/suite-cli/tests/guard_validate_e2e.rs)
              add_filtered_test "$package" "guard_validate_e2e" "guard_cli"
              ;;
            crates/suite-cli/tests/context_assemble_e2e.rs)
              add_filtered_test "$package" "context_assemble_e2e" "context_assemble_cli"
              ;;
            crates/suite-cli/tests/support/context_assemble.rs)
              add_filtered_test "$package" "context_assemble_e2e" "context_assemble_cli"
              ;;
            crates/suite-cli/tests/context_recall_daemon_e2e.rs)
              add_filtered_test "$package" "context_recall_daemon_e2e" "context_recall_daemon_cli"
              ;;
            crates/suite-cli/tests/context_via_daemon_e2e.rs)
              add_filtered_test "$package" "context_via_daemon_e2e" "context_via_daemon_cli"
              ;;
            crates/suite-cli/tests/context_via_daemon_store_e2e.rs)
              add_filtered_test "$package" "context_via_daemon_store_e2e" "context_via_daemon_cli"
              ;;
            crates/suite-cli/tests/support/context_packet.rs)
              add_filtered_test "$package" "context_state_e2e" "context_state_cli"
              add_filtered_test "$package" "context_via_daemon_e2e" "context_via_daemon_cli"
              ;;
            crates/suite-cli/tests/support/context_daemon_core.rs)
              add_filtered_test "$package" "context_via_daemon_e2e" "context_via_daemon_cli"
              add_filtered_test "$package" "context_via_daemon_store_e2e" "context_via_daemon_cli"
              ;;
            crates/suite-cli/tests/support/context_daemon.rs)
              add_filtered_test "$package" "context_recall_daemon_e2e" "context_recall_daemon_cli"
              ;;
            crates/suite-cli/tests/context_state_e2e.rs)
              add_filtered_test "$package" "context_state_e2e" "context_state_cli"
              ;;
            crates/suite-cli/tests/context_store_state_e2e.rs)
              add_filtered_test "$package" "context_store_state_e2e" "context_store_state_cli"
              ;;
            crates/suite-cli/tests/context_store_recall_e2e.rs)
              add_filtered_test "$package" "context_store_recall_e2e" "context_store_state_cli"
              ;;
            crates/suite-cli/tests/context_correlate_e2e.rs)
              add_filtered_test "$package" "context_correlate_e2e" "context_correlate_cli"
              ;;
            crates/suite-cli/tests/stack_build_e2e.rs)
              add_filtered_test "$package" "stack_build_e2e" "stack_build_cli"
              ;;
            crates/suite-cli/tests/stack_build_via_daemon_e2e.rs)
              add_filtered_test "$package" "stack_build_via_daemon_e2e" "stack_build_cli_via_daemon"
              ;;
            crates/suite-cli/tests/support/stack_build.rs)
              add_filtered_test "$package" "stack_build_e2e" "stack_build_cli"
              add_filtered_test "$package" "stack_build_via_daemon_e2e" "stack_build_cli_via_daemon"
              ;;
            crates/suite-cli/tests/support/stack_build_daemon.rs)
              add_filtered_test "$package" "stack_build_via_daemon_e2e" "stack_build_cli_via_daemon"
              ;;
            crates/suite-cli/tests/map_proxy_e2e.rs)
              add_filtered_test "$package" "map_proxy_e2e" "map_proxy_cli"
              ;;
            crates/suite-cli/tests/map_proxy_formats_e2e.rs)
              add_filtered_test "$package" "map_proxy_formats_e2e" "map_proxy_cli"
              ;;
            crates/suite-cli/tests/map_proxy_governed_e2e.rs)
              add_filtered_test "$package" "map_proxy_governed_e2e" "map_proxy_governed"
              ;;
            crates/suite-cli/tests/map_proxy_cache_e2e.rs)
              add_filtered_test "$package" "map_proxy_cache_e2e" "map_proxy_cache"
              ;;
            crates/suite-cli/tests/map_proxy_profiles_e2e.rs)
              add_filtered_test "$package" "map_proxy_profiles_e2e" "map_proxy_profiles"
              ;;
            crates/suite-cli/tests/map_proxy_profiles_proxy_e2e.rs)
              add_filtered_test "$package" "map_proxy_profiles_proxy_e2e" "map_proxy_profiles_proxy"
              ;;
            crates/suite-cli/tests/support/map_proxy.rs)
              add_filtered_test "$package" "map_proxy_e2e" "map_proxy_cli"
              add_filtered_test "$package" "map_proxy_cache_e2e" "map_proxy_cache"
              add_filtered_test "$package" "map_proxy_formats_e2e" "map_proxy_cli"
              add_filtered_test "$package" "map_proxy_profiles_e2e" "map_proxy_profiles"
              add_filtered_test "$package" "map_proxy_profiles_proxy_e2e" "map_proxy_profiles_proxy"
              ;;
            crates/suite-cli/tests/support/map_proxy_repo.rs)
              add_filtered_test "$package" "map_proxy_e2e" "map_proxy_cli"
              add_filtered_test "$package" "map_proxy_formats_e2e" "map_proxy_cli"
              add_filtered_test "$package" "map_proxy_profiles_e2e" "map_proxy_profiles"
              ;;
            crates/suite-cli/tests/support/map_proxy_packet.rs)
              add_filtered_test "$package" "map_proxy_e2e" "map_proxy_cli"
              add_filtered_test "$package" "map_proxy_profiles_e2e" "map_proxy_profiles"
              add_filtered_test "$package" "map_proxy_profiles_proxy_e2e" "map_proxy_profiles_proxy"
              ;;
            crates/suite-cli/tests/support/map_proxy_payload.rs)
              add_filtered_test "$package" "map_proxy_profiles_e2e" "map_proxy_profiles"
              add_filtered_test "$package" "map_proxy_profiles_proxy_e2e" "map_proxy_profiles_proxy"
              ;;
            crates/suite-cli/tests/mcp_proxy_cache_e2e.rs)
              add_filtered_test "$package" "mcp_proxy_cache_e2e" "mcp_proxy_cache"
              ;;
            crates/suite-cli/tests/mcp_proxy_e2e.rs)
              add_filtered_test "$package" "mcp_proxy_e2e" "mcp_proxy_cli"
              ;;
            crates/suite-cli/tests/support/mcp_proxy.rs)
              add_filtered_test "$package" "mcp_proxy_cache_e2e" "mcp_proxy_cache"
              add_filtered_test "$package" "mcp_proxy_e2e" "mcp_proxy_cli"
              ;;
            crates/suite-cli/tests/support/mcp_proxy_fake.rs)
              add_filtered_test "$package" "mcp_proxy_e2e" "mcp_proxy_cli"
              ;;
            crates/suite-cli/src/cmd_wakeup.rs)
              add_filtered_package "$package" "test_wakeup_scopes_context_by_path_symbol_and_intent"
              ;;
            crates/suite-cli/src/cmd_mcp.rs|crates/suite-cli/src/cmd_mcp_native.rs)
              add_filtered_package "$package" "mcp"
              ;;
            crates/packet28-search-core/src/lib.rs)
              add_lib_package "$package"
              ;;
            crates/packet28d/src/broker_*.rs|crates/packet28d/src/tests.rs)
              add_filtered_package "$package" "broker"
              ;;
            *)
              add_full_package "$package"
              ;;
          esac
        fi
        ;;
    esac
  done < <(
    git diff --name-only HEAD -- Cargo.toml Cargo.lock crates
    git ls-files --others --exclude-standard -- Cargo.toml Cargo.lock crates
  )
fi

if [[ "$tests_only" == false ]]; then
  run_cmd cargo fmt --check
else
  echo "Skipping cargo fmt because --tests-only was provided."
fi

if [[ "$tests_only" == true ]]; then
  echo "Skipping cargo clippy because --tests-only was provided."
else
  if [[ "$full" == true ]]; then
    run_cmd cargo clippy --all-targets --all-features -- -D warnings
  elif ((${#lint_packages[@]})); then
    for package in "${lint_packages[@]}"; do
      run_cmd cargo clippy -p "$package" --all-targets --all-features -- -D warnings
    done
    for spec in "${lint_test_specs[@]}"; do
      package="${spec%%:*}"
      test_target="${spec#*:}"
      if has_item "$package" "${lint_packages[@]}"; then
        continue
      fi
      run_cmd cargo clippy -p "$package" --test "$test_target" --all-features -- -D warnings
    done
  elif ((${#lint_test_specs[@]})); then
    for spec in "${lint_test_specs[@]}"; do
      package="${spec%%:*}"
      test_target="${spec#*:}"
      run_cmd cargo clippy -p "$package" --test "$test_target" --all-features -- -D warnings
    done
  else
    echo "No Rust package changes detected; skipped cargo clippy."
  fi
fi

for spec in "${filtered_test_specs[@]}"; do
  package="${spec%%:*}"
  rest="${spec#*:}"
  test_target="${rest%%:*}"
  filter="${rest#*:}"
  if has_item "$package" "${full_packages[@]}" || has_item "$package" "${lib_packages[@]}"; then
    continue
  fi
  run_filtered_cargo_test "$package" "$test_target" "$filter"
done

for spec in "${filtered_specs[@]}"; do
  package="${spec%%:*}"
  filter="${spec#*:}"
  if has_item "$package" "${full_packages[@]}" || has_item "$package" "${lib_packages[@]}"; then
    continue
  fi
  run_filtered_package_test "$package" "$filter"
done

for package in "${lib_packages[@]}"; do
  if has_item "$package" "${full_packages[@]}"; then
    continue
  fi
  run_cmd cargo test -p "$package" --all-features --lib
done

for package in "${full_packages[@]}"; do
  run_cmd cargo test -p "$package" --all-features
done

if [[ "$full" == true ]]; then
  run_cmd cargo test --all-features
elif ((
  ${#filtered_test_specs[@]} == 0 &&
  ${#filtered_specs[@]} == 0 &&
  ${#lib_packages[@]} == 0 &&
  ${#full_packages[@]} == 0
)); then
  echo "No Rust package changes detected; skipped cargo test."
fi
