#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACKET28_BIN="${PACKET28_BIN:-$ROOT/target/debug/Packet28}"
P28_BIN="${P28_BIN:-$ROOT/target/debug/p28}"
RUN_ID="${PACKET28_REAL_EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT_DIR="${PACKET28_REAL_EXPERIMENT_OUT:-$ROOT/docs/experiments/real-repos/$RUN_ID}"
REPEATS="${PACKET28_REAL_EXPERIMENT_REPEATS:-3}"
WORK_DIR="${PACKET28_REAL_EXPERIMENT_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/packet28-real-repos.XXXXXX")}"
KEEP_WORK="${PACKET28_REAL_EXPERIMENT_KEEP_WORK:-0}"

if [[ ! -x "$PACKET28_BIN" ]]; then
  echo "Packet28 binary not found or not executable: $PACKET28_BIN" >&2
  echo "Run: cargo build -p suite-cli" >&2
  exit 2
fi

mkdir -p "$OUT_DIR" "$WORK_DIR"
if [[ "$KEEP_WORK" != "1" ]]; then
  trap 'chmod -R u+w "$WORK_DIR" 2>/dev/null || true; rm -rf "$WORK_DIR" 2>/dev/null || true' EXIT
fi

estimate_tokens() {
  python3 - "$1" <<'PY'
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

now_ms() {
  python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
}

clone_repo() {
  local name="$1" url="$2"
  local dest="$WORK_DIR/$name"
  if [[ ! -d "$dest/.git" ]]; then
    git clone --depth 50 "$url" "$dest" >/dev/null 2>"$OUT_DIR/${name}.clone.err"
  fi
  printf '%s\n' "$dest"
}

clone_local_repo() {
  local name="$1" source="$2"
  local dest="$WORK_DIR/$name"
  if [[ ! -d "$dest/.git" ]]; then
    git clone --local "$source" "$dest" >/dev/null 2>"$OUT_DIR/${name}.clone.err"
  fi
  printf '%s\n' "$dest"
}

record_jsonl() {
  printf '%s\n' "$1" >> "$OUT_DIR/results.jsonl"
}

run_native() {
  local repo="$1" repo_name="$2" workflow="$3"
  shift 3
  local out="$OUT_DIR/${repo_name}.${workflow}.native.out"
  local err="$OUT_DIR/${repo_name}.${workflow}.native.err"
  local start end status bytes tokens
  start="$(now_ms)"
  set +e
  (cd "$repo" && "$@") >"$out" 2>"$err"
  status=$?
  set -e
  end="$(now_ms)"
  bytes="$(wc -c <"$out" | tr -d ' ')"
  tokens="$(estimate_tokens "$bytes")"
  record_jsonl "{\"kind\":\"native\",\"repo\":$(json_escape "$repo_name"),\"workflow\":$(json_escape "$workflow"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"command\":$(json_escape "$*")}"
}

run_packet28() {
  local repo="$1" repo_name="$2" workflow="$3"
  shift 3
  local out="$OUT_DIR/${repo_name}.${workflow}.packet28.out"
  local err="$OUT_DIR/${repo_name}.${workflow}.packet28.err"
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
try: print(json.load(open(sys.argv[1])).get("raw_est_tokens", 0))
except Exception: print(0)
PY
)"
  reduced="$(python3 - "$out" <<'PY'
import json, sys
try: print(json.load(open(sys.argv[1])).get("reduced_est_tokens", 0))
except Exception: print(0)
PY
)"
  savings="$(python3 - "$out" <<'PY'
import json, sys
try: print(json.load(open(sys.argv[1])).get("savings_percent", 0.0))
except Exception: print(0.0)
PY
)"
  fallback="$(python3 - "$out" <<'PY'
import json, sys
try:
    value=json.load(open(sys.argv[1])).get("fallback_reason")
    print("" if value is None else value)
except Exception: print("")
PY
)"
  artifact="$(python3 - "$out" <<'PY'
import json, sys
try: print("true" if json.load(open(sys.argv[1])).get("raw_artifact", {}).get("available") else "false")
except Exception: print("false")
PY
)"
  record_jsonl "{\"kind\":\"packet28\",\"repo\":$(json_escape "$repo_name"),\"workflow\":$(json_escape "$workflow"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"raw_est_tokens\":$raw,\"reduced_est_tokens\":$reduced,\"savings_percent\":$savings,\"fallback_reason\":$(json_escape "$fallback"),\"raw_artifact_available\":$artifact,\"command\":$(json_escape "$*")}"
}

run_p28_search() {
  local repo="$1" repo_name="$2" workflow="$3" query="$4"
  local out="$OUT_DIR/${repo_name}.${workflow}.p28.out"
  local err="$OUT_DIR/${repo_name}.${workflow}.p28.err"
  local start end status bytes tokens backend fallback indexed
  if [[ ! -x "$P28_BIN" ]]; then
    return 0
  fi
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
text=open(sys.argv[1], errors="ignore").read()+"\n"+open(sys.argv[2], errors="ignore").read()
m=re.search(r"backend=([A-Za-z0-9_-]+)", text)
print(m.group(1) if m else "")
PY
)"
  fallback="$(python3 - "$out" "$err" <<'PY'
import re, sys
text=open(sys.argv[1], errors="ignore").read()+"\n"+open(sys.argv[2], errors="ignore").read()
m=re.search(r"fallback(?:_reason)?=([^\n]+)", text)
value="" if not m else m.group(1).strip()
print("" if value in ("none", "None", "null") else value)
PY
)"
  [[ "$backend" == indexed* ]] && indexed=true || indexed=false
  record_jsonl "{\"kind\":\"p28_search\",\"repo\":$(json_escape "$repo_name"),\"workflow\":$(json_escape "$workflow"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"indexed_search_hit\":$indexed,\"search_backend\":$(json_escape "$backend"),\"fallback_reason\":$(json_escape "$fallback"),\"command\":$(json_escape "p28 --compact --stats $query")}"
}

