#!/usr/bin/env python3
"""Run the PER-10 cold workspace-index benchmark without touching live state."""

from __future__ import annotations

import argparse
import functools
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import stat
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Sequence


HERE = Path(__file__).resolve().parent
REPOSITORY_ROOT = HERE.parents[1]
HISTORICAL_REPORT = REPOSITORY_ROOT / "benchmarks" / "packet28_search_tool_benchmark.md"
DEFAULT_RAW_REPORT = HERE / "current-2026-07-28-v1.json"
DEFAULT_README = HERE / "README.md"
SCHEMA = "packet28.per10.workspace-index.v1"
EXPECTED_HISTORICAL_SHA256 = (
    "b61978a4c3e72aadf72761d9a4abbb9c0e9ff232a9758ce2e41a020b465ef244"
)
MINIMUM_EVIDENCE_ITERATIONS = 3
BUILD_OUTPUT_RE = re.compile(
    r"^build_ms=(?P<build_ms>[0-9]+(?:\.[0-9]+)?) "
    r"generation=(?P<generation>[0-9]+) files=(?P<files>[0-9]+)$"
)
PYTHON_RUNTIME_SUFFIXES = {".pyc", ".pyo"}
REPRODUCIBILITY_ENVIRONMENT_KEYS = (
    "AR",
    "CARGO_BUILD_JOBS",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "CARGO_NET_OFFLINE",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_PROFILE_RELEASE_INCREMENTAL",
    "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    "CARGO_PROFILE_RELEASE_PANIC",
    "CC",
    "CXX",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_TERMINAL_PROMPT",
    "MACOSX_DEPLOYMENT_TARGET",
    "PATH",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "SDKROOT",
)
GIT_CONTEXT_ENVIRONMENT_KEYS = (
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CEILING_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_SYSTEM",
    "GIT_DIR",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_EXEC_PATH",
    "GIT_INDEX_FILE",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_WORK_TREE",
)


@dataclass(frozen=True)
class Snapshot:
    sha256: str
    file_count: int
    byte_count: int
    relative_paths: tuple[Path, ...]
    absent_index_paths: tuple[str, ...]


@dataclass(frozen=True)
class CommandRecord:
    argv: tuple[str, ...]
    resolved_executable: str | None
    cwd: str
    environment: dict[str, str]
    removed_environment: tuple[str, ...]
    exit_code: int
    wall_ms: float
    stdout: str
    stderr: str

    def to_json(self) -> dict[str, object]:
        return {
            "argv": list(self.argv),
            "command": shlex.join(self.argv),
            "resolved_executable": self.resolved_executable,
            "cwd": self.cwd,
            "environment": self.environment,
            "removed_environment": list(self.removed_environment),
            "exit_code": self.exit_code,
            "wall_ms": self.wall_ms,
            "stdout": self.stdout,
            "stderr": self.stderr,
        }


