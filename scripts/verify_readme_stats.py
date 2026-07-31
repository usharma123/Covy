#!/usr/bin/env python3
"""Generate or verify README repository statistics from tracked source."""

from __future__ import annotations

import argparse
import difflib
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
README = ROOT / "README.md"
START = "<!-- BEGIN GENERATED PROJECT STATS -->"
END = "<!-- END GENERATED PROJECT STATS -->"
ARCHITECTURE = re.compile(
    r"Packet28 is a Rust workspace of \d+ crates organized into four layers:"
)


@dataclass(frozen=True)
class Stats:
    crates: int
    rust_files: int
    rust_lines: int
    binary_targets: int


def run(*args: str) -> str:
    return subprocess.run(
        args,
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout


def collect() -> Stats:
    metadata = json.loads(
        run(
            "cargo",
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        )
    )
    rust_paths = {
        ROOT / line
        for line in run(
            "git",
            "ls-files",
            "--cached",
            "--",
            "*.rs",
        ).splitlines()
        if line
    }
    existing = sorted(path for path in rust_paths if path.is_file())
    rust_lines = sum(len(path.read_bytes().splitlines()) for path in existing)
    binary_targets = sum(
        "bin" in target["kind"]
        for package in metadata["packages"]
        for target in package["targets"]
    )
    return Stats(
        crates=len(metadata["workspace_members"]),
        rust_files=len(existing),
        rust_lines=rust_lines,
        binary_targets=binary_targets,
    )


def stats_block(stats: Stats) -> str:
    return "\n".join(
        [
            START,
            f"- {stats.rust_lines:,} lines across {stats.rust_files} Rust files",
            f"- {stats.crates} crates in the workspace",
            (
                f"- {stats.binary_targets} Cargo binary targets "
                "(including one internal generator)"
            ),
            END,
        ]
    )


def updated_readme(current: str, stats: Stats) -> str:
    expected_architecture = (
        f"Packet28 is a Rust workspace of {stats.crates} crates "
        "organized into four layers:"
    )
    updated, architecture_count = ARCHITECTURE.subn(expected_architecture, current)
    if architecture_count != 1:
        raise ValueError("README architecture crate-count sentence was not found once")

    block = stats_block(stats)
    if START in updated and END in updated:
        pattern = re.compile(
            rf"{re.escape(START)}.*?{re.escape(END)}", re.DOTALL
        )
        updated, count = pattern.subn(block, updated)
        if count != 1:
            raise ValueError("README generated stats markers were not found once")
        return updated

    project_stats = re.compile(r"(?ms)^## Project Stats\n\n.*\Z")
    updated, count = project_stats.subn(f"## Project Stats\n\n{block}\n", updated)
    if count != 1:
        raise ValueError("README Project Stats section was not found once")
    return updated


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    args = parser.parse_args()

    try:
        current = README.read_text(encoding="utf-8")
        expected = updated_readme(current, collect())
    except (OSError, ValueError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"README stats verification failed: {error}", file=sys.stderr)
        return 1

    if args.write:
        README.write_text(expected, encoding="utf-8")
        print("README repository statistics updated")
        return 0
    if current != expected:
        diff = difflib.unified_diff(
            current.splitlines(),
            expected.splitlines(),
            fromfile="README.md",
            tofile="README.md (generated)",
            lineterm="",
        )
        print("\n".join(diff), file=sys.stderr)
        return 1
    print("README repository statistics verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
