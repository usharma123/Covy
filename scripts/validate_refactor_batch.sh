#!/usr/bin/env bash
set -eo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/validate_refactor_batch.sh [--full] [package[:filter] ...] [package:test-target:filter ...]

Runs the fast validation gate for an incremental refactor batch:
  - cargo fmt --check
  - package-scoped cargo clippy for changed Rust packages
  - targeted cargo test commands for changed Rust packages

Pass --full to run workspace clippy and cargo test --all-features after the
targeted package tests.
Pass package names or package:filter pairs to override auto-detection.
USAGE
}

full=false
declare -a requested=()
for arg in "$@"; do
  case "$arg" in
    --full)
      full=true
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
            crates/suite-cli/src/cmd_verify.rs)
              add_filtered_package "$package" "verify"
              ;;
            crates/suite-cli/tests/verify_e2e.rs)
              add_filtered_test "$package" "verify_e2e" "verify"
              ;;
            crates/suite-cli/tests/e2e_smoke.rs)
              add_lint_test "$package" "e2e_smoke"
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
            crates/suite-cli/tests/run_raw_artifact_e2e.rs)
              add_filtered_test "$package" "run_raw_artifact_e2e" "run_raw_artifact"
              ;;
            crates/suite-cli/tests/run_filter_e2e.rs)
              add_filtered_test "$package" "run_filter_e2e" "run_filter"
              ;;
            crates/suite-cli/tests/run_reducer_e2e.rs)
              add_filtered_test "$package" "run_reducer_e2e" "run_reducer"
              ;;
            crates/suite-cli/tests/runtime_backend_e2e.rs)
              add_filtered_test "$package" "runtime_backend_e2e" "runtime_backend_cli"
              ;;
            crates/suite-cli/tests/memory_pending_e2e.rs)
              add_filtered_test "$package" "memory_pending_e2e" "memory_pending"
              ;;
            crates/suite-cli/tests/memory_migration_e2e.rs)
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
            crates/suite-cli/tests/feedback_graph_e2e.rs)
              add_filtered_test "$package" "feedback_graph_e2e" "feedback_graph"
              ;;
            crates/suite-cli/tests/transcript_e2e.rs)
              add_filtered_test "$package" "transcript_e2e" "transcript_round_trip"
              ;;
            crates/suite-cli/tests/dashboard_e2e.rs)
              add_filtered_test "$package" "dashboard_e2e" "dashboard_local"
              ;;
            crates/suite-cli/tests/agent_cli_e2e.rs)
              add_filtered_test "$package" "agent_cli_e2e" "agent_cli"
              ;;
            crates/suite-cli/tests/daemon_lifecycle_e2e.rs)
              add_filtered_test "$package" "daemon_lifecycle_e2e" "daemon_lifecycle_cli"
              ;;
            crates/suite-cli/tests/hypothesis_cli_e2e.rs)
              add_filtered_test "$package" "hypothesis_cli_e2e" "hypothesis_cli"
              ;;
            crates/suite-cli/tests/discover_e2e.rs)
              add_filtered_test "$package" "discover_e2e" "discover_"
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
            crates/suite-cli/tests/cover_e2e.rs)
              add_filtered_test "$package" "cover_e2e" "cover_cli"
              ;;
            crates/suite-cli/tests/diff_analyze_e2e.rs)
              add_filtered_test "$package" "diff_analyze_e2e" "diff_analyze_cli"
              ;;
            crates/suite-cli/tests/test_impact_e2e.rs)
              add_filtered_test "$package" "test_impact_e2e" "test_impact_cli"
              ;;
            crates/suite-cli/tests/guard_e2e.rs)
              add_filtered_test "$package" "guard_e2e" "guard_cli"
              ;;
            crates/suite-cli/tests/context_assemble_e2e.rs)
              add_filtered_test "$package" "context_assemble_e2e" "context_assemble_cli"
              ;;
            crates/suite-cli/tests/context_store_state_e2e.rs)
              add_filtered_test "$package" "context_store_state_e2e" "context_store_state_cli"
              ;;
            crates/suite-cli/tests/context_correlate_e2e.rs)
              add_filtered_test "$package" "context_correlate_e2e" "context_correlate_cli"
              ;;
            crates/suite-cli/tests/stack_build_e2e.rs)
              add_filtered_test "$package" "stack_build_e2e" "stack_build_cli"
              ;;
            crates/suite-cli/tests/map_proxy_e2e.rs)
              add_filtered_test "$package" "map_proxy_e2e" "map_proxy_cli"
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

echo "+ cargo fmt --check"
cargo fmt --check

if [[ "$full" == true ]]; then
  echo "+ cargo clippy --all-targets --all-features -- -D warnings"
  cargo clippy --all-targets --all-features -- -D warnings
elif ((${#lint_packages[@]})); then
  for package in "${lint_packages[@]}"; do
    echo "+ cargo clippy -p $package --all-targets --all-features -- -D warnings"
    cargo clippy -p "$package" --all-targets --all-features -- -D warnings
  done
  for spec in "${lint_test_specs[@]}"; do
    package="${spec%%:*}"
    test_target="${spec#*:}"
    echo "+ cargo clippy -p $package --test $test_target --all-features -- -D warnings"
    cargo clippy -p "$package" --test "$test_target" --all-features -- -D warnings
  done
elif ((${#lint_test_specs[@]})); then
  for spec in "${lint_test_specs[@]}"; do
    package="${spec%%:*}"
    test_target="${spec#*:}"
    echo "+ cargo clippy -p $package --test $test_target --all-features -- -D warnings"
    cargo clippy -p "$package" --test "$test_target" --all-features -- -D warnings
  done
else
  echo "No Rust package changes detected; skipped cargo clippy."
fi

for spec in "${filtered_test_specs[@]}"; do
  package="${spec%%:*}"
  rest="${spec#*:}"
  test_target="${rest%%:*}"
  filter="${rest#*:}"
  if has_item "$package" "${full_packages[@]}" || has_item "$package" "${lib_packages[@]}"; then
    continue
  fi
  echo "+ cargo test -p $package --test $test_target --all-features $filter -- --test-threads=1"
  cargo test -p "$package" --test "$test_target" --all-features "$filter" -- --test-threads=1
done

for spec in "${filtered_specs[@]}"; do
  package="${spec%%:*}"
  filter="${spec#*:}"
  if has_item "$package" "${full_packages[@]}" || has_item "$package" "${lib_packages[@]}"; then
    continue
  fi
  echo "+ cargo test -p $package --all-features $filter -- --test-threads=1"
  cargo test -p "$package" --all-features "$filter" -- --test-threads=1
done

for package in "${lib_packages[@]}"; do
  if has_item "$package" "${full_packages[@]}"; then
    continue
  fi
  echo "+ cargo test -p $package --all-features --lib"
  cargo test -p "$package" --all-features --lib
done

for package in "${full_packages[@]}"; do
  echo "+ cargo test -p $package --all-features"
  cargo test -p "$package" --all-features
done

if [[ "$full" == true ]]; then
  echo "+ cargo test --all-features"
  cargo test --all-features
elif ((
  ${#filtered_test_specs[@]} == 0 &&
  ${#filtered_specs[@]} == 0 &&
  ${#lib_packages[@]} == 0 &&
  ${#full_packages[@]} == 0
)); then
  echo "No Rust package changes detected; skipped cargo test."
fi
