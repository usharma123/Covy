#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
P28_BIN="${P28_BIN:-$ROOT/target/debug/p28}"
FFF_MCP_BIN="${P28_FFF_MCP_BIN:-${FFF_MCP_BIN:-}}"
RUN_ID="${P28_SEARCH_BACKEND_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT_DIR="${P28_SEARCH_BACKEND_OUT:-$ROOT/docs/experiments/search-backends/$RUN_ID}"
WORK_DIR="${P28_SEARCH_BACKEND_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/p28-search-backends.XXXXXX")}"
REPEATS="${P28_SEARCH_BACKEND_REPEATS:-3}"
KEEP_WORK="${P28_SEARCH_BACKEND_KEEP_WORK:-0}"
MAX_TOTAL_MATCHES="${P28_SEARCH_BACKEND_MAX_TOTAL_MATCHES:-50}"

if [[ ! -x "$P28_BIN" ]]; then
  echo "p28 binary not found or not executable: $P28_BIN" >&2
  echo "Run: cargo build -p packet28-search-cli" >&2
  exit 2
fi

if [[ -z "$FFF_MCP_BIN" || ! -x "$FFF_MCP_BIN" ]]; then
  echo "fff-mcp binary not found or not executable." >&2
  echo "Set P28_FFF_MCP_BIN=/path/to/fff-mcp or FFF_MCP_BIN=/path/to/fff-mcp." >&2
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

hit_count() {
  local path="$1"
  python3 - "$path" <<'PY'
import sys
print(sum(1 for line in open(sys.argv[1], errors="ignore") if line.strip()))
PY
}

first_hits_hash() {
  local path="$1"
  python3 - "$path" <<'PY'
import hashlib, sys
lines=[line.strip() for line in open(sys.argv[1], errors="ignore") if line.strip()]
print(hashlib.sha256("\n".join(lines[:50]).encode()).hexdigest())
PY
}

record_search() {
  local kind="$1" repo="$2" repo_name="$3" workflow="$4" query="$5"
  local out="$OUT_DIR/${repo_name}.${workflow}.${kind}.out"
  local err="$OUT_DIR/${repo_name}.${workflow}.${kind}.err"
  local start end status bytes tokens lines hash backend fallback command_text
  start="$(now_ms)"
  set +e
  case "$kind" in
    native_rg)
      (cd "$repo" && rg -n --max-count "$MAX_TOTAL_MATCHES" "$query" .) >"$out" 2>"$err"
      status=$?
      command_text="rg -n --max-count $MAX_TOTAL_MATCHES $query ."
      ;;
    p28_indexed)
      (cd "$repo" && "$P28_BIN" --compact --stats --max-total-matches "$MAX_TOTAL_MATCHES" "$query") >"$out" 2>"$err"
      status=$?
      command_text="p28 --compact --stats --max-total-matches $MAX_TOTAL_MATCHES $query"
      ;;
    p28_fff)
      (cd "$repo" && P28_FFF_MCP_BIN="$FFF_MCP_BIN" "$P28_BIN" --engine fff --compact --stats --max-total-matches "$MAX_TOTAL_MATCHES" "$query") >"$out" 2>"$err"
      status=$?
      command_text="P28_FFF_MCP_BIN=... p28 --engine fff --compact --stats --max-total-matches $MAX_TOTAL_MATCHES $query"
      ;;
    *)
      echo "unknown search kind: $kind" >&2
      exit 2
      ;;
  esac
  set -e
  end="$(now_ms)"
  bytes="$(wc -c <"$out" | tr -d ' ')"
  tokens="$(estimate_tokens "$bytes")"
  lines="$(hit_count "$out")"
  hash="$(first_hits_hash "$out")"
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
  record_jsonl "{\"kind\":$(json_escape "$kind"),\"repo\":$(json_escape "$repo_name"),\"workflow\":$(json_escape "$workflow"),\"query\":$(json_escape "$query"),\"status\":$status,\"duration_ms\":$((end-start)),\"stdout_bytes\":$bytes,\"est_stdout_tokens\":$tokens,\"hit_lines\":$lines,\"first_hits_sha256\":$(json_escape "$hash"),\"search_backend\":$(json_escape "$backend"),\"fallback_reason\":$(json_escape "$fallback"),\"command\":$(json_escape "$command_text")}"
}

