#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

fail() {
  echo "workspace policy invariant failed: $*" >&2
  exit 1
}

[[ -f Cargo.lock ]] || fail "Cargo.lock is missing"
if git check-ignore --quiet --no-index Cargo.lock; then
  fail "Cargo.lock is ignored"
fi
git ls-files --error-unmatch -- Cargo.lock >/dev/null 2>&1 ||
  fail "Cargo.lock is not tracked"

grep -Fqx 'rust-version = "1.88.0"' Cargo.toml ||
  fail 'workspace rust-version must be exactly "1.88.0"'
grep -Fqx '[workspace.lints.rust]' Cargo.toml ||
  fail "workspace Rust lint policy is missing"
grep -Fqx '[workspace.lints.clippy]' Cargo.toml ||
  fail "workspace Clippy lint policy is missing"

internal_dependencies=(
  buildy-core
  context-kernel-core
  context-memory-core
  context-scheduler-core
  contextq-core
  covy-core
  covy-ingest
  diffy-core
  guardy-core
  mapy-core
  packet28-daemon-core
  packet28-reducer-core
  packet28-search-core
  stacky-core
  suite-foundation-core
  suite-ingest
  suite-packet-core
  suite-policy-core
  suite-proxy-core
  testy-cli-common
  testy-core
)

for dependency in "${internal_dependencies[@]}"; do
  expected="$dependency = { version = \"0.2.0\", path = \"crates/$dependency\" }"
  grep -Fqx "$expected" Cargo.toml ||
    fail "$dependency is not centralized in workspace dependencies"
done

if grep -En 'path[[:space:]]*=[[:space:]]*"\.\./' crates/*/Cargo.toml; then
  fail "member manifests must inherit internal workspace dependencies"
fi

member_count=0
for manifest in crates/*/Cargo.toml; do
  member_count=$((member_count + 1))
  grep -Fqx 'rust-version.workspace = true' "$manifest" ||
    fail "$manifest does not inherit the workspace MSRV"

  lint_table_count="$(grep -Ec '^\[lints\]$' "$manifest" || true)"
  [[ "$lint_table_count" -eq 1 ]] ||
    fail "$manifest must contain exactly one [lints] table"
  awk '
    $0 == "[lints]" {
      in_lints = 1
      next
    }
    /^\[/ {
      in_lints = 0
    }
    in_lints && $0 == "workspace = true" {
      inherited = 1
    }
    END {
      exit inherited ? 0 : 1
    }
  ' "$manifest" || fail "$manifest does not inherit workspace lints"
done

metadata_member_count="$(
  cargo metadata --locked --no-deps --format-version 1 |
    node -e '
      const fs = require("node:fs");
      const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
      process.stdout.write(String(metadata.workspace_members.length));
    '
)"
[[ "$member_count" -eq "$metadata_member_count" ]] ||
  fail "found $member_count manifests but Cargo reports $metadata_member_count workspace members"

echo "workspace policy invariant passed ($member_count members, ${#internal_dependencies[@]} centralized internal dependencies)"