def run_command(
    argv: Sequence[str],
    *,
    cwd: Path,
    environment: dict[str, str] | None = None,
    remove_environment: Iterable[str] = (),
) -> CommandRecord:
    env, recorded_environment, removed_environment = prepare_environment(
        environment=environment,
        remove_environment=remove_environment,
    )
    resolved_executable = shutil.which(str(argv[0]), path=env.get("PATH"))
    started_ns = time.perf_counter_ns()
    completed = subprocess.run(
        list(argv),
        cwd=cwd,
        env=env,
        text=True,
        errors="surrogateescape",
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    wall_ms = (time.perf_counter_ns() - started_ns) / 1_000_000
    return CommandRecord(
        argv=tuple(str(item) for item in argv),
        resolved_executable=resolved_executable,
        cwd=str(cwd),
        environment=recorded_environment,
        removed_environment=removed_environment,
        exit_code=completed.returncode,
        wall_ms=wall_ms,
        stdout=completed.stdout,
        stderr=completed.stderr,
    )


def prepare_environment(
    *,
    environment: dict[str, str] | None = None,
    remove_environment: Iterable[str] = (),
) -> tuple[dict[str, str], dict[str, str], tuple[str, ...]]:
    env = os.environ.copy()
    removed = tuple(sorted(set(remove_environment)))
    for key in removed:
        env.pop(key, None)
    if environment:
        env.update(environment)
    recorded_keys = set(REPRODUCIBILITY_ENVIRONMENT_KEYS)
    if environment:
        recorded_keys.update(environment)
    recorded = {key: env[key] for key in sorted(recorded_keys) if key in env}
    return env, recorded, removed


def isolated_git_environment() -> tuple[dict[str, str], tuple[str, ...]]:
    dynamic_config_keys = {
        key
        for key in os.environ
        if key.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"))
    }
    removed = tuple(sorted(set(GIT_CONTEXT_ENVIRONMENT_KEYS) | dynamic_config_keys))
    return (
        {
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_TERMINAL_PROMPT": "0",
        },
        removed,
    )


def checked_output(
    argv: Sequence[str],
    *,
    cwd: Path,
    environment: dict[str, str] | None = None,
    remove_environment: Iterable[str] = (),
) -> str:
    env, _recorded, _removed = prepare_environment(
        environment=environment,
        remove_environment=remove_environment,
    )
    completed = subprocess.run(
        list(argv),
        cwd=cwd,
        env=env,
        text=True,
        errors="surrogateescape",
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"{shlex.join(argv)} failed with exit code {completed.returncode}: "
            f"{completed.stderr.strip()}"
        )
    return completed.stdout


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_historical_report(path: Path) -> str:
    actual = sha256_file(path)
    if actual != EXPECTED_HISTORICAL_SHA256:
        raise RuntimeError(
            "historical benchmark report digest changed: "
            f"expected {EXPECTED_HISTORICAL_SHA256}, found {actual}"
        )
    return actual


def version_control_visible_paths(
    root: Path,
    *,
    excluded_paths: Iterable[Path] = (),
) -> tuple[Path, ...]:
    excluded = set(excluded_paths)
    for relative in excluded:
        validate_relative_path(relative)
    git_environment, removed_environment = isolated_git_environment()
    env, _recorded, _removed = prepare_environment(
        environment=git_environment,
        remove_environment=removed_environment,
    )
    completed = subprocess.run(
        [
            "git",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        cwd=root,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"git ls-files failed: {stderr}")

    paths: list[Path] = []
    for raw_path in completed.stdout.split(b"\0"):
        if not raw_path:
            continue
        relative = Path(os.fsdecode(raw_path))
        validate_relative_path_shape(relative)
        if is_excluded_state_path(relative):
            continue
        validate_relative_path(relative)
        if relative not in excluded and not is_generated_runtime_path(relative):
            paths.append(relative)
    return tuple(sorted(set(paths), key=lambda path: os.fsencode(path.as_posix())))


def is_generated_runtime_path(relative: Path) -> bool:
    """Return whether a Git-visible path is disposable interpreter state."""

    return (
        "__pycache__" in relative.parts
        or relative.suffix.lower() in PYTHON_RUNTIME_SUFFIXES
    )


def repository_relative_outputs(
    root: Path,
    output_paths: Iterable[Path],
) -> tuple[Path, ...]:
    repository = root.resolve()
    relative_paths = []
    for output in output_paths:
        resolved = output.resolve()
        try:
            relative = resolved.relative_to(repository)
        except ValueError:
            continue
        validate_relative_path(relative)
        relative_paths.append(relative)
    return tuple(sorted(set(relative_paths), key=lambda path: os.fsencode(path.as_posix())))


def validate_relative_path(relative: Path) -> None:
    validate_relative_path_shape(relative)
    if is_excluded_state_path(relative):
        state = next(
            part for part in relative.parts if part in {".git", ".packet28"}
        )
        raise ValueError(f"benchmark snapshot cannot include {state} state")


def validate_relative_path_shape(relative: Path) -> None:
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise ValueError(f"unsafe version-control path: {relative!s}")


def is_excluded_state_path(relative: Path) -> bool:
    return any(part in {".git", ".packet28"} for part in relative.parts)


def relative_path_beneath(root: Path, candidate: Path, *, label: str) -> Path:
    try:
        relative = candidate.resolve(strict=False).relative_to(root.resolve())
    except ValueError as error:
        raise ValueError(f"{label} escaped its snapshot root: {candidate}") from error
    if is_excluded_state_path(relative):
        state = next(
            part for part in relative.parts if part in {".git", ".packet28"}
        )
        raise ValueError(f"{label} resolves into excluded {state} state")
    return relative


def validate_snapshot_symlink(
    *,
    source_root: Path,
    destination_root: Path,
    relative: Path,
    target: str,
) -> None:
    target_path = Path(target)
    if target_path.is_absolute():
        raise ValueError(
            f"absolute symlink is unsupported in the benchmark snapshot: "
            f"{relative.as_posix()} -> {target}"
        )
    relative_path_beneath(
        source_root,
        source_root / relative.parent / target_path,
        label=f"source symlink {relative.as_posix()}",
    )
    relative_path_beneath(
        destination_root,
        destination_root / relative.parent / target_path,
        label=f"snapshot symlink {relative.as_posix()}",
    )


def assert_isolated_path(path: Path, *, temporary_root: Path, workspace: Path) -> None:
    resolved = path.resolve()
    temporary = temporary_root.resolve()
    repository = workspace.resolve()
    if not resolved.is_relative_to(temporary):
        raise ValueError(f"fixture escaped the temporary root: {resolved}")
    if resolved == repository or resolved.is_relative_to(repository):
        raise ValueError(f"fixture overlaps the live workspace: {resolved}")
    if repository.is_relative_to(resolved):
        raise ValueError(f"fixture contains the live workspace: {resolved}")


def copy_worktree_snapshot(
    source_root: Path,
    destination: Path,
    relative_paths: Iterable[Path],
) -> Snapshot:
    destination.mkdir(parents=True, exist_ok=False)
    copied: list[Path] = []
    absent: list[str] = []

    for relative in relative_paths:
        validate_relative_path(relative)
        source = source_root / relative
        target = destination / relative
        if not source.exists() and not source.is_symlink():
            # A tracked path deleted in the working tree remains in git ls-files.
            absent.append(relative.as_posix())
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_symlink():
            link_target = os.readlink(source)
            validate_snapshot_symlink(
                source_root=source_root,
                destination_root=destination,
                relative=relative,
                target=link_target,
            )
            os.symlink(link_target, target)
        elif source.is_file():
            shutil.copy2(source, target)
        elif source.is_dir():
            raise ValueError(
                f"submodule or directory entry is unsupported in the benchmark snapshot: "
                f"{relative.as_posix()}"
            )
        else:
            raise ValueError(
                f"special filesystem entry is unsupported in the benchmark snapshot: "
                f"{relative.as_posix()}"
            )
        copied.append(relative)

    if any(is_excluded_state_path(relative) for relative in copied):
        raise AssertionError("snapshot unexpectedly contains excluded repository state")
    sha256, byte_count = hash_snapshot(destination, copied)
    return Snapshot(
        sha256=sha256,
        file_count=len(copied),
        byte_count=byte_count,
        relative_paths=tuple(copied),
        absent_index_paths=tuple(absent),
    )


def existing_paths(root: Path, relative_paths: Iterable[Path]) -> tuple[Path, ...]:
    return tuple(
        relative
        for relative in relative_paths
        if (root / relative).exists() or (root / relative).is_symlink()
    )


def snapshot_matches_live_worktree(
    source_root: Path,
    snapshot: Snapshot,
    current_paths: Iterable[Path],
) -> tuple[bool, str]:
    live_paths = existing_paths(source_root, current_paths)
    if live_paths != snapshot.relative_paths:
        return False, "version-control-visible path set changed during capture"
    try:
        live_sha256, live_bytes = hash_snapshot(source_root, live_paths)
    except OSError as error:
        return False, f"worktree changed while validating capture: {error}"
    if (live_sha256, live_bytes) != (snapshot.sha256, snapshot.byte_count):
        return False, "worktree file content or mode changed during capture"
    return True, "source paths, bytes, modes, HEAD, and status were stable"


def hash_snapshot(root: Path, relative_paths: Iterable[Path]) -> tuple[str, int]:
    digest = hashlib.sha256()
    byte_count = 0
    for relative in relative_paths:
        validate_relative_path(relative)
        path = root / relative
        relative_bytes = os.fsencode(relative.as_posix())
        metadata = path.lstat()
        digest.update(len(relative_bytes).to_bytes(8, "big"))
        digest.update(relative_bytes)
        digest.update(stat.S_IMODE(metadata.st_mode).to_bytes(4, "big"))
        if path.is_symlink():
            target = os.fsencode(os.readlink(path))
            digest.update(b"L")
            digest.update(len(target).to_bytes(8, "big"))
            digest.update(target)
            byte_count += len(target)
            continue
        digest.update(b"F")
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
                byte_count += len(chunk)
    return digest.hexdigest(), byte_count


def initialize_ephemeral_git_repository(root: Path) -> dict[str, object]:
    git_environment, removed_environment = isolated_git_environment()
    commands = [
        [
            "git",
            "-c",
            "init.defaultBranch=benchmark-snapshot",
            "init",
            "--template=",
            "-q",
        ],
        [
            "git",
            "-c",
            f"core.hooksPath={os.devnull}",
            "-c",
            "user.name=Packet28 benchmark",
            "-c",
            "user.email=benchmark@invalid.example",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "--no-verify",
            "-q",
            "-m",
            "benchmark snapshot",
        ],
    ]
    environment = {
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }
    records = []
    for argv in commands:
        record = run_command(
            argv,
            cwd=root,
            environment={**git_environment, **environment},
            remove_environment=removed_environment,
        )
        records.append(record.to_json())
        if record.exit_code != 0:
            raise RuntimeError(
                f"failed to initialize isolated Git metadata: {record.stderr.strip()}"
            )
    commit = checked_output(
        ["git", "rev-parse", "HEAD"],
        cwd=root,
        environment=git_environment,
        remove_environment=removed_environment,
    ).strip()
    return {
        "purpose": (
            "temporary empty commit makes git-aware traversal and commit capture "
            "deterministic without copying or linking the user's .git directory"
        ),
        "commit": commit,
        "commands": records,
    }


def parse_build_output(stdout: str) -> dict[str, object]:
    nonempty = [line.strip() for line in stdout.splitlines() if line.strip()]
    if len(nonempty) != 1:
        raise ValueError(f"expected one p28 build output line, got {nonempty!r}")
    match = BUILD_OUTPUT_RE.fullmatch(nonempty[0])
    if match is None:
        raise ValueError(f"unexpected p28 build output: {nonempty[0]!r}")
    return {
        "build_ms": float(match.group("build_ms")),
        "generation": int(match.group("generation")),
        "files": int(match.group("files")),
    }


def summarize_runs(runs: Sequence[dict[str, object]]) -> dict[str, object] | None:
    required = {"build_ms", "wall_ms", "files", "generation"}
    if not runs or any(not required.issubset(run) for run in runs):
        return None
    build_values = [float(run["build_ms"]) for run in runs]
    wall_values = [float(run["wall_ms"]) for run in runs]
    files = {int(run["files"]) for run in runs}
    generations = {int(run["generation"]) for run in runs}
    return {
        "iterations": len(runs),
        "build_ms": {
            "min": min(build_values),
            "median": statistics.median(build_values),
            "max": max(build_values),
        },
        "wall_ms": {
            "min": min(wall_values),
            "median": statistics.median(wall_values),
            "max": max(wall_values),
        },
        "indexed_files_consistent": len(files) == 1,
        "indexed_files": sorted(files),
        "generations_consistent": len(generations) == 1,
        "generations": sorted(generations),
    }


def validate_iterations(iterations: int) -> None:
    if iterations < MINIMUM_EVIDENCE_ITERATIONS:
        raise ValueError(
            "evidence runs require at least "
            f"{MINIMUM_EVIDENCE_ITERATIONS} fresh cold fixtures"
        )


def source_input_sha256(snapshot_root: Path) -> dict[str, str]:
    required = (
        Path("Cargo.toml"),
        Path("Cargo.lock"),
        Path("benchmarks/per-10-workspace-index/run.py"),
    )
    identities: dict[str, str] = {}
    for relative in required:
        path = snapshot_root / relative
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(
                f"frozen source snapshot is missing regular input {relative.as_posix()}"
            )
        identities[relative.as_posix()] = sha256_file(path)
    return identities


def command_version(argv: Sequence[str], *, cwd: Path) -> dict[str, object]:
    try:
        record = run_command(argv, cwd=cwd)
    except OSError as error:
        return {
            "argv": list(argv),
            "command": shlex.join(argv),
            "available": False,
            "error": str(error),
        }
    return {
        "argv": list(argv),
        "command": shlex.join(argv),
        "resolved_executable": record.resolved_executable,
        "available": record.exit_code == 0,
        "exit_code": record.exit_code,
        "stdout": record.stdout.strip(),
        "stderr": record.stderr.strip(),
    }


def total_memory_bytes() -> int | None:
    if sys.platform == "darwin":
        try:
            return int(
                checked_output(["sysctl", "-n", "hw.memsize"], cwd=REPOSITORY_ROOT).strip()
            )
        except (OSError, RuntimeError, ValueError):
            hardware = mac_hardware_metadata()
            memory = hardware.get("memory")
            if isinstance(memory, str):
                match = re.fullmatch(
                    r"(?P<amount>[0-9]+(?:\.[0-9]+)?)\s*"
                    r"(?P<unit>KB|MB|GB|TB)",
                    memory.strip(),
                    flags=re.IGNORECASE,
                )
                if match is not None:
                    unit = match.group("unit").upper()
                    multiplier = {
                        "KB": 1024,
                        "MB": 1024**2,
                        "GB": 1024**3,
                        "TB": 1024**4,
                    }[unit]
                    return int(float(match.group("amount")) * multiplier)
            return None
    try:
        page_size = os.sysconf("SC_PAGE_SIZE")
        physical_pages = os.sysconf("SC_PHYS_PAGES")
        return int(page_size * physical_pages)
    except (AttributeError, OSError, TypeError, ValueError):
        return None


def cpu_model() -> str | None:
    if sys.platform == "darwin":
        for key in ("machdep.cpu.brand_string", "hw.model"):
            try:
                value = checked_output(["sysctl", "-n", key], cwd=REPOSITORY_ROOT).strip()
            except (OSError, RuntimeError):
                continue
            if value:
                return value
        chip = mac_hardware_metadata().get("chip")
        if isinstance(chip, str) and chip:
            return chip
    if sys.platform.startswith("linux"):
        try:
            for line in Path("/proc/cpuinfo").read_text(
                encoding="utf-8", errors="replace"
            ).splitlines():
                if line.lower().startswith(("model name", "hardware")):
                    return line.split(":", 1)[-1].strip()
        except OSError:
            pass
    return platform.processor() or None


@functools.lru_cache(maxsize=1)
def mac_hardware_metadata() -> dict[str, str]:
    if sys.platform != "darwin":
        return {}
    try:
        output = checked_output(
            ["system_profiler", "SPHardwareDataType"], cwd=REPOSITORY_ROOT
        )
    except (OSError, RuntimeError):
        return {}
    allowed = {
        "Model Name": "model_name",
        "Model Identifier": "model_identifier",
        "Chip": "chip",
        "Total Number of Cores": "cores",
        "Memory": "memory",
    }
    metadata: dict[str, str] = {}
    for raw_line in output.splitlines():
        key, separator, value = raw_line.strip().partition(":")
        if separator and key in allowed and value.strip():
            metadata[allowed[key]] = value.strip()
    return metadata


def machine_metadata() -> dict[str, object]:
    uname = platform.uname()
    return {
        "operating_system": platform.system(),
        "os_release": platform.release(),
        "os_version": platform.version(),
        "architecture": platform.machine(),
        "cpu_model": cpu_model(),
        "logical_cpu_count": os.cpu_count(),
        "total_memory_bytes": total_memory_bytes(),
        "hardware": mac_hardware_metadata() if sys.platform == "darwin" else {},
        "python": {
            "version": platform.python_version(),
            "implementation": platform.python_implementation(),
        },
        "uname": {
            "system": uname.system,
            "release": uname.release,
            "version": uname.version,
            "machine": uname.machine,
        },
    }


def git_metadata(root: Path) -> dict[str, object]:
    git_environment, removed_environment = isolated_git_environment()
    head = checked_output(
        ["git", "rev-parse", "HEAD"],
        cwd=root,
        environment=git_environment,
        remove_environment=removed_environment,
    ).strip()
    branch = checked_output(
        ["git", "branch", "--show-current"],
        cwd=root,
        environment=git_environment,
        remove_environment=removed_environment,
    ).strip()
    status = checked_output(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=root,
        environment=git_environment,
        remove_environment=removed_environment,
    )
    status_lines = [line for line in status.splitlines() if line]
    return {
        "head_commit": head,
        "branch": branch or None,
        "dirty": bool(status_lines),
        "status_entry_count": len(status_lines),
        "status_porcelain_v1": status_lines,
        "status_sha256": hashlib.sha256(
            status.encode("utf-8", errors="surrogateescape")
        ).hexdigest(),
    }


def historical_evidence(source_sha256: str) -> dict[str, object]:
    return {
        "recorded_at": "2026-03-31T13:56:32.170705+00:00",
        "workspace_index_build_ms": 10375.754,
        "p28_git": "59e54fb",
        "packet28d_version": "0.2.39",
        "source": "benchmarks/packet28_search_tool_benchmark.md",
        "source_sha256": source_sha256,
        "comparability": (
            "historical evidence only; it was not rerun at the current source "
            "snapshot and must not be used to claim a current speedup or regression"
        ),
    }


def benchmark_document(
    *,
    status: str,
    generated_at: str,
    git: dict[str, object],
    snapshot: Snapshot,
    machine: dict[str, object],
    toolchain: dict[str, object],
    historical: dict[str, object],
    build: CommandRecord,
    binary_sha256: str | None,
    fixture_git: dict[str, object] | None,
    runs: list[dict[str, object]],
    blocker: dict[str, object] | None,
    historical_unchanged: bool,
    snapshot_capture: dict[str, object],
    input_sha256: dict[str, str],
) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "benchmark": "PER-10 cold full workspace index build",
        "status": status,
        "generated_at_utc": generated_at,
        "evidence_boundary": {
            "current_result": (
                "machine-local measurement of the exact frozen dirty source snapshot"
            ),
            "historical_result": historical["comparability"],
            "cross_machine_ratio_reported": False,
        },
        "source": {
            "git": git,
            "snapshot_sha256": snapshot.sha256,
            "snapshot_file_count": snapshot.file_count,
            "snapshot_bytes": snapshot.byte_count,
            "tracked_paths_absent_from_worktree": list(snapshot.absent_index_paths),
            "selection": "git ls-files --cached --others --exclude-standard -z",
            "input_sha256": input_sha256,
            "excluded_state": [
                ".git/",
                "every .packet28/ subtree",
                "target/ and other ignored files",
                "Python __pycache__/ and *.py[co] runtime artifacts",
            ],
            "capture": snapshot_capture,
        },
        "isolation": {
            "frozen_source_snapshot": True,
            "fresh_fixture_per_iteration": True,
            "fixture_created_under_os_temporary_directory": True,
            "live_workspace_passed_to_p28": False,
            "live_packet28_state_read_or_written": False,
            "temporary_fixture_packet28_state_removed_by_temporary_directory_cleanup": True,
            "historical_report_unchanged": historical_unchanged,
            "fixture_git": fixture_git,
        },
        "method": {
            "warmups": 0,
            "iterations_requested": len(runs) if status == "complete" else None,
            "index_kind": "cold full build",
            "internal_timer": "p28 build_ms",
            "external_timer": "Python time.perf_counter_ns around process execution",
            "build_graph_locked": "--locked" in build.argv,
        },
        "machine": machine,
        "toolchain": toolchain,
        "historical": historical,
        "build": {
            **build.to_json(),
            "binary_sha256": binary_sha256,
        },
        "runs": runs,
        "summary": summarize_runs(runs),
        "blocker": blocker,
    }


def execute_benchmark(
    *,
    repository_root: Path,
    iterations: int,
    excluded_output_paths: Iterable[Path] = (),
) -> tuple[dict[str, object], int]:
    validate_iterations(iterations)

    generated_at = datetime.now(timezone.utc).isoformat()
    machine = machine_metadata()
    toolchain = {
        "rustc": command_version(["rustc", "-Vv"], cwd=repository_root),
        "cargo": command_version(["cargo", "-V"], cwd=repository_root),
        "git": command_version(["git", "--version"], cwd=repository_root),
    }
    historical_before = validate_historical_report(HISTORICAL_REPORT)
    historical = historical_evidence(historical_before)
    excluded_output_paths = tuple(excluded_output_paths)

    with tempfile.TemporaryDirectory(prefix="packet28-per10-") as temporary:
        temporary_root = Path(temporary)
        source_snapshot: Path | None = None
        snapshot: Snapshot | None = None
        git: dict[str, object] | None = None
        snapshot_capture: dict[str, object] | None = None
        capture_attempts: list[dict[str, object]] = []
        for attempt in range(1, 4):
            candidate = temporary_root / f"source-attempt-{attempt}"
            assert_isolated_path(
                candidate,
                temporary_root=temporary_root,
                workspace=repository_root,
            )
            capture_start = git_metadata(repository_root)
            paths = version_control_visible_paths(
                repository_root,
                excluded_paths=excluded_output_paths,
            )
            candidate_snapshot = copy_worktree_snapshot(
                repository_root, candidate, paths
            )
            end_paths = version_control_visible_paths(
                repository_root,
                excluded_paths=excluded_output_paths,
            )
            capture_end = git_metadata(repository_root)
            matches_live, reason = snapshot_matches_live_worktree(
                repository_root, candidate_snapshot, end_paths
            )
            stable = (
                matches_live
                and capture_start["head_commit"] == capture_end["head_commit"]
                and capture_start["status_sha256"] == capture_end["status_sha256"]
            )
            capture_attempts.append(
                {
                    "attempt": attempt,
                    "stable": stable,
                    "reason": reason,
                    "start_head_commit": capture_start["head_commit"],
                    "end_head_commit": capture_end["head_commit"],
                    "start_status_sha256": capture_start["status_sha256"],
                    "end_status_sha256": capture_end["status_sha256"],
                    "snapshot_sha256": candidate_snapshot.sha256,
                }
            )
            if stable:
                source_snapshot = candidate
                snapshot = candidate_snapshot
                git = capture_start
                snapshot_capture = {
                    "stable": True,
                    "selected_attempt": attempt,
                    "generated_output_paths_excluded": [
                        path.as_posix() for path in excluded_output_paths
                    ],
                    "attempts": capture_attempts,
                }
                break
        if source_snapshot is None or snapshot is None or git is None:
            raise RuntimeError(
                "worktree changed during three consecutive frozen-snapshot attempts"
            )
        assert snapshot_capture is not None
        input_sha256 = source_input_sha256(source_snapshot)

        build_target = temporary_root / "cargo-target"
        assert_isolated_path(
            build_target,
            temporary_root=temporary_root,
            workspace=repository_root,
        )
        git_environment, removed_git_environment = isolated_git_environment()
        build_environment = {
            **git_environment,
            "CARGO_TARGET_DIR": str(build_target),
        }
        build_argv = [
            "cargo",
            "build",
            "--quiet",
            "--release",
            "--locked",
            "-p",
            "packet28-search-cli",
        ]
        build = run_command(
            build_argv,
            cwd=source_snapshot,
            environment=build_environment,
            remove_environment=removed_git_environment,
        )
        historical_after_build = validate_historical_report(HISTORICAL_REPORT)
        historical_unchanged = historical_before == historical_after_build
        if not historical_unchanged:
            raise RuntimeError("historical benchmark report changed during PER-10")

        if build.exit_code != 0:
            document = benchmark_document(
                status="blocked",
                generated_at=generated_at,
                git=git,
                snapshot=snapshot,
                machine=machine,
                toolchain=toolchain,
                historical=historical,
                build=build,
                binary_sha256=None,
                fixture_git=None,
                runs=[],
                blocker={
                    "kind": "locked_release_build_failed",
                    "message": (
                        "cargo build --locked failed in the frozen temporary "
                        "snapshot; Cargo.lock was not modified"
                    ),
                    "exit_code": build.exit_code,
                },
                historical_unchanged=historical_unchanged,
                snapshot_capture=snapshot_capture,
                input_sha256=input_sha256,
            )
            return document, 1

        post_build_sha256, post_build_bytes = hash_snapshot(
            source_snapshot, snapshot.relative_paths
        )
        if (post_build_sha256, post_build_bytes) != (
            snapshot.sha256,
            snapshot.byte_count,
        ):
            raise RuntimeError("locked release build modified the frozen source snapshot")

        binary = build_target / "release" / "p28"
        if not binary.is_file():
            raise RuntimeError(f"cargo succeeded without producing {binary}")
        binary_sha256 = sha256_file(binary)

        fixture_template = temporary_root / "fixture-template"
        assert_isolated_path(
            fixture_template,
            temporary_root=temporary_root,
            workspace=repository_root,
        )
        shutil.copytree(source_snapshot, fixture_template, symlinks=True)
        fixture_git = initialize_ephemeral_git_repository(fixture_template)

        runs: list[dict[str, object]] = []
        benchmark_failure: dict[str, object] | None = None
        for run_number in range(1, iterations + 1):
            fixture = temporary_root / "runs" / f"run-{run_number:02d}"
            fixture.parent.mkdir(parents=True, exist_ok=True)
            assert_isolated_path(
                fixture,
                temporary_root=temporary_root,
                workspace=repository_root,
            )
            shutil.copytree(fixture_template, fixture, symlinks=True)
            if (fixture / ".packet28").exists():
                raise AssertionError("cold fixture unexpectedly has a .packet28 directory")

            argv = [str(binary), "debug", "build", str(fixture)]
            command = run_command(
                argv,
                cwd=fixture,
                environment=git_environment,
                remove_environment=removed_git_environment,
            )
            raw_run: dict[str, object] = {
                "iteration": run_number,
                "argv": list(command.argv),
                "command": shlex.join(command.argv),
                "cwd": command.cwd,
                "exit_code": command.exit_code,
                "wall_ms": command.wall_ms,
                "stdout": command.stdout,
                "stderr": command.stderr,
                "fixture_was_cold": True,
                "fixture_was_isolated": True,
            }
            if command.exit_code == 0:
                try:
                    raw_run.update(parse_build_output(command.stdout))
                except ValueError as error:
                    raw_run["parse_error"] = str(error)
                    benchmark_failure = {
                        "kind": "unexpected_p28_output",
                        "message": str(error),
                        "iteration": run_number,
                    }
            else:
                benchmark_failure = {
                    "kind": "p28_debug_build_failed",
                    "message": (
                        f"p28 debug build failed on cold iteration {run_number}"
                    ),
                    "iteration": run_number,
                    "exit_code": command.exit_code,
                }
            runs.append(raw_run)
            if benchmark_failure is not None:
                break

        historical_after = validate_historical_report(HISTORICAL_REPORT)
        historical_unchanged = historical_before == historical_after
        if not historical_unchanged:
            raise RuntimeError("historical benchmark report changed during PER-10")

        status = "complete" if benchmark_failure is None else "failed"
        document = benchmark_document(
            status=status,
            generated_at=generated_at,
            git=git,
            snapshot=snapshot,
            machine=machine,
            toolchain=toolchain,
            historical=historical,
            build=build,
            binary_sha256=binary_sha256,
            fixture_git=fixture_git,
            runs=runs,
            blocker=benchmark_failure,
            historical_unchanged=historical_unchanged,
            snapshot_capture=snapshot_capture,
            input_sha256=input_sha256,
        )
        document["method"]["iterations_requested"] = iterations
        return document, 0 if status == "complete" else 1


def markdown_code_block(value: str) -> str:
    return f"```text\n{value.rstrip()}\n```"


def render_readme(document: dict[str, object]) -> str:
    historical = document["historical"]
    source = document["source"]
    git = source["git"]
    status = document["status"]
    hardened_identity = bool(source.get("input_sha256"))
    if hardened_identity:
        capture_method = (
            "- Select the version-control-visible worktree with "
            "`git ls-files --cached --others --exclude-standard -z`, so tracked "
            "modifications and non-ignored untracked source are captured. The "
            "generated JSON and Markdown outputs are excluded so a prior report "
            "cannot perturb the next source identity. Disposable Python "
            "`__pycache__` and `*.py[co]` runtime artifacts are also excluded. "
            "Snapshot symlinks are accepted only when their relative targets remain "
            "inside both the live source root and frozen snapshot."
        )
        copy_method = (
            "- Copy those files once to a frozen OS-temporary source snapshot. "
            "The real `.git`, every `.packet28` subtree, `target`, and ignored user "
            "state are never copied or passed to p28."
        )
        build_method = (
            "- Build `packet28-search-cli` from that frozen snapshot with the "
            "checked-in lock graph. `CARGO_TARGET_DIR` is another temporary path. "
            "The report hashes `Cargo.toml`, `Cargo.lock`, and this harness."
        )
        timing_method = (
            "- Record both p28's internal `build_ms` and external process wall time. "
            "Fixture creation is deliberately outside both timers. Executable paths "
            "and the allowlisted build-affecting environment are recorded."
        )
        raw_identity = (
            "Raw source, host, toolchain, resolved executable, effective command "
            "environment, stdout/stderr, and per-iteration records are in "
            "[`current-2026-07-28-v1.json`](current-2026-07-28-v1.json)."
        )
        safety_lines = [
            "- Inherited Git repository/configuration selectors are removed for snapshot, "
            "fixture, build, and p28 commands.",
            "- The historical benchmark file must match its pinned digest before the run, "
            "is hashed afterward, and the harness aborts if either check fails.",
            "- Snapshot symlinks cannot be absolute or resolve outside the frozen source.",
        ]
    else:
        capture_method = (
            "- Select the version-control-visible worktree with "
            "`git ls-files --cached --others --exclude-standard -z`, so tracked "
            "modifications and non-ignored untracked source are captured. Generated "
            "outputs and disposable Python runtime artifacts are excluded."
        )
        copy_method = (
            "- Copy those files once to a frozen OS-temporary source snapshot. The "
            "repository-root `.git`, `.packet28`, `target`, and ignored user state are "
            "not copied or passed to p28."
        )
        build_method = (
            "- Build `packet28-search-cli` from that frozen snapshot with the checked-in "
            "lock graph and a temporary `CARGO_TARGET_DIR`."
        )
        timing_method = (
            "- Record both p28's internal `build_ms` and external process wall time. "
            "Fixture creation is deliberately outside both timers."
        )
        raw_identity = (
            "Raw source, host, toolchain, command, stdout/stderr, and per-iteration "
            "records are in "
            "[`current-2026-07-28-v1.json`](current-2026-07-28-v1.json)."
        )
        safety_lines = [
            "- This preliminary result predates the current runner's pinned historical "
            "digest, nested `.packet28` exclusion, Git-environment isolation, and "
            "symlink-boundary checks; a final integrated-HEAD rerun is required.",
            "- The historical benchmark file was hashed before and after the preliminary "
            "run and remained unchanged.",
        ]
    lines = [
        "# PER-10 current workspace-index benchmark",
        "",
        (
            "This is a controlled, machine-local cold-build measurement of the current "
            "source snapshot. It preserves the March 2026 result as historical evidence "
            "and does not treat it as a directly comparable baseline."
        ),
        (
            "A result is final-tree evidence only when its recorded HEAD and snapshot "
            "identity match the integrated source being handed off."
        ),
        "",
        "## Evidence boundary",
        "",
        "| Evidence | Source | Result | Interpretation |",
        "| --- | --- | ---: | --- |",
        (
            f"| Historical | p28 `{historical['p28_git']}` / packet28d "
            f"`{historical['packet28d_version']}` on 2026-03-31 | "
            f"{historical['workspace_index_build_ms']:,.3f} ms | Historical only; "
            "different source, harness, and potentially machine/toolchain. |"
        ),
    ]

    if status == "complete":
        summary = document["summary"]
        lines.append(
            f"| Current frozen snapshot | HEAD `{git['head_commit'][:12]}` "
            f"(`dirty={str(git['dirty']).lower()}`), snapshot "
            f"`{source['snapshot_sha256'][:16]}…` | "
            f"{summary['build_ms']['median']:,.3f} ms median | Current measurement; "
            "no cross-environment speedup ratio is claimed. |"
        )
    else:
        lines.append(
            "| Current frozen snapshot | Locked release build or benchmark did not "
            f"complete | unavailable | Status: `{status}`; see raw JSON blocker. |"
        )

    lines.extend(
        [
            "",
            "The historical `10,375.754 ms` row remains unchanged in "
            "[the original benchmark](../packet28_search_tool_benchmark.md).",
            "",
            "## Method",
            "",
            capture_method,
            copy_method,
            build_method,
            (
                "- Clone a fresh fixture for every iteration, add an empty deterministic "
                "temporary Git commit for git-aware traversal, and time "
                "`p28 debug build` against that fixture."
            ),
            timing_method,
            "",
            "## Reproduce",
            "",
            markdown_code_block(
                "python3 benchmarks/per-10-workspace-index/run.py --iterations 3"
            ),
            "",
            "The harness executes these command shapes:",
            "",
            markdown_code_block(
                "CARGO_TARGET_DIR=<temporary-cargo-target> "
                "cargo build --quiet --release --locked -p packet28-search-cli\n"
                "<temporary-cargo-target>/release/p28 debug build "
                "<fresh-temporary-fixture>"
            ),
            "",
            raw_identity,
        ]
    )

    if status == "complete":
        lines.extend(
            [
                "",
                "## Current results",
                "",
                "| Run | p28 build_ms | Wall ms | Generation | Indexed files |",
                "| ---: | ---: | ---: | ---: | ---: |",
            ]
        )
        for run in document["runs"]:
            lines.append(
                f"| {run['iteration']} | {run['build_ms']:.3f} | "
                f"{run['wall_ms']:.3f} | {run['generation']} | {run['files']} |"
            )
        summary = document["summary"]
        lines.extend(
            [
                "",
                (
                    f"Median internal build time: **{summary['build_ms']['median']:.3f} "
                    f"ms** (min {summary['build_ms']['min']:.3f}, "
                    f"max {summary['build_ms']['max']:.3f})."
                ),
                (
                    f"Median external wall time: **{summary['wall_ms']['median']:.3f} "
                    f"ms** (min {summary['wall_ms']['min']:.3f}, "
                    f"max {summary['wall_ms']['max']:.3f})."
                ),
            ]
        )
    else:
        blocker = document.get("blocker") or {}
        build = document["build"]
        lines.extend(
            [
                "",
                "## Current blocker",
                "",
                f"`{blocker.get('kind', 'unknown')}`: {blocker.get('message', '')}",
            ]
        )
        if build.get("stderr"):
            lines.extend(
                [
                    "",
                    "Build stderr:",
                    "",
                    markdown_code_block(str(build["stderr"])),
                ]
            )

    lines.extend(
        [
            "",
            "## Safety invariants",
            "",
            "- Every fixture path is mechanically required to be beneath the harness-owned "
            "temporary directory and outside the live repository.",
            "- Every p28 invocation receives only a fresh temporary fixture path.",
            *safety_lines,
            "- Temporary `.packet28` indexes disappear with the temporary directory.",
            "",
        ]
    )
    return "\n".join(lines)


def write_text_atomic(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        delete=False,
    ) as handle:
        temporary = Path(handle.name)
        handle.write(text)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)