summarize() {
  python3 - "$OUT_DIR/results.jsonl" "$OUT_DIR/summary.md" "$REPEATS" "$RUN_ID" "$FFF_MCP_BIN" <<'PY'
import collections, json, statistics, sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
by_kind=collections.defaultdict(list)
for row in rows:
    by_kind[row["kind"]].append(row)
failed=[r for r in rows if r["status"] not in (0, 1)]
fallbacks=[r for r in rows if r.get("fallback_reason")]
backends=collections.Counter(r.get("search_backend") or "(native)" for r in rows)
coverage=collections.Counter((r["repo"], r["query"], r["kind"]) for r in rows)
def avg(kind, field):
    values=[float(r[field]) for r in by_kind[kind]]
    return statistics.mean(values) if values else 0.0
with open(sys.argv[2], "w") as f:
    f.write("# p28 Search Backend Comparison\n\n")
    f.write("This artifact compares native `rg`, default Packet28 indexed `p28`, and the opt-in `p28 --engine fff` MCP adapter. It is search-backend evidence only, not a full parity maturity claim.\n\n")
    f.write(f"- Run id: {sys.argv[4]}\n")
    f.write(f"- Repeats requested: {sys.argv[3]}\n")
    f.write(f"- fff-mcp binary: `{sys.argv[5]}`\n")
    f.write(f"- Total rows: {len(rows)}\n")
    f.write(f"- Failed rows excluding rg no-match status 1: {len(failed)}\n")
    f.write(f"- Fallback rows: {len(fallbacks)}\n")
    f.write(f"- Average native rg duration ms: {avg('native_rg', 'duration_ms'):.1f}\n")
    f.write(f"- Average p28 indexed duration ms: {avg('p28_indexed', 'duration_ms'):.1f}\n")
    f.write(f"- Average p28 fff duration ms: {avg('p28_fff', 'duration_ms'):.1f}\n\n")
    f.write("## Backend Counts\n\n")
    f.write("| backend | rows |\n|---|---:|\n")
    for backend, count in sorted(backends.items()):
        f.write(f"| {backend} | {count} |\n")
    f.write("\n## Coverage\n\n")
    f.write("| repo | query | native rg | p28 indexed | p28 fff |\n|---|---|---:|---:|---:|\n")
    repos=sorted({r["repo"] for r in rows})
    queries=sorted({r["query"] for r in rows})
    for repo in repos:
        for query in queries:
            f.write(f"| {repo} | `{query}` | {coverage[(repo, query, 'native_rg')]} | {coverage[(repo, query, 'p28_indexed')]} | {coverage[(repo, query, 'p28_fff')]} |\n")
    f.write("\n## Rows\n\n")
    f.write("| kind | repo | query | status | duration ms | hit lines | tokens | backend | fallback |\n")
    f.write("|---|---|---|---:|---:|---:|---:|---|---|\n")
    for r in rows:
        f.write(f"| {r['kind']} | {r['repo']} | `{r['query']}` | {r['status']} | {r['duration_ms']} | {r['hit_lines']} | {r['est_stdout_tokens']} | {r.get('search_backend','')} | {r.get('fallback_reason','')} |\n")
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
declare -a QUERIES=("fn" "Result" "TODO")

for repeat in $(seq 1 "$REPEATS"); do
  for entry in "${REPOS[@]}"; do
    repo_name="${entry%%:*}"
    repo="${entry#*:}"
    for query in "${QUERIES[@]}"; do
      workflow="search_${query}_${repeat}"
      record_search native_rg "$repo" "$repo_name" "$workflow" "$query"
      record_search p28_indexed "$repo" "$repo_name" "$workflow" "$query"
      record_search p28_fff "$repo" "$repo_name" "$workflow" "$query"
    done
  done
done

summarize
echo "Wrote p28 search backend comparison artifacts to $OUT_DIR"
