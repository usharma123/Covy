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

cargo_workspace_count=0
while IFS= read -r manifest; do
  lock_file="${manifest%Cargo.toml}Cargo.lock"
  [[ -f "$lock_file" ]] ||
    fail "$manifest defines a Cargo workspace without $lock_file"
  git ls-files --error-unmatch -- "$lock_file" >/dev/null 2>&1 ||
    fail "$lock_file is not tracked"
  cargo metadata \
    --locked \
    --offline \
    --manifest-path "$manifest" \
    --format-version 1 >/dev/null
  cargo_workspace_count=$((cargo_workspace_count + 1))
done < <(
  git ls-files '*Cargo.toml' |
    python3 -c '
import pathlib
import sys
import tomllib

for raw_path in sys.stdin:
    path = pathlib.Path(raw_path.strip())
    if not path.name:
        continue
    if path.parts[:2] == ("scripts", "fixtures"):
        continue
    with path.open("rb") as manifest_file:
        manifest = tomllib.load(manifest_file)
    if isinstance(manifest.get("workspace"), dict):
        print(path.as_posix())
'
)
[[ "$cargo_workspace_count" -gt 0 ]] ||
  fail "no tracked Cargo workspaces were discovered"

python3 scripts/check_cargo_publish_policy.py

grep -Fqx 'rust-version = "1.88.0"' Cargo.toml ||
  fail 'workspace rust-version must be exactly "1.88.0"'
grep -Fqx '[workspace.lints.rust]' Cargo.toml ||
  fail "workspace Rust lint policy is missing"
grep -Fqx '[workspace.lints.clippy]' Cargo.toml ||
  fail "workspace Clippy lint policy is missing"
grep -Fqx 'unsafe-op-in-unsafe-fn = "deny"' Cargo.toml ||
  fail "unsafe operations in unsafe functions must require explicit blocks"
grep -Fqx 'missing-safety-doc = "deny"' Cargo.toml ||
  fail "public unsafe APIs must document their safety contract"
grep -Fqx 'undocumented-unsafe-blocks = "deny"' Cargo.toml ||
  fail "unsafe blocks must carry a local safety rationale"

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

read -r metadata_member_count internal_dependency_count < <(
  cargo metadata --locked --no-deps --format-version 1 |
    node -e '
      const fs = require("node:fs");
      const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
      const members = new Set(metadata.workspace_members);
      const packages = metadata.packages.filter((candidate) =>
        members.has(candidate.id)
      );
      const names = new Set(packages.map((candidate) => candidate.name));
      const internal = new Set();
      for (const candidate of packages) {
        for (const dependency of candidate.dependencies) {
          if (names.has(dependency.name)) {
            internal.add(dependency.name);
          }
        }
      }
      process.stdout.write(`${packages.length} ${internal.size}\n`);
    '
)
[[ "$member_count" -eq "$metadata_member_count" ]] ||
  fail "found $member_count manifests but Cargo reports $metadata_member_count workspace members"

echo "workspace policy invariant passed ($member_count members, $internal_dependency_count metadata-derived internal dependencies, $cargo_workspace_count locked Cargo workspaces)"
