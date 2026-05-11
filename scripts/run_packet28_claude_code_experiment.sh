#!/usr/bin/env bash
set -euo pipefail

SOURCE_ROOT="${SOURCE_ROOT:-/Users/utsavsharma/Documents/GitHub/Buns/claude-code-main}"
COVERAGE_ROOT="${COVERAGE_ROOT:-/Users/utsavsharma/Documents/GitHub/Coverage}"
PACKET28_BIN="${PACKET28_BIN:-$COVERAGE_ROOT/target/debug/Packet28}"
P28_BIN="${P28_BIN:-$COVERAGE_ROOT/target/debug/p28}"
RUN_ID="${RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT_DIR="${OUT_DIR:-$COVERAGE_ROOT/docs/experiments/packet28-claude-code-main/$RUN_ID}"
WORK_DIR="${WORK_DIR:-/tmp/packet28-claude-code-main-$RUN_ID}"
TASK_PATTERN="${TASK_PATTERN:-hook|mcp|tool_use}"
CONTROL_PATTERN="${CONTROL_PATTERN:-tool_use}"
INDEX_TIMEOUT_SECONDS="${INDEX_TIMEOUT_SECONDS:-300}"

REPO_DIR="$WORK_DIR/repo"
mkdir -p "$OUT_DIR" "$WORK_DIR"

if [ ! -x "$PACKET28_BIN" ] || [ ! -x "$P28_BIN" ]; then
  cargo build -q -p suite-cli --bin Packet28
  cargo build -q -p packet28-search-cli --bin p28
fi

rsync -a --delete \
  --exclude '.git' \
  --exclude 'node_modules' \
  "$SOURCE_ROOT/" "$REPO_DIR/"

git -C "$REPO_DIR" init -q

measure() {
  local name="$1"
  shift
  local stdout_file="$OUT_DIR/$name.out"
  local stderr_file="$OUT_DIR/$name.err"
  local status_file="$OUT_DIR/$name.status"
  local start_ns end_ns status bytes est_tokens duration_ms

  start_ns="$(date +%s%N)"
  set +e
  (cd "$REPO_DIR" && "$@") >"$stdout_file" 2>"$stderr_file"
  status=$?
  set -e
  end_ns="$(date +%s%N)"
  bytes="$(wc -c <"$stdout_file" | tr -d ' ')"
  est_tokens="$(((bytes + 3) / 4))"
  duration_ms="$(((end_ns - start_ns) / 1000000))"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$name" "$status" "$duration_ms" "$bytes" "$est_tokens" "$*" >>"$OUT_DIR/metrics.tsv"
  printf 'status=%s\nbytes=%s\nest_tokens=%s\nduration_ms=%s\ncommand=%q\n' \
    "$status" "$bytes" "$est_tokens" "$duration_ms" "$*" >"$status_file"
}

poll_index_ready() {
  local deadline=$((SECONDS + INDEX_TIMEOUT_SECONDS))
  printf 'elapsed_seconds\tready\tstatus\tindexed_files\ttotal_files\tregex_status\tregex_indexed_files\tregex_total_files\n' >"$OUT_DIR/index-poll.tsv"
  while [ "$SECONDS" -lt "$deadline" ]; do
    "$PACKET28_BIN" daemon index status --root "$REPO_DIR" --json >"$OUT_DIR/daemon-index-status.json" 2>"$OUT_DIR/daemon-index-status.err" || true
    node -e '
      const fs = require("fs");
      const p = process.argv[1];
      const elapsed = process.argv[2];
      const v = JSON.parse(fs.readFileSync(p, "utf8"));
      const m = v.manifest || {};
      console.log([
        elapsed,
        Boolean(v.ready),
        m.status || "",
        m.indexed_files ?? 0,
        m.total_files ?? 0,
        m.regex_status || "",
        m.regex_indexed_files ?? 0,
        m.regex_total_files ?? 0,
      ].join("\t"));
    ' "$OUT_DIR/daemon-index-status.json" "$SECONDS" >>"$OUT_DIR/index-poll.tsv"
    if node -e 'const fs=require("fs"); const p=process.argv[1]; const v=JSON.parse(fs.readFileSync(p,"utf8")); process.exit(v.ready ? 0 : 1)' "$OUT_DIR/daemon-index-status.json"; then
      return 0
    fi
    sleep 1
  done
  return 1
}

printf 'name\tstatus\tduration_ms\tstdout_bytes\test_stdout_tokens\tcommand\n' >"$OUT_DIR/metrics.tsv"

"$PACKET28_BIN" --version >"$OUT_DIR/packet28-version.txt"
"$P28_BIN" --help >"$OUT_DIR/p28-help.txt"
find "$REPO_DIR" -type f | wc -l | tr -d ' ' >"$OUT_DIR/file-count.txt"
find "$REPO_DIR" -type f \( -name '*.ts' -o -name '*.tsx' \) | wc -l | tr -d ' ' >"$OUT_DIR/ts-file-count.txt"
du -sh "$REPO_DIR" >"$OUT_DIR/repo-size.txt"

"$PACKET28_BIN" daemon start --root "$REPO_DIR" >"$OUT_DIR/daemon-start.out" 2>"$OUT_DIR/daemon-start.err" || true
"$PACKET28_BIN" daemon index rebuild --root "$REPO_DIR" >"$OUT_DIR/daemon-index-rebuild.out" 2>"$OUT_DIR/daemon-index-rebuild.err"
if ! poll_index_ready; then
  "$PACKET28_BIN" daemon status --root "$REPO_DIR" --json >"$OUT_DIR/daemon-status-timeout.json" 2>"$OUT_DIR/daemon-status-timeout.err" || true
  printf 'Packet28 index did not become ready within %s seconds\n' "$INDEX_TIMEOUT_SECONDS" >"$OUT_DIR/index-timeout.txt"
fi

measure native_rg rg -n "$TASK_PATTERN" -g '*.ts' -g '*.tsx' .
measure native_file_list find . -type f \( -name '*.ts' -o -name '*.tsx' \)
measure packet28_rewrite "$PACKET28_BIN" rewrite "rg -n \"$TASK_PATTERN\" -g \"*.ts\" -g \"*.tsx\" ." --json --root "$REPO_DIR"
measure packet28_run_rg "$PACKET28_BIN" run --root "$REPO_DIR" --json -- rg -n "$TASK_PATTERN" -g '*.ts' -g '*.tsx' .
measure p28_compact "$P28_BIN" --compact --stats "$TASK_PATTERN" .
measure p28_json "$P28_BIN" --json --max-total-matches 50 "$TASK_PATTERN" .
measure native_control_rg rg -n --fixed-strings "$CONTROL_PATTERN" -g '*.ts' -g '*.tsx' .
measure p28_indexed_control "$P28_BIN" --engine indexed --transport inproc --compact --stats "$CONTROL_PATTERN"

"$PACKET28_BIN" daemon status --root "$REPO_DIR" --json >"$OUT_DIR/daemon-status-final.json" 2>"$OUT_DIR/daemon-status-final.err" || true
"$PACKET28_BIN" daemon stop --root "$REPO_DIR" >"$OUT_DIR/daemon-stop.out" 2>"$OUT_DIR/daemon-stop.err" || true

node "$COVERAGE_ROOT/scripts/summarize_packet28_experiment.mjs" "$OUT_DIR" "$SOURCE_ROOT" "$REPO_DIR" "$TASK_PATTERN"
printf '%s\n' "$OUT_DIR"
