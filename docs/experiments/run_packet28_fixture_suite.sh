#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACKET28_BIN="${PACKET28_BIN:-$ROOT/target/debug/Packet28}"
P28_BIN="${P28_BIN:-$ROOT/target/debug/p28}"
RUN_ID="${PACKET28_EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT_DIR="${PACKET28_EXPERIMENT_OUT:-$ROOT/docs/experiments/fixture-suite/$RUN_ID}"
REPEATS="${PACKET28_EXPERIMENT_REPEATS:-3}"

if [[ ! -x "$PACKET28_BIN" ]]; then
  echo "Packet28 binary not found or not executable: $PACKET28_BIN" >&2
  echo "Run: cargo build -p suite-cli" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/packet28-fixture-suite.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

estimate_tokens() {
  local bytes="$1"
  python3 - "$bytes" <<'PY'
import math, sys
print(int(math.ceil(int(sys.argv[1]) / 4.0)))
PY
}

json_escape() {
  python3 - "$1" <<'PY'
import json, sys
print(json.dumps(sys.argv[1]))
PY
}

write_jsonl() {
  local file="$1"
  shift
  printf '%s\n' "$*" >> "$file"
}

now_ms() {
  python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
}

make_rust_repo() {
  local repo="$1"
  mkdir -p "$repo/src" "$repo/tests"
  cat > "$repo/Cargo.toml" <<'EOF'
[package]
name = "packet28-rust-fixture"
version = "0.1.0"
edition = "2021"

[dependencies]
EOF
  cat > "$repo/src/lib.rs" <<'EOF'
pub fn packet28_context_marker(input: &str) -> String {
    format!("context::{input}")
}

pub fn broken_sum(left: i32, right: i32) -> i32 {
    left + right + 1
}
EOF
  cat > "$repo/tests/failing.rs" <<'EOF'
#[test]
fn failing_sum_regression() {
    assert_eq!(packet28_rust_fixture::broken_sum(2, 2), 4);
}
EOF
  (cd "$repo" && git init -q && git add . && git -c user.name=Packet28 -c user.email=packet28@example.invalid commit -qm init)
}

make_node_repo() {
  local repo="$1"
  mkdir -p "$repo/src" "$repo/docs"
  cat > "$repo/package.json" <<'EOF'
{"name":"packet28-node-fixture","version":"1.0.0","type":"module","scripts":{"test":"node src/test.js"}}
EOF
  cat > "$repo/src/index.js" <<'EOF'
export function contextRoute(name) {
  return `packet28-context:${name}`;
}

export function featureFlag() {
  return "mcp-hook-memory";
}
EOF
  cat > "$repo/src/test.js" <<'EOF'
import { featureFlag } from './index.js';
if (featureFlag() !== 'mcp-hook-memory') {
  throw new Error('feature flag mismatch');
}
EOF
  cat > "$repo/docs/integration.md" <<'EOF'
# Integration Notes

Packet28 fixture docs mention MCP, hooks, memory recall, wakeup packs, and dashboard evidence.
EOF
  (cd "$repo" && git init -q && git add . && git -c user.name=Packet28 -c user.email=packet28@example.invalid commit -qm init)
}

make_docs_repo() {
  local repo="$1"
  mkdir -p "$repo/docs" "$repo/notes"
  cat > "$repo/docs/architecture.md" <<'EOF'
# Architecture

The runtime records command reductions, raw artifact handles, fallback reasons, and session adoption.
The memory layer stores feedback, graph concepts, pending extractions, and wakeup context.
EOF
  cat > "$repo/notes/handoff.md" <<'EOF'
# Handoff

Next agent should inspect Packet28 dashboard output and verify graph relations before release.
EOF
  (cd "$repo" && git init -q && git add . && git -c user.name=Packet28 -c user.email=packet28@example.invalid commit -qm init)
}

run_native() {
  local repo="$1" name="$2"
  shift 2
  local out="$OUT_DIR/$name.native.out"
  local err="$OUT_DIR/$name.native.err"
  local start end status bytes tokens
  start="$(now_ms)"
  set +e
  (cd "$repo" && "$@") >"$out" 2>"$err"
  status=$?
  set -e
  end="$(now_ms)"
  bytes="$(wc -c <"$out" | tr -d ' ')"
  tokens="$(estimate_tokens "$bytes")"
  write_jsonl "$OUT_DIR/results.jsonl" "{\"kind\":\"native\",\"workflow\":$(json_escape "$name"),\"repo\":$(json_escape "$(basename "$repo")"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"command\":$(json_escape "$*")}"
}

