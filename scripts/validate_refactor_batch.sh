#!/usr/bin/env bash
set -eo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/validate_refactor_batch.sh [--full] [package[:filter] ...]

Runs the fast validation gate for an incremental refactor batch:
  - cargo fmt --check
  - cargo clippy --all-targets --all-features -- -D warnings
  - targeted cargo test commands for changed Rust packages

Pass --full to run cargo test --all-features after the targeted tests.
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
}

add_lib_package() {
  local package="$1"
  [[ -n "$package" ]] || return 0
  has_item "$package" "${lib_packages[@]}" || lib_packages+=("$package")
}

add_filtered_package() {
  local package="$1"
  local filter="$2"
  [[ -n "$package" && -n "$filter" ]] || return 0
  local spec="$package:$filter"
  has_item "$spec" "${filtered_specs[@]}" || filtered_specs+=("$spec")
}

add_package_spec() {
  local spec="$1"
  if [[ "$spec" == *:* ]]; then
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
              add_filtered_package "$package" "verify"
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
  done < <(git diff --name-only HEAD -- Cargo.toml Cargo.lock crates)
fi

echo "+ cargo fmt --check"
cargo fmt --check

echo "+ cargo clippy --all-targets --all-features -- -D warnings"
cargo clippy --all-targets --all-features -- -D warnings

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
elif ((${#filtered_specs[@]} == 0 && ${#lib_packages[@]} == 0 && ${#full_packages[@]} == 0)); then
  echo "No Rust package changes detected; skipped cargo test."
fi
