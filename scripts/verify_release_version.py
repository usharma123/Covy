#!/usr/bin/env python3
"""Reject release tags that disagree with Cargo, npm, or release notes."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path


TAG = re.compile(r"^v(?P<version>0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
PLATFORM_PACKAGES = (
    "@packet28/darwin-arm64",
    "@packet28/darwin-x64",
    "@packet28/linux-x64",
    "@packet28/linux-arm64",
)


def package_data(path: Path) -> dict[str, object]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError(f"{path} is not a JSON object")
    return data


def package_version(path: Path, data: dict[str, object]) -> str:
    version = data.get("version")
    if not isinstance(version, str) or not version:
        raise ValueError(f"{path} has no string version")
    return version


def validate(root: Path, tag: str) -> list[str]:
    errors: list[str] = []
    matched = TAG.fullmatch(tag)
    if matched is None:
        return [f"release tag must match vMAJOR.MINOR.PATCH exactly: {tag!r}"]
    tag_version = tag[1:]

    cargo_path = root / "Cargo.toml"
    cargo = tomllib.loads(cargo_path.read_text(encoding="utf-8"))
    cargo_version = cargo.get("workspace", {}).get("package", {}).get("version")
    root_package_path = root / "npm" / "packet28" / "package.json"
    platform_template_path = root / "npm" / "platform-template" / "package.json"
    root_package = package_data(root_package_path)
    platform_template = package_data(platform_template_path)
    versions = {
        "workspace Cargo version": cargo_version,
        "root npm package version": package_version(root_package_path, root_package),
        "platform npm template version": package_version(
            platform_template_path, platform_template
        ),
    }
    for label, version in versions.items():
        if version != tag_version:
            errors.append(f"{label} is {version!r}, expected {tag_version!r}")

    optional = root_package.get("optionalDependencies")
    if not isinstance(optional, dict):
        errors.append("root npm package has no optionalDependencies object")
    else:
        for package in PLATFORM_PACKAGES:
            dependency_version = optional.get(package)
            if dependency_version != tag_version:
                errors.append(
                    f"root npm dependency {package} is {dependency_version!r}, "
                    f"expected {tag_version!r}"
                )

    notes = root / "docs" / "releases" / f"{tag}.md"
    if not notes.is_file() or not notes.read_text(encoding="utf-8").strip():
        errors.append(f"release notes are missing or empty: {notes}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()

    try:
        errors = validate(args.root.resolve(), args.tag)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"release version preflight failed: {error}", file=sys.stderr)
        return 1

    if errors:
        for error in errors:
            print(f"release version preflight failed: {error}", file=sys.stderr)
        return 1
    print(f"release version preflight passed ({args.tag})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