def write_reports(
    document: dict[str, object],
    *,
    raw_path: Path,
    readme_path: Path,
) -> None:
    raw = json.dumps(document, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    write_text_atomic(raw_path, raw)
    write_text_atomic(readme_path, render_readme(document))


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the isolated PER-10 current workspace-index benchmark."
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=3,
        help="number of fresh cold fixtures to measure (default: 3)",
    )
    parser.add_argument(
        "--raw-output",
        type=Path,
        default=DEFAULT_RAW_REPORT,
        help=f"versioned raw JSON output (default: {DEFAULT_RAW_REPORT})",
    )
    parser.add_argument(
        "--readme-output",
        type=Path,
        default=DEFAULT_README,
        help=f"generated Markdown report (default: {DEFAULT_README})",
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        excluded_output_paths = repository_relative_outputs(
            REPOSITORY_ROOT,
            (args.raw_output, args.readme_output),
        )
        document, exit_code = execute_benchmark(
            repository_root=REPOSITORY_ROOT,
            iterations=args.iterations,
            excluded_output_paths=excluded_output_paths,
        )
        write_reports(
            document,
            raw_path=args.raw_output,
            readme_path=args.readme_output,
        )
    except (OSError, RuntimeError, ValueError) as error:
        print(f"PER-10 benchmark failed safely: {error}", file=sys.stderr)
        return 2

    print(
        f"PER-10 status={document['status']} "
        f"raw={args.raw_output} report={args.readme_output}"
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