run_workflow_pair() {
  local repo="$1" repo_name="$2" workflow="$3"
  shift 3
  run_native "$repo" "$repo_name" "$workflow" "$@"
  run_packet28 "$repo" "$repo_name" "$workflow" "$@"
}

summarize() {
  python3 - "$OUT_DIR/results.jsonl" "$OUT_DIR/summary.md" "$REPEATS" "$RUN_ID" <<'PY'
import collections, json, statistics, sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
packet=[r for r in rows if r["kind"]=="packet28"]
native=[r for r in rows if r["kind"]=="native"]
search=[r for r in rows if r["kind"]=="p28_search"]
repos=sorted({r["repo"] for r in rows})
workflows=sorted({r["workflow"] for r in rows if r["kind"] in {"native","packet28"}})
fallbacks=[r for r in packet if r.get("fallback_reason")]
p28_fallbacks=[r for r in search if r.get("fallback_reason")]
failed=[r for r in rows if r.get("status") not in (0, None)]
artifact_ok=sum(1 for r in packet if r.get("raw_artifact_available"))
avg_savings=statistics.mean([float(r.get("savings_percent") or 0) for r in packet]) if packet else 0.0
indexed=sum(1 for r in search if r.get("indexed_search_hit"))
by_workflow=collections.Counter((r["kind"], r["workflow"]) for r in rows)
with open(sys.argv[2], "w") as f:
    f.write("# Packet28 Real Repository Experiment\n\n")
    f.write("This is a repeatable real-repository evidence run for the parity goal. It is not a full maturity claim by itself.\n\n")
    f.write(f"- Run id: {sys.argv[4]}\n")
    f.write(f"- Repeats requested: {sys.argv[3]}\n")
    f.write(f"- Repositories: {', '.join(repos)}\n")
    f.write(f"- Workflows: {', '.join(workflows)}\n")
    f.write(f"- Native runs: {len(native)}\n")
    f.write(f"- Packet28 runs: {len(packet)}\n")
    f.write(f"- Packet28 average savings percent: {avg_savings:.2f}\n")
    f.write(f"- Packet28 fallback count: {len(fallbacks)}\n")
    f.write(f"- p28 search fallback count: {len(p28_fallbacks)}\n")
    f.write(f"- Failed command count: {len(failed)}\n")
    f.write(f"- Raw artifact recovery available: {artifact_ok}/{len(packet)}\n")
    f.write(f"- Indexed-search hit count: {indexed}/{len(search)}\n\n")
    f.write("## Coverage Counts\n\n")
    f.write("| kind | workflow | runs |\n|---|---:|---:|\n")
    for (kind, workflow), count in sorted(by_workflow.items()):
        f.write(f"| {kind} | {workflow} | {count} |\n")
    f.write("\n## Rows\n\n")
    f.write("| kind | repo | workflow | status | est tokens | savings % | fallback |\n|---|---|---|---:|---:|---:|---|\n")
    for r in rows:
        f.write(f"| {r['kind']} | {r['repo']} | {r['workflow']} | {r.get('status','')} | {r.get('est_stdout_tokens', r.get('raw_est_tokens', 0))} | {r.get('savings_percent','')} | {r.get('fallback_reason','')} |\n")
PY
}

: > "$OUT_DIR/results.jsonl"

PACKET_REPO="$(clone_local_repo packet28 "$ROOT")"
RIPGREP_REPO="$(clone_repo ripgrep https://github.com/BurntSushi/ripgrep.git)"
FD_REPO="$(clone_repo fd https://github.com/sharkdp/fd.git)"

declare -a REPOS=(
  "packet28:$PACKET_REPO"
  "ripgrep:$RIPGREP_REPO"
  "fd:$FD_REPO"
)

for repeat in $(seq 1 "$REPEATS"); do
  for entry in "${REPOS[@]}"; do
    repo_name="${entry%%:*}"
    repo="${entry#*:}"
    run_workflow_pair "$repo" "$repo_name" "search_$repeat" rg -n "TODO|FIXME|unsafe|panic|unwrap" .
    run_p28_search "$repo" "$repo_name" "indexed_search_$repeat" "TODO|FIXME|unsafe|panic|unwrap"
    run_workflow_pair "$repo" "$repo_name" "code_review_$repeat" git diff --stat HEAD~1..HEAD
    run_workflow_pair "$repo" "$repo_name" "failing_test_triage_$repeat" rg -n "assert|panic|error|fail" .
    run_workflow_pair "$repo" "$repo_name" "implementation_$repeat" git status --short
    run_workflow_pair "$repo" "$repo_name" "docs_lookup_$repeat" rg -n "install|usage|configuration|hook|MCP|memory|context" .
    run_workflow_pair "$repo" "$repo_name" "handoff_bootstrap_$repeat" git log -1 --stat
  done
done

for entry in "${REPOS[@]}"; do
  repo_name="${entry%%:*}"
  repo="${entry#*:}"
  "$PACKET28_BIN" gain --root "$repo" --json > "$OUT_DIR/${repo_name}.gain.json" || true
  "$PACKET28_BIN" discover --root "$repo" --json > "$OUT_DIR/${repo_name}.discover.json" || true
done

summarize
echo "Wrote Packet28 real-repo experiment artifacts to $OUT_DIR"
