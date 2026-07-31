#!/usr/bin/env python3
"""Enforce Packet28's unsafe locality and production panic policy."""

from __future__ import annotations

import argparse
from collections import Counter
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
from typing import Iterable, Sequence


ROOT = Path(__file__).resolve().parents[1]

# Unsafe code is deliberately confined to OS/FFI adapters and the harnesses
# that verify them. Adding a path requires an explicit architectural review.
ALLOWED_UNSAFE_FILES = {
    "benchmarks/per-06-shared-scan/src/lib.rs": "allocation-counting benchmark",
    "crates/context-instruct-shim/src/linux.rs": "Linux preload FFI and syscalls",
    "crates/context-instruct-shim/src/macos.rs": "macOS interpose FFI and syscalls",
    "crates/context-memory-core/src/persist.rs": (
        "test-only persistence resource-limit and FIFO probes"
    ),
    "crates/covy-ingest/examples/ingest_allocation_probe.rs": "allocation-counting example",
    "crates/packet28-daemon-client/src/runtime_discovery.rs": (
        "descriptor-relative Unix runtime discovery and ACL/xattr FFI"
    ),
    "crates/packet28-daemon-client/src/transport.rs": (
        "Unix peer-credential verification"
    ),
    "crates/packet28-daemon-core/src/retention/capability.rs": (
        "task-store descriptor, ACL, and xattr capability adapter"
    ),
    "crates/packet28-daemon-core/src/storage.rs": (
        "test-only resident-memory bound instrumentation"
    ),
    "crates/packet28-daemon-protocol/src/paths.rs": (
        "effective-user-specific Unix socket namespace"
    ),
    "crates/packet28-search-core/src/layer.rs": "read-only memory-mapped index layer",
    "crates/packet28-state-fs/src/lib.rs": "test-only FIFO rejection probe",
    "crates/packet28-search-cli/tests/support/daemon.rs": (
        "effective-user-specific Unix socket fallback fixture"
    ),
    "crates/packet28d/src/application.rs": (
        "Unix socket owner and peer-UID authentication"
    ),
    "crates/packet28d/src/launch.rs": "Unix process-group signals",
    "crates/packet28d/src/runtime_files_unix.rs": "retained Unix dirfd capability filesystem operations",
    "crates/packet28d/src/tests/cancellation.rs": "Unix process-group test probe",
    "crates/suite-cli/src/cli_runtime.rs": "Unix stdout descriptor redirection",
    "crates/suite-cli/src/cmd_macos_swap.rs": "macOS process and filesystem syscalls",
    "crates/suite-cli/src/cmd_mcp_artifact_io.rs": (
        "descriptor-relative MCP artifact filesystem adapter"
    ),
    "crates/suite-cli/tests/daemon_lifecycle_e2e.rs": "Unix daemon liveness probe",
    "crates/suite-cli/tests/process_harness_e2e.rs": "Unix process-harness probes",
    "crates/suite-cli/tests/runtime_backend_macos_e2e.rs": "macOS signal regression test",
    "crates/suite-cli/tests/support/process_harness.rs": "Unix process-harness signals",
}

UNSAFE_SYNTAX = re.compile(
    r"(?:#\s*\[\s*unsafe\s*\(|\bunsafe\s+(?:extern|fn|impl|trait)\b|\bunsafe\s*\{)"
)

LINT_OVERRIDE_ATTRIBUTE = re.compile(
    r"#\s*!?\s*\[\s*(?P<kind>allow|expect)\s*\((?P<body>.*?)\)\s*\]",
    re.DOTALL,
)

PANIC_LINTS = (
    "clippy::unwrap_used",
    "clippy::expect_used",
    "clippy::panic",
    "clippy::unimplemented",
    "clippy::todo",
    "clippy::unreachable",
    "clippy::panic_in_result_fn",
)

# These are mechanically proved invariants or build-script fatal paths, not
# fallible runtime operations. The exact multiset prevents new suppressions
# from entering production without an explicit policy review.
REVIEWED_PANIC_EXPECTATIONS = Counter(
    {
        ("crates/buildy-core/src/parse.rs", "clippy::expect_used"): 1,
        ("crates/context-instruct-shim/build.rs", "clippy::expect_used"): 1,
        ("crates/context-instruct-shim/build.rs", "clippy::panic"): 1,
        ("crates/context-scheduler-core/src/runtime.rs", "clippy::expect_used"): 1,
        ("crates/covy-ingest/src/gocov.rs", "clippy::expect_used"): 1,
        ("crates/mapy-core/src/ast.rs", "clippy::expect_used"): 1,
        ("crates/stacky-core/src/parse.rs", "clippy::expect_used"): 1,
        ("crates/suite-cli/build.rs", "clippy::expect_used"): 1,
        ("crates/suite-cli/build.rs", "clippy::panic"): 1,
        ("crates/suite-cli/src/cmd_system/source.rs", "clippy::expect_used"): 1,
        ("crates/suite-cli/src/toml_filters.rs", "clippy::expect_used"): 1,
        ("crates/suite-proxy-core/src/runtime.rs", "clippy::expect_used"): 1,
        ("crates/testy-core/src/pipeline.rs", "clippy::expect_used"): 1,
    }
)

UNSAFE_LINTS = (
    "clippy::undocumented_unsafe_blocks",
    "clippy::missing_safety_doc",
)

IGNORED_SOURCE_DIRECTORIES = frozenset({"target"})


def rust_files_beneath(base: Path) -> Iterable[Path]:
    """Yield Rust files without traversing generated Cargo target trees."""

    for directory, child_directories, file_names in os.walk(base):
        child_directories[:] = sorted(
            name
            for name in child_directories
            if name not in IGNORED_SOURCE_DIRECTORIES
        )
        current = Path(directory)
        for file_name in sorted(file_names):
            if file_name.endswith(".rs"):
                yield current / file_name


