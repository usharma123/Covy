#!/usr/bin/env python3
"""Run the controlled local instruction-prefix experiment.

This harness deliberately does not synthesize provider-cache observations.
Unavailable provider fields remain explicit ``unknown`` measurements in the
raw records and generated report.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import platform
import shlex
import subprocess
import sys
import tempfile
from typing import Any, Iterable

OUTPUT_SCHEMA = "packet28.instruction_prefix_experiment_artifacts.v1"
DEFAULT_MANIFEST = pathlib.Path(__file__).with_name("instruction_prefix_manifest.json")
DEFAULT_OUTPUT = pathlib.Path(__file__).with_name("20260728")


def run(
    command: list[str],
    *,
    cwd: pathlib.Path,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def git_snapshot_sha256(repo_root: pathlib.Path, excluded: pathlib.Path) -> str:
    digest = hashlib.sha256()
    excluded = excluded.resolve()
    diff_command = ["git", "diff", "--binary", "HEAD", "--", "."]
    try:
        relative_excluded = excluded.relative_to(repo_root.resolve())
    except ValueError:
        relative_excluded = None
    if relative_excluded is not None:
        diff_command.append(f":(exclude){relative_excluded.as_posix()}/**")
    diff = run(diff_command, cwd=repo_root).stdout
    digest.update(diff.encode())
    untracked = run(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"],
        cwd=repo_root,
    ).stdout.split("\0")
    for relative in sorted(name for name in untracked if name):
        path = repo_root / relative
        if path.is_symlink():
            digest.update(relative.encode())
            digest.update(b"\0symlink\0")
            digest.update(os.readlink(path).encode())
            digest.update(b"\0")
            continue
        resolved = path.resolve()
        if (
            resolved == excluded
            or excluded in resolved.parents
            or repo_root.resolve() not in resolved.parents
            or not resolved.is_file()
        ):
            continue
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(resolved.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def memory_bytes() -> int | None:
    if sys.platform == "darwin":
        result = subprocess.run(
            ["sysctl", "-n", "hw.memsize"],
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if result.returncode == 0 and result.stdout.strip().isdigit():
            return int(result.stdout.strip())
    try:
        return int(os.sysconf("SC_PHYS_PAGES")) * int(os.sysconf("SC_PAGE_SIZE"))
    except (AttributeError, OSError, ValueError):
        pass
    meminfo = pathlib.Path("/proc/meminfo")
    if meminfo.exists():
        for line in meminfo.read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1]) * 1024
    return None


def machine_metadata(
    repo_root: pathlib.Path,
    command: list[str],
    output_dir: pathlib.Path,
) -> dict[str, Any]:
    status = run(["git", "status", "--porcelain=v1"], cwd=repo_root).stdout
    branch = run(["git", "branch", "--show-current"], cwd=repo_root).stdout.strip()
    return {
        "schema": OUTPUT_SCHEMA,
        "captured_at_utc": dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat(),
        "git": {
            "head": run(["git", "rev-parse", "HEAD"], cwd=repo_root).stdout.strip(),
            "branch": branch or None,
            "dirty": bool(status),
            "snapshot_sha256": git_snapshot_sha256(repo_root, output_dir),
        },
        "machine": {
            "platform": platform.platform(),
            "system": platform.system(),
            "release": platform.release(),
            "architecture": platform.machine(),
            "processor": platform.processor() or None,
            "logical_cpu_count": os.cpu_count(),
            "memory_bytes": memory_bytes(),
        },
        "toolchain": {
            "rustc": run(["rustc", "-Vv"], cwd=repo_root).stdout.strip(),
            "cargo": run(["cargo", "-V"], cwd=repo_root).stdout.strip(),
            "python": sys.version,
        },
        "command": command,
        "command_display": shlex.join(command),
        "provider_request_executed": False,
        "evidence_boundary": (
            "Only local renderer bytes, render-relevant snapshot hashes, and "
            "Packet28 local cache hits are observed. Provider prompt ordering, "
            "cache boundaries, creation/read tokens, costs, compaction rewarm, "
            "adherence, and net savings are not established."
        ),
    }


def atomic_write(path: pathlib.Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        delete=False,
    ) as handle:
        handle.write(contents)
        temporary = pathlib.Path(handle.name)
    os.replace(temporary, path)


def observed_value(measurement: dict[str, Any]) -> Any | None:
    if measurement.get("state") != "observed":
        return None
    return measurement.get("value")


def render_summary(
    metadata: dict[str, Any],
    result: dict[str, Any],
) -> str:
    records = result["records"]
    rows = []
    for mode in ("passthrough", "stable", "adaptive"):
        selected = [record for record in records if record["mode"] == mode]
        hashes = {record["rendered_prefix_sha256"] for record in selected}
        eligible = sum(record["renderer_cache_eligible"] for record in selected)
        hits = sum(record["renderer_cache_hit"] for record in selected)
        rows.append(
            f"| `{mode}` | {len(selected)} | {eligible} | {hits} | {len(hashes)} |"
        )
    unknown_metrics = []
    for mode, metrics in result["provider_metrics_by_mode"].items():
        for name, measurement in metrics.items():
            if observed_value(measurement) is None:
                unknown_metrics.append(f"- `{mode}.{name}`: {measurement['reason']}")
    assertions = "\n".join(
        f"- {'PASS' if item['passed'] else 'FAIL'} `{item['name']}` — {item['detail']}"
        for item in result["assertions"]
    )
    return (
        "# Instruction-prefix experiment — 2026-07-28\n\n"
        "This is controlled local renderer/cache evidence. It made no provider "
        "request and therefore does not establish provider cache placement, "
        "cache-token savings, price savings, compaction rewarm cost, or model "
        "adherence.\n\n"
        f"- Result: `{'PASS' if result['ok'] else 'FAIL'}`\n"
        f"- Git HEAD: `{metadata['git']['head']}`\n"
        f"- Dirty source snapshot: `{metadata['git']['snapshot_sha256']}`\n"
        f"- Repetitions: `{len(records) // 18}`\n"
        "- Scenarios per mode: cold start, second request, compaction, task "
        "A→B, same-task snapshot drift, fresh-worker handoff\n\n"
        "## Local observations\n\n"
        "| Mode | Requests | Cache-eligible | Local cache hits | Unique rendered-prefix hashes |\n"
        "|---|---:|---:|---:|---:|\n"
        + "\n".join(rows)
        + "\n\n"
        "Stable mode is expected to have one rendered-prefix hash across all "
        "transitions and to miss only on cold start and fresh-worker handoff. "
        "Passthrough and adaptive modes intentionally bypass the local renderer "
        "cache.\n\n"
        "## Mechanically checked invariants\n\n"
        + assertions
        + "\n\n"
        "## Explicitly unknown provider metrics\n\n"
        + "\n".join(unknown_metrics)
        + "\n"
    )


def write_artifacts(
    output_dir: pathlib.Path,
    metadata: dict[str, Any],
    result: dict[str, Any],
) -> None:
    atomic_write(
        output_dir / "metadata.json",
        json.dumps(metadata, indent=2, sort_keys=True) + "\n",
    )
    atomic_write(
        output_dir / "local-results.json",
        json.dumps(result, indent=2, sort_keys=True) + "\n",
    )
    records = "".join(
        json.dumps(record, sort_keys=True) + "\n" for record in result["records"]
    )
    atomic_write(output_dir / "records.jsonl", records)
    atomic_write(output_dir / "summary.md", render_summary(metadata, result))


def parse_args(argv: Iterable[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=pathlib.Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--output-dir", type=pathlib.Path, default=DEFAULT_OUTPUT)
    return parser.parse_args(argv)


def main(argv: Iterable[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root = pathlib.Path(
        run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=pathlib.Path.cwd(),
        ).stdout.strip()
    )
    manifest = args.manifest.resolve()
    output_dir = args.output_dir.resolve()
    try:
        command_manifest = str(manifest.relative_to(repo_root))
    except ValueError:
        command_manifest = str(manifest)
    command = [
        "cargo",
        "run",
        "--locked",
        "--release",
        "-p",
        "context-kernel-core",
        "--example",
        "instruction_prefix_experiment",
        "--",
        "--manifest",
        command_manifest,
    ]
    completed = None
    metadata = None
    for attempt in range(1, 4):
        metadata = machine_metadata(repo_root, command, output_dir)
        completed = run(command, cwd=repo_root, check=False)
        if completed.returncode != 0:
            sys.stderr.write(completed.stderr)
            sys.stderr.write(completed.stdout)
            return completed.returncode
        after_head = run(["git", "rev-parse", "HEAD"], cwd=repo_root).stdout.strip()
        after_snapshot = git_snapshot_sha256(repo_root, output_dir)
        stable = (
            metadata["git"]["head"] == after_head
            and metadata["git"]["snapshot_sha256"] == after_snapshot
        )
        metadata["source_stability"] = {
            "attempt": attempt,
            "head_after": after_head,
            "snapshot_sha256_after": after_snapshot,
            "stable_during_run": stable,
        }
        if stable:
            break
        sys.stderr.write(
            f"source changed during experiment attempt {attempt}; retrying\n"
        )
    assert completed is not None
    assert metadata is not None
    if not metadata["source_stability"]["stable_during_run"]:
        sys.stderr.write("source remained unstable across three attempts\n")
        return 1
    result = json.loads(completed.stdout)
    write_artifacts(output_dir, metadata, result)
    print(output_dir / "summary.md")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
