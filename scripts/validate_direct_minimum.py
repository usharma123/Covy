#!/usr/bin/env python3
"""Build Packet28 against its committed direct-minimum Cargo graph."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Sequence


ROOT = Path(__file__).resolve().parents[1]
CONFIG_NAME = "direct-minimum.toml"
LOCK_NAME = "Cargo.direct-minimal.lock"
IGNORED_COPY_NAMES = frozenset(
    {".git", ".packet28", "__pycache__", "node_modules", "target"}
)
SHA256 = re.compile(r"^[0-9a-f]{64}$")


class DirectMinimumError(RuntimeError):
    """A direct-minimum graph invariant is not satisfied."""


@dataclass(frozen=True)
class TransitivePin:
    package: str
    version: str
    reason: str


@dataclass(frozen=True)
class DirectMinimumConfig:
    toolchain: str
    manifest_sha256: str
    transitive_pins: tuple[TransitivePin, ...]


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def require_string(table: dict[str, Any], key: str, source: Path) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise DirectMinimumError(f"{source}: {key!r} must be a non-empty string")
    return value


def load_config(root: Path) -> DirectMinimumConfig:
    path = root / CONFIG_NAME
    document = load_toml(path)
    if document.get("format") != 1:
        raise DirectMinimumError(f"{path}: format must be exactly 1")

    toolchain = require_string(document, "toolchain", path)
    manifest_sha256 = require_string(document, "manifest-sha256", path)
    if SHA256.fullmatch(manifest_sha256) is None:
        raise DirectMinimumError(
            f"{path}: manifest-sha256 must be 64 lowercase hexadecimal digits"
        )

    raw_pins = document.get("transitive-pins", [])
    if not isinstance(raw_pins, list):
        raise DirectMinimumError(f"{path}: transitive-pins must be an array")
    pins = []
    seen_packages: set[str] = set()
    for index, raw_pin in enumerate(raw_pins):
        if not isinstance(raw_pin, dict):
            raise DirectMinimumError(
                f"{path}: transitive-pins[{index}] must be a table"
            )
        package = require_string(raw_pin, "package", path)
        if package in seen_packages:
            raise DirectMinimumError(
                f"{path}: duplicate transitive pin for {package!r}"
            )
        seen_packages.add(package)
        pins.append(
            TransitivePin(
                package=package,
                version=require_string(raw_pin, "version", path),
                reason=require_string(raw_pin, "reason", path),
            )
        )

    return DirectMinimumConfig(
        toolchain=toolchain,
        manifest_sha256=manifest_sha256,
        transitive_pins=tuple(pins),
    )


def workspace_manifest_paths(root: Path) -> tuple[Path, ...]:
    root_manifest = root / "Cargo.toml"
    document = load_toml(root_manifest)
    workspace = document.get("workspace")
    if not isinstance(workspace, dict):
        raise DirectMinimumError(f"{root_manifest}: workspace table is missing")
    members = workspace.get("members")
    if not isinstance(members, list) or not all(
        isinstance(member, str) and member for member in members
    ):
        raise DirectMinimumError(
            f"{root_manifest}: workspace.members must be non-empty strings"
        )

    manifests = {root_manifest}
    for member in members:
        matches = sorted(root.glob(member))
        if not matches:
            raise DirectMinimumError(
                f"{root_manifest}: workspace member {member!r} does not exist"
            )
        for match in matches:
            manifest = match / "Cargo.toml"
            if not manifest.is_file():
                raise DirectMinimumError(
                    f"{root_manifest}: workspace member {member!r} lacks Cargo.toml"
                )
            manifests.add(manifest)
    return tuple(sorted(manifests))


def manifest_digest(root: Path) -> str:
    digest = hashlib.sha256()
    for manifest in workspace_manifest_paths(root):
        relative = manifest.relative_to(root).as_posix()
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(manifest.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def write_config(
    root: Path,
    config: DirectMinimumConfig,
    manifest_sha256: str,
) -> None:
    lines = [
        "format = 1",
        f"toolchain = {json.dumps(config.toolchain)}",
        f"manifest-sha256 = {json.dumps(manifest_sha256)}",
    ]
    for pin in config.transitive_pins:
        lines.extend(
            [
                "",
                "[[transitive-pins]]",
                f"package = {json.dumps(pin.package)}",
                f"version = {json.dumps(pin.version)}",
                f"reason = {json.dumps(pin.reason)}",
            ]
        )
    (root / CONFIG_NAME).write_text("\n".join(lines) + "\n", encoding="utf-8")


def ignored_copy_entries(_directory: str, names: list[str]) -> set[str]:
    return set(names).intersection(IGNORED_COPY_NAMES)


def copy_workspace(root: Path, destination: Path) -> None:
    shutil.copytree(
        root,
        destination,
        ignore=ignored_copy_entries,
        symlinks=True,
    )


def run(command: Sequence[str], cwd: Path, target_dir: Path) -> None:
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    subprocess.run(command, cwd=cwd, env=environment, check=True)


def cargo_command(
    arguments: Sequence[str],
    *,
    offline: bool,
) -> list[str]:
    command = ["cargo", *arguments]
    if offline:
        command.append("--offline")
    return command


def nightly_cargo_command(
    toolchain: str,
    arguments: Sequence[str],
    *,
    offline: bool,
) -> list[str]:
    return [
        "rustup",
        "run",
        toolchain,
        *cargo_command(arguments, offline=offline),
    ]


def refresh_lock(
    root: Path,
    config: DirectMinimumConfig,
    *,
    offline: bool,
) -> None:
    with tempfile.TemporaryDirectory(prefix="packet28-direct-minimum-") as raw:
        workspace = Path(raw) / "workspace"
        copy_workspace(root, workspace)
        generated_lock = workspace / "Cargo.lock"
        generated_lock.unlink(missing_ok=True)
        target_dir = root / "target" / "direct-minimum"

        run(
            nightly_cargo_command(
                config.toolchain,
                ["-Z", "direct-minimal-versions", "generate-lockfile"],
                offline=offline,
            ),
            workspace,
            target_dir,
        )
        for pin in config.transitive_pins:
            run(
                nightly_cargo_command(
                    config.toolchain,
                    [
                        "update",
                        "-p",
                        pin.package,
                        "--precise",
                        pin.version,
                    ],
                    offline=offline,
                ),
                workspace,
                target_dir,
            )
        shutil.copy2(generated_lock, root / LOCK_NAME)

    write_config(root, config, manifest_digest(root))


def validate_transitive_pins(
    source_lock: Path,
    config: DirectMinimumConfig,
) -> None:
    document = load_toml(source_lock)
    raw_packages = document.get("package")
    if not isinstance(raw_packages, list):
        raise DirectMinimumError(f"{source_lock}: package array is missing")

    versions_by_name: dict[str, set[str]] = {}
    for raw_package in raw_packages:
        if not isinstance(raw_package, dict):
            raise DirectMinimumError(
                f"{source_lock}: every package entry must be a table"
            )
        name = raw_package.get("name")
        version = raw_package.get("version")
        if isinstance(name, str) and isinstance(version, str):
            versions_by_name.setdefault(name, set()).add(version)

    for pin in config.transitive_pins:
        actual = versions_by_name.get(pin.package, set())
        if actual != {pin.version}:
            rendered = ", ".join(sorted(actual)) or "missing"
            raise DirectMinimumError(
                f"{source_lock}: transitive pin {pin.package!r} must resolve "
                f"only to {pin.version}, found {rendered}; refresh the graph"
            )


def validate_lock(
    root: Path,
    config: DirectMinimumConfig,
    *,
    offline: bool,
) -> None:
    actual_digest = manifest_digest(root)
    if actual_digest != config.manifest_sha256:
        raise DirectMinimumError(
            "workspace manifests changed without refreshing the direct-minimum "
            f"graph: expected {config.manifest_sha256}, found {actual_digest}; "
            "run scripts/validate_direct_minimum.py --refresh"
        )

    source_lock = root / LOCK_NAME
    if not source_lock.is_file():
        raise DirectMinimumError(f"{source_lock}: committed lock is missing")
    validate_transitive_pins(source_lock, config)

    with tempfile.TemporaryDirectory(prefix="packet28-direct-minimum-") as raw:
        workspace = Path(raw) / "workspace"
        copy_workspace(root, workspace)
        shutil.copy2(source_lock, workspace / "Cargo.lock")
        run(
            cargo_command(
                [
                    "check",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--locked",
                ],
                offline=offline,
            ),
            workspace,
            root / "target" / "direct-minimum",
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "compile the workspace against its committed direct-minimum "
            "dependency graph"
        )
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="workspace root (default: repository root)",
    )
    parser.add_argument(
        "--refresh",
        action="store_true",
        help="regenerate the committed graph with the pinned nightly resolver",
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="forbid registry and crate downloads",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    try:
        config = load_config(root)
        if args.refresh:
            refresh_lock(root, config, offline=args.offline)
            config = load_config(root)
        validate_lock(root, config, offline=args.offline)
    except (DirectMinimumError, OSError, subprocess.CalledProcessError) as error:
        print(f"direct-minimum dependency invariant failed: {error}", file=sys.stderr)
        return 1

    print(
        "direct-minimum dependency invariant passed "
        f"({config.toolchain}, {len(config.transitive_pins)} transitive pin)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