def rust_source_files(root: Path) -> Iterable[Path]:
    """Yield checked Rust sources without descending into generated targets."""

    for base_name in ("crates", "benchmarks"):
        base = root / base_name
        if base.exists():
            yield from rust_files_beneath(base)


def production_rust_source_files(root: Path) -> Iterable[Path]:
    """Yield Rust library, binary, and build-script sources."""

    for base_name in ("crates", "benchmarks"):
        base = root / base_name
        if not base.exists():
            continue
        for package in sorted(base.iterdir()):
            if (
                not package.is_dir()
                or package.name in IGNORED_SOURCE_DIRECTORIES
            ):
                continue
            build_script = package / "build.rs"
            if build_script.is_file():
                yield build_script
            source = package / "src"
            if source.exists():
                yield from rust_files_beneath(source)


def unsafe_source_files(root: Path) -> set[str]:
    """Return repository-relative files containing Rust unsafe syntax."""

    found: set[str] = set()
    for path in rust_source_files(root):
        text = path.read_text(encoding="utf-8")
        if any(
            UNSAFE_SYNTAX.search(line)
            for line in text.splitlines()
            if not line.lstrip().startswith("//")
        ):
            found.add(path.relative_to(root).as_posix())
    return found


def unexpected_unsafe_files(root: Path) -> set[str]:
    """Return unsafe-bearing files outside the reviewed locality allowlist."""

    return unsafe_source_files(root).difference(ALLOWED_UNSAFE_FILES)


def stale_unsafe_allowlist_entries(root: Path) -> set[str]:
    """Return reviewed paths that no longer contain unsafe syntax."""

    return set(ALLOWED_UNSAFE_FILES).difference(unsafe_source_files(root))


def panic_override_inventory(
    root: Path,
) -> tuple[Counter[tuple[str, str]], list[str]]:
    """Return production panic-lint expectations and malformed overrides."""

    inventory: Counter[tuple[str, str]] = Counter()
    errors: list[str] = []
    for path in production_rust_source_files(root):
        relative = path.relative_to(root).as_posix()
        text = path.read_text(encoding="utf-8")
        for attribute in LINT_OVERRIDE_ATTRIBUTE.finditer(text):
            kind = attribute.group("kind")
            body = attribute.group("body")
            lints = [
                match.group(0)
                for match in re.finditer(r"clippy::[a-z_]+", body)
                if match.group(0) in PANIC_LINTS
            ]
            if not lints:
                continue
            if kind == "allow":
                errors.append(f"{relative}: production panic lints may not use #[allow]")
                continue
            if not re.search(r"\breason\s*=", body):
                errors.append(f"{relative}: panic-lint #[expect] must include a reason")
            inventory.update((relative, lint) for lint in lints)
    return inventory, errors


def panic_override_errors(root: Path) -> list[str]:
    """Return unreviewed, stale, or malformed production lint overrides."""

    inventory, errors = panic_override_inventory(root)
    unexpected = inventory - REVIEWED_PANIC_EXPECTATIONS
    stale = REVIEWED_PANIC_EXPECTATIONS - inventory
    for (path, lint), count in sorted(unexpected.items()):
        errors.append(f"{path}: {count} unreviewed #[expect] for {lint}")
    for (path, lint), count in sorted(stale.items()):
        errors.append(f"{path}: {count} stale reviewed #[expect] for {lint}")
    return errors


def clippy_commands() -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Build the all-target unsafe and production-only panic lint commands."""

    unsafe_command = (
        "cargo",
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--locked",
        "--no-deps",
        "--",
        *("-D", "warnings"),
        *(part for lint in UNSAFE_LINTS for part in ("-D", lint)),
    )
    panic_command = (
        "cargo",
        "clippy",
        "--workspace",
        "--lib",
        "--bins",
        "--all-features",
        "--locked",
        "--no-deps",
        "--",
        *("-D", "warnings"),
        *(part for lint in PANIC_LINTS for part in ("-D", lint)),
        "-D",
        "unfulfilled_lint_expectations",
    )
    return unsafe_command, panic_command


def run_clippy(root: Path, commands: Sequence[Sequence[str]]) -> None:
    """Run each checked Clippy command, stopping at the first failure."""

    for command in commands:
        subprocess.run(command, cwd=root, check=True)


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--list",
        action="store_true",
        help="print the Clippy commands without running them",
    )
    parser.add_argument(
        "--source-only",
        action="store_true",
        help="check unsafe source locality without running Clippy",
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    commands = clippy_commands()
    if args.list:
        for command in commands:
            print(shlex.join(command))
        return 0

    unsafe_files = unsafe_source_files(ROOT)
    unexpected = sorted(unsafe_files.difference(ALLOWED_UNSAFE_FILES))
    stale = sorted(set(ALLOWED_UNSAFE_FILES).difference(unsafe_files))
    panic_overrides = panic_override_errors(ROOT)
    if unexpected or stale or panic_overrides:
        print(
            "Rust hazard inventory failed; reconcile the reviewed policy:",
            file=sys.stderr,
        )
        for path in unexpected:
            print(f"  unexpected unsafe source: {path}", file=sys.stderr)
        for path in stale:
            print(f"  stale allowlist entry: {path}", file=sys.stderr)
        for error in panic_overrides:
            print(f"  {error}", file=sys.stderr)
        return 1

    if not args.source_only:
        try:
            run_clippy(ROOT, commands)
        except subprocess.CalledProcessError as error:
            return error.returncode

    print(
        "Rust hazard policy passed "
        f"({len(ALLOWED_UNSAFE_FILES)} reviewed unsafe files)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