run_packet28() {
  local repo="$1" name="$2"
  shift 2
  local out="$OUT_DIR/$name.packet28.out"
  local err="$OUT_DIR/$name.packet28.err"
  local start end status bytes tokens raw reduced savings fallback artifact
  start="$(now_ms)"
  set +e
  (cd "$repo" && "$PACKET28_BIN" run --root "$repo" --json -- "$@") >"$out" 2>"$err"
  status=$?
  set -e
  end="$(now_ms)"
  bytes="$(wc -c <"$out" | tr -d ' ')"
  tokens="$(estimate_tokens "$bytes")"
  raw="$(python3 - "$out" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("raw_est_tokens", 0))
except Exception:
    print(0)
PY
)"
  reduced="$(python3 - "$out" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("reduced_est_tokens", 0))
except Exception:
    print(0)
PY
)"
  savings="$(python3 - "$out" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("savings_percent", 0.0))
except Exception:
    print(0.0)
PY
)"
  fallback="$(python3 - "$out" <<'PY'
import json, sys
try:
    value=json.load(open(sys.argv[1])).get("fallback_reason")
    print("" if value is None else value)
except Exception:
    print("")
PY
)"
  artifact="$(python3 - "$out" <<'PY'
import json, sys
try:
    print("true" if json.load(open(sys.argv[1])).get("raw_artifact", {}).get("available") else "false")
except Exception:
    print("false")
PY
)"
  write_jsonl "$OUT_DIR/results.jsonl" "{\"kind\":\"packet28\",\"workflow\":$(json_escape "$name"),\"repo\":$(json_escape "$(basename "$repo")"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"raw_est_tokens\":$raw,\"reduced_est_tokens\":$reduced,\"savings_percent\":$savings,\"fallback_reason\":$(json_escape "$fallback"),\"raw_artifact_available\":$artifact,\"command\":$(json_escape "$*")}"
}

run_p28_search() {
  local repo="$1" name="$2" query="$3"
  local out="$OUT_DIR/$name.p28.out"
  local err="$OUT_DIR/$name.p28.err"
  local start end status bytes tokens indexed backend fallback
  start="$(now_ms)"
  set +e
  (cd "$repo" && "$P28_BIN" --compact --stats "$query") >"$out" 2>"$err"
  status=$?
  set -e
  end="$(now_ms)"
  bytes="$(wc -c <"$out" | tr -d ' ')"
  tokens="$(estimate_tokens "$bytes")"
  backend="$(python3 - "$out" "$err" <<'PY'
import re, sys
text=open(sys.argv[1], errors="ignore").read() + "\n" + open(sys.argv[2], errors="ignore").read()
m=re.search(r"backend=([A-Za-z0-9_-]+)", text)
print(m.group(1) if m else "")
PY
)"
  fallback="$(python3 - "$out" "$err" <<'PY'
import re, sys
text=open(sys.argv[1], errors="ignore").read() + "\n" + open(sys.argv[2], errors="ignore").read()
m=re.search(r"fallback(?:_reason)?=([^\n]+)", text)
value="" if not m else m.group(1).strip()
print("" if value in ("none", "None", "null") else value)
PY
)"
  if [[ "$backend" == indexed* ]]; then
    indexed=true
  else
    indexed=false
  fi
  write_jsonl "$OUT_DIR/results.jsonl" "{\"kind\":\"p28_search\",\"workflow\":$(json_escape "$name"),\"repo\":$(json_escape "$(basename "$repo")"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"indexed_search_hit\":$indexed,\"search_backend\":$(json_escape "$backend"),\"fallback_reason\":$(json_escape "$fallback"),\"command\":$(json_escape "p28 --compact --stats $query")}"
}

