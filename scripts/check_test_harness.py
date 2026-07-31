#!/usr/bin/env python3
"""Enforce the suite-cli integration harness boundary and reviewed exceptions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence


ROOT = Path(__file__).resolve().parents[1]
TEST_ROOT = Path("crates/suite-cli/tests")
HARNESS_PATH = "crates/suite-cli/tests/support/process_harness.rs"
MANUAL_LIFECYCLE_ALLOWLIST = {
    HARNESS_PATH: "the shared bounded process owner",
}

MANUAL_LIFECYCLE = re.compile(
    r"\bChild(?:Stdin|Stdout)?\b|Stdio::piped\(\)|"
    r"\.spawn\(\)|\.kill\(\)|\.wait\(\)|wait_with_output\("
)
NESTED_CARGO = re.compile(
    r'(?:std::process::)?(?:Command|ProcessCommand)::new\(\s*"cargo"\s*\)'
)
DIRECT_GIT = re.compile(
    r'(?:std::process::)?(?:Command|ProcessCommand)::new\(\s*"git"\s*\)'
)
SUPPORT_SYNC_CHILD = re.compile(r"\.(?:output|status)\(\)")
MANUAL_CLEANUP = re.compile(
    r"\b(?:std::fs::|fs::)?(?:remove_file|remove_dir|remove_dir_all)\s*\("
)
RAW_MCP_FRAMING = re.compile(r"Content-Length:")
RAW_SOCKET = re.compile(r"(?:TcpListener::bind|UnixStream::connect)")
RAW_STD_PROCESS = re.compile(
    r"std::process::Command::new|use\s+std::process::Command|"
    r"use\s+std::process::\{[^}]*\bCommand\b"
)

# These files model the peer side of MCP framing. They are deliberately local
# fixtures; production-side client framing belongs in McpHarness.
MCP_FRAMING_ALLOWLIST = {
    HARNESS_PATH: "the sole MCP framing implementation",
    "crates/suite-cli/tests/process_harness_e2e.rs": "malformed/framing harness regressions",
    "crates/suite-cli/tests/mcp_proxy_cache_e2e.rs": "inline fake upstream MCP peer",
    "crates/suite-cli/tests/support/mcp_proxy_fake.rs": "fake upstream MCP peers",
}

# These sockets are the behavior under test, not process ownership helpers.
# Their polling loops must retain a mechanically visible elapsed-time bound.
SOCKET_ALLOWLIST = {
    "crates/suite-cli/tests/daemon_lifecycle_e2e.rs": (
        "std::time::Instant::now()",
        "elapsed() < Duration::from_secs",
    ),
    "crates/suite-cli/tests/daemon_lifecycle_disconnect_e2e.rs": (
        "std::time::Instant::now()",
        "elapsed() < Duration::from_secs",
    ),
}


def rust_test_sources(root: Path) -> list[Path]:
    base = root / TEST_ROOT
    if not base.exists():
        return []
    return sorted(base.rglob("*.rs"))


def relative(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def matching_lines(
    root: Path, pattern: re.Pattern[str]
) -> tuple[set[str], list[tuple[str, int]]]:
    files: set[str] = set()
    occurrences: list[tuple[str, int]] = []
    for path in rust_test_sources(root):
        rel = relative(root, path)
        text = path.read_text(encoding="utf-8")
        for match in pattern.finditer(text):
            files.add(rel)
            line_number = text.count("\n", 0, match.start()) + 1
            occurrences.append((rel, line_number))
    return files, occurrences


def audit_repository(root: Path) -> tuple[list[str], dict[str, set[str]]]:
    """Return policy failures and a mechanically derived source inventory."""

    lifecycle_files, lifecycle_lines = matching_lines(root, MANUAL_LIFECYCLE)
    cargo_files, cargo_lines = matching_lines(root, NESTED_CARGO)
    git_files, git_lines = matching_lines(root, DIRECT_GIT)
    cleanup_files, cleanup_lines = matching_lines(root, MANUAL_CLEANUP)
    framing_files, framing_lines = matching_lines(root, RAW_MCP_FRAMING)
    socket_files, socket_lines = matching_lines(root, RAW_SOCKET)
    raw_process_files, _ = matching_lines(root, RAW_STD_PROCESS)

    support_sync_files: set[str] = set()
    support_sync_lines: list[tuple[str, int]] = []
    support_prefix = f"{TEST_ROOT.as_posix()}/support/"
    for path, line_number in matching_lines(root, SUPPORT_SYNC_CHILD)[1]:
        if path.startswith(support_prefix):
            support_sync_files.add(path)
            support_sync_lines.append((path, line_number))

    errors: list[str] = []

    def reject_outside(
        label: str,
        occurrences: list[tuple[str, int]],
        allowed: set[str],
    ) -> None:
        for path, line_number in occurrences:
            if path not in allowed:
                errors.append(
                    f"{path}:{line_number}: {label} must use {HARNESS_PATH}"
                )

    reject_outside(
        "manual child lifecycle",
        lifecycle_lines,
        set(MANUAL_LIFECYCLE_ALLOWLIST),
    )
    reject_outside("nested Cargo build", cargo_lines, {HARNESS_PATH})
    reject_outside("Git fixture process", git_lines, {HARNESS_PATH})
    reject_outside("synchronous support child", support_sync_lines, set())
    reject_outside("manual filesystem cleanup", cleanup_lines, set())
    reject_outside(
        "MCP client framing",
        framing_lines,
        set(MCP_FRAMING_ALLOWLIST),
    )
    reject_outside("raw socket lifecycle", socket_lines, set(SOCKET_ALLOWLIST))

    for path in sorted(raw_process_files.difference({HARNESS_PATH})):
        text = (root / path).read_text(encoding="utf-8")
        if (
            path not in MANUAL_LIFECYCLE_ALLOWLIST
            and "ProcessHarness" not in text
            and "McpHarness" not in text
        ):
            errors.append(
                f"{path}: raw std::process::Command lacks a bounded harness owner"
            )

    for path in sorted(socket_files.intersection(SOCKET_ALLOWLIST)):
        text = (root / path).read_text(encoding="utf-8")
        for marker in SOCKET_ALLOWLIST[path]:
            if marker not in text:
                errors.append(
                    f"{path}: reviewed socket fixture lost deadline marker {marker!r}"
                )

    inventory = {
        "manual_lifecycle": lifecycle_files,
        "nested_cargo": cargo_files,
        "direct_git": git_files,
        "support_sync_child": support_sync_files,
        "manual_cleanup": cleanup_files,
        "mcp_framing": framing_files,
        "raw_socket": socket_files,
        "raw_process_client": raw_process_files,
    }
    return errors, inventory


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="repository root to inspect",
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    errors, inventory = audit_repository(args.root.resolve())
    if errors:
        for error in errors:
            print(f"test harness policy invariant failed: {error}", file=sys.stderr)
        return 1

    print(
        "test harness policy passed "
        f"(lifecycle={len(inventory['manual_lifecycle'])}, "
        f"cargo={len(inventory['nested_cargo'])}, "
        f"git={len(inventory['direct_git'])}, "
        f"bounded_process_clients={len(inventory['raw_process_client'])}, "
        f"peer_framing={len(inventory['mcp_framing'])}, "
        f"local_sockets={len(inventory['raw_socket'])})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
