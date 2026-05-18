#!/usr/bin/env bash
set -euo pipefail

ROOT="${GITHUB_WORKSPACE:-$(pwd)}"
FAILURE_LOG="${CI_FAILURE_LOG:-$ROOT/.packet28/ci-autofix/failure.log}"
OUT_DIR="${CODEX_AUTOFIX_DIR:-$ROOT/.packet28/ci-autofix}"
CODEX_CMD="${CODEX_CMD:-codex exec}"
VERIFY_CMD="${CODEX_AUTOFIX_VERIFY:-cargo test --workspace --all-targets}"
RUN_URL="${CI_RUN_URL:-unknown}"

mkdir -p "$OUT_DIR"

if ! command -v codex >/dev/null 2>&1; then
  echo "codex CLI not found; skipping autofix" >&2
  exit 78
fi

if [[ ! -s "$FAILURE_LOG" ]]; then
  echo "CI failure log is missing or empty: $FAILURE_LOG" >&2
  exit 1
fi

cat >"$OUT_DIR/prompt.txt" <<PROMPT
You are running inside GitHub Actions on a trusted same-repository CI failure.

Goal: inspect the failing CI log, make the smallest correct code or test fix, and leave the repository ready for a pull request.

Constraints:
- Do not modify secrets, workflow permissions, release credentials, or unrelated generated artifacts.
- Do not run destructive git commands.
- Do not push, commit, or open a PR; the workflow will handle that after verification.
- Prefer targeted tests first, then run this verification command before finishing:
  ${VERIFY_CMD}

Failed run: ${RUN_URL}
Failure log path: ${FAILURE_LOG}

Start by reading the failure log, identify the failing command and root cause, patch the repo, run the relevant tests, then run the verification command above.
PROMPT

echo "Running Codex autofix with: $CODEX_CMD" >&2
# shellcheck disable=SC2086
$CODEX_CMD "$(cat "$OUT_DIR/prompt.txt")" 2>&1 | tee "$OUT_DIR/codex.log"

if git diff --quiet -- . ':!.packet28'; then
  echo "Codex completed but produced no tracked diff" >&2
  exit 0
fi

echo "Codex produced a diff:" >&2
git diff --stat -- . ':!.packet28'
