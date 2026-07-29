#!/usr/bin/env bash
set -euo pipefail

ROOT="${CODEX_AUTOFIX_ROOT:-${GITHUB_WORKSPACE:-$(pwd)}}"
FAILURE_LOG="${CI_FAILURE_LOG:-$ROOT/.packet28/ci-autofix/failure.log}"
OUT_DIR="${CODEX_AUTOFIX_DIR:-$ROOT/.packet28/ci-autofix}"
CODEX_BIN="${CODEX_BIN:-codex}"
VERIFY_CMD="${CODEX_AUTOFIX_VERIFY:-cargo test --locked --workspace --all-targets}"
RUN_URL="${CI_RUN_URL:-unknown}"
DRY_RUN="${CODEX_AUTOFIX_DRY_RUN:-0}"

mkdir -p "$OUT_DIR"

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
- If the failure is caused by external infrastructure, credentials, or a flaky service, make no
  code changes and explain that clearly.
- Prefer targeted tests first, then run this verification command before finishing:
  ${VERIFY_CMD}

Failed run: ${RUN_URL}
Failure log path: ${FAILURE_LOG}

Start by reading the failure log, identify the failing command and root cause, patch the repo, run the relevant tests, then run the verification command above.
PROMPT

if [[ "$DRY_RUN" == "1" || "$DRY_RUN" == "true" || "$DRY_RUN" == "TRUE" ]]; then
  echo "Codex autofix dry run wrote prompt: $OUT_DIR/prompt.txt" >&2
  exit 0
fi

if ! command -v "$CODEX_BIN" >/dev/null 2>&1; then
  echo "codex CLI not found; skipping autofix" >&2
  exit 78
fi

echo "Running Codex autofix with a secret-filtered subprocess environment" >&2
"$CODEX_BIN" exec \
  --ignore-user-config \
  --strict-config \
  --config 'shell_environment_policy.inherit="core"' \
  --config 'shell_environment_policy.ignore_default_excludes=false' \
  --config 'shell_environment_policy.exclude=["OPENAI_API_KEY","GITHUB_TOKEN","GH_TOKEN","ACTIONS_ID_TOKEN_REQUEST_TOKEN"]' \
  --cd "$ROOT" \
  --sandbox workspace-write \
  --output-last-message "$OUT_DIR/final.md" \
  - <"$OUT_DIR/prompt.txt" 2>&1 | tee "$OUT_DIR/codex.log"

if git -C "$ROOT" diff --quiet -- . ':!.packet28'; then
  echo "Codex completed but produced no tracked diff" >&2
  exit 0
fi

echo "Codex produced a diff:" >&2
git -C "$ROOT" diff --stat -- . ':!.packet28'
git -C "$ROOT" diff --binary --full-index -- . ':!.packet28' >"$OUT_DIR/diff.patch"