summarize() {
  python3 - "$OUT_DIR/results.jsonl" "$OUT_DIR/summary.md" <<'PY'
import json, sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
packet=[r for r in rows if r["kind"]=="packet28"]
native=[r for r in rows if r["kind"]=="native"]
p28=[r for r in rows if r["kind"]=="p28_search"]
fallbacks=[r for r in packet if r.get("fallback_reason")]
fallbacks.extend(r for r in p28 if r.get("fallback_reason"))
failed=[r for r in rows if int(r.get("status") or 0) != 0]
artifact_ok=sum(1 for r in packet if r.get("raw_artifact_available"))
indexed_ok=sum(1 for r in p28 if r.get("indexed_search_hit"))
raw=sum(int(r.get("raw_est_tokens") or 0) for r in packet)
reduced=sum(int(r.get("reduced_est_tokens") or 0) for r in packet)
saved=max(raw-reduced, 0)
pct=(saved/raw*100.0) if raw else 0.0
with open(sys.argv[2], "w") as f:
    f.write("# Packet28 Fixture Experiment Suite\n\n")
    f.write("This is a repeatable local fixture suite, not a full maturity claim. It exercises synthetic repositories so regressions are cheap to reproduce before running larger real-repo experiments.\n\n")
    f.write(f"- Native runs: {len(native)}\n")
    f.write(f"- Packet28 runs: {len(packet)}\n")
    f.write(f"- p28 indexed-search runs: {len(p28)}\n")
    f.write(f"- Packet28 raw estimated tokens: {raw}\n")
    f.write(f"- Packet28 reduced estimated tokens: {reduced}\n")
    f.write(f"- Packet28 savings percent: {pct:.2f}%\n")
    f.write(f"- Fallback count: {len(fallbacks)}\n")
    f.write(f"- Failed command count: {len(failed)}\n")
    f.write(f"- Raw artifact recovery available: {artifact_ok}/{len(packet)}\n\n")
    f.write(f"- Indexed search hit rate: {indexed_ok}/{len(p28)}\n\n")
    f.write("| Kind | Repo | Workflow | Status | Tokens | Savings | Fallback |\n")
    f.write("|---|---|---:|---:|---:|---:|---|\n")
    for r in rows:
        f.write(f"| {r['kind']} | {r['repo']} | {r['workflow']} | {r['status']} | {r.get('est_stdout_tokens', r.get('raw_est_tokens', 0))} | {r.get('savings_percent', '')} | {r.get('fallback_reason', '')} |\n")
PY
}

RUST_REPO="$WORK_DIR/rust-repo"
NODE_REPO="$WORK_DIR/node-repo"
DOCS_REPO="$WORK_DIR/docs-repo"
make_rust_repo "$RUST_REPO"
make_node_repo "$NODE_REPO"
make_docs_repo "$DOCS_REPO"

cat >> "$RUST_REPO/src/lib.rs" <<'EOF'

pub fn review_target() -> &'static str {
    "hook telemetry should keep raw artifact handles visible"
}
EOF

cat >> "$NODE_REPO/src/index.js" <<'EOF'

export function implementationTarget() {
  return "dashboard-evidence";
}
EOF

: > "$OUT_DIR/results.jsonl"
for repeat in $(seq 1 "$REPEATS"); do
  run_native "$RUST_REPO" "rust_search_$repeat" rg -n packet28_context_marker src tests
  run_packet28 "$RUST_REPO" "rust_search_$repeat" rg -n packet28_context_marker src tests
  if [[ -x "$P28_BIN" ]]; then
    run_p28_search "$RUST_REPO" "rust_indexed_search_$repeat" packet28_context_marker
  fi
  run_native "$RUST_REPO" "rust_failing_test_$repeat" cargo test
  run_packet28 "$RUST_REPO" "rust_failing_test_$repeat" cargo test
  run_native "$RUST_REPO" "rust_code_review_$repeat" git diff -- src tests
  run_packet28 "$RUST_REPO" "rust_code_review_$repeat" git diff -- src tests

  run_native "$NODE_REPO" "node_search_$repeat" rg -n "mcp|hook|memory" .
  run_packet28 "$NODE_REPO" "node_search_$repeat" rg -n "mcp|hook|memory" .
  if [[ -x "$P28_BIN" ]]; then
    run_p28_search "$NODE_REPO" "node_indexed_search_$repeat" memory
  fi
  run_native "$NODE_REPO" "node_docs_lookup_$repeat" find docs -type f
  run_packet28 "$NODE_REPO" "node_docs_lookup_$repeat" find docs -type f
  run_native "$NODE_REPO" "node_implementation_diff_$repeat" git diff -- src
  run_packet28 "$NODE_REPO" "node_implementation_diff_$repeat" git diff -- src

  run_native "$DOCS_REPO" "docs_handoff_lookup_$repeat" rg -n "handoff|dashboard|graph" .
  run_packet28 "$DOCS_REPO" "docs_handoff_lookup_$repeat" rg -n "handoff|dashboard|graph" .
  run_native "$DOCS_REPO" "docs_bootstrap_$repeat" find . -maxdepth 2 -type f
  run_packet28 "$DOCS_REPO" "docs_bootstrap_$repeat" find . -maxdepth 2 -type f
done

for repo in "$RUST_REPO" "$NODE_REPO" "$DOCS_REPO"; do
  "$PACKET28_BIN" gain --root "$repo" --json > "$OUT_DIR/$(basename "$repo").gain.json"
  "$PACKET28_BIN" discover --root "$repo" --json > "$OUT_DIR/$(basename "$repo").discover.json"
done

summarize
echo "Wrote Packet28 fixture experiment artifacts to $OUT_DIR"
