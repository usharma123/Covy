#!/usr/bin/env python3
"""Reject direct Cargo dependencies that have no source-level use."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


DEPENDENCY_TABLES = (
    ("dependencies", "normal"),
    ("dev-dependencies", "dev"),
    ("build-dependencies", "build"),
)


@dataclass(frozen=True)
class AuditResult:
    errors: tuple[str, ...]
    package_count: int
    direct_count: int
    workspace_dependency_count: int


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def dependency_tables(
    manifest: dict[str, Any],
) -> Iterable[tuple[str, str, dict[str, Any]]]:
    for table_name, scope in DEPENDENCY_TABLES:
        table = manifest.get(table_name, {})
        if isinstance(table, dict):
            yield table_name, scope, table

    targets = manifest.get("target", {})
    if not isinstance(targets, dict):
        return
    for target_name, target in sorted(targets.items()):
        if not isinstance(target, dict):
            continue
        for table_name, scope in DEPENDENCY_TABLES:
            table = target.get(table_name, {})
            if isinstance(table, dict):
                yield f"target.{target_name}.{table_name}", scope, table


def rust_sources(crate_root: Path, scope: str) -> tuple[Path, ...]:
    if scope == "build":
        build_script = crate_root / "build.rs"
        return (build_script,) if build_script.is_file() else ()

    sources = []
    for path in crate_root.rglob("*.rs"):
        relative = path.relative_to(crate_root)
        if "target" in relative.parts or relative == Path("build.rs"):
            continue
        sources.append(path)
    return tuple(sorted(sources))


def rust_code_without_comments_and_literals(source: str) -> str:
    """Blank non-code Rust text while preserving token boundaries and lines."""
    code = list(source)
    length = len(source)
    index = 0

    def blank(start: int, end: int) -> None:
        for position in range(start, min(end, length)):
            if code[position] != "\n":
                code[position] = " "

    while index < length:
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            end = length if end == -1 else end
            blank(index, end)
            index = end
            continue

        if source.startswith("/*", index):
            start = index
            depth = 1
            index += 2
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            blank(start, index)
            continue

        raw_prefix_length = 0
        for prefix in ("br", "cr", "r"):
            if source.startswith(prefix, index):
                raw_prefix_length = len(prefix)
                break
        if raw_prefix_length:
            cursor = index + raw_prefix_length
            hash_count = 0
            while cursor < length and source[cursor] == "#":
                hash_count += 1
                cursor += 1
            if cursor < length and source[cursor] == '"':
                delimiter = '"' + ("#" * hash_count)
                end = source.find(delimiter, cursor + 1)
                end = length if end == -1 else end + len(delimiter)
                blank(index, end)
                index = end
                continue

        string_start = index
        if source[index] in {"b", "c"} and index + 1 < length:
            if source[index + 1] == '"':
                string_start = index + 1
        if source[string_start] == '"':
            cursor = string_start + 1
            escaped = False
            while cursor < length:
                character = source[cursor]
                cursor += 1
                if escaped:
                    escaped = False
                elif character == "\\":
                    escaped = True
                elif character == '"':
                    break
            blank(index, cursor)
            index = cursor
            continue

        if source[index] == "'" and index + 2 < length:
            char_end = None
            if source[index + 1] != "\\" and source[index + 2] == "'":
                char_end = index + 3
            elif source[index + 1] == "\\":
                cursor = index + 2
                while cursor < min(length, index + 14):
                    if source[cursor] == "'" and source[cursor - 1] != "\\":
                        char_end = cursor + 1
                        break
                    cursor += 1
            if char_end is not None:
                blank(index, char_end)
                index = char_end
                continue

        index += 1

    return "".join(code)


def source_uses_dependency(source_text: str, dependency_name: str) -> bool:
    crate_identifier = dependency_name.replace("-", "_")
    pattern = rf"(?<![A-Za-z0-9_]){re.escape(crate_identifier)}(?:\b|::)"
    return re.search(pattern, source_text) is not None


def cargo_manifests(root: Path) -> tuple[Path, ...]:
    manifests = []
    for path in root.rglob("Cargo.toml"):
        relative = path.relative_to(root)
        if (
            "target" in relative.parts
            or ".git" in relative.parts
            or relative.parts[:2] == ("scripts", "fixtures")
        ):
            continue
        manifests.append(path)
    return tuple(sorted(manifests))


def audit(root: Path) -> AuditResult:
    root = root.resolve()
    errors: list[str] = []
    inherited_workspace_dependencies: set[str] = set()
    workspace_dependencies: dict[Path, dict[str, Any]] = {}
    direct_count = 0
    package_count = 0

    for manifest_path in cargo_manifests(root):
        manifest = load_toml(manifest_path)
        workspace = manifest.get("workspace", {})
        if isinstance(workspace, dict):
            dependencies = workspace.get("dependencies", {})
            if not isinstance(dependencies, dict):
                relative_manifest = manifest_path.relative_to(root)
                errors.append(
                    f"{relative_manifest}: [workspace.dependencies] must be a table"
                )
                dependencies = {}
            workspace_dependencies[manifest_path] = dependencies

        if "package" not in manifest:
            continue

        package_count += 1
        crate_root = manifest_path.parent
        source_cache: dict[str, str] = {}

        for table_name, scope, dependencies in dependency_tables(manifest):
            if scope not in source_cache:
                source_cache[scope] = rust_code_without_comments_and_literals(
                    "\n".join(
                        path.read_text(encoding="utf-8", errors="replace")
                        for path in rust_sources(crate_root, scope)
                    )
                )

            for dependency_name, specification in sorted(dependencies.items()):
                direct_count += 1
                if (
                    isinstance(specification, dict)
                    and specification.get("workspace") is True
                ):
                    inherited_workspace_dependencies.add(dependency_name)

                if not source_uses_dependency(
                    source_cache[scope], dependency_name
                ):
                    relative_manifest = manifest_path.relative_to(root)
                    errors.append(
                        f"{relative_manifest}: [{table_name}] dependency "
                        f"`{dependency_name}` has no use in {scope} Rust targets"
                    )

    workspace_dependency_count = 0
    for manifest_path, dependencies in workspace_dependencies.items():
        workspace_dependency_count += len(dependencies)
        for dependency_name in sorted(
            set(dependencies) - inherited_workspace_dependencies
        ):
            relative_manifest = manifest_path.relative_to(root)
            errors.append(
                f"{relative_manifest}: [workspace.dependencies] dependency "
                f"`{dependency_name}` is not inherited by any workspace member"
            )

    return AuditResult(
        errors=tuple(errors),
        package_count=package_count,
        direct_count=direct_count,
        workspace_dependency_count=workspace_dependency_count,
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="validate Packet28's direct-minimum Cargo dependency graph"
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path.cwd(),
        help="workspace root (default: current directory)",
    )
    args = parser.parse_args()

    result = audit(args.root)
    if result.errors:
        for error in result.errors:
            print(f"direct dependency invariant failed: {error}", file=sys.stderr)
        return 1

    print(
        "direct dependency invariant passed "
        f"({result.package_count} packages, {result.direct_count} direct "
        f"declarations, {result.workspace_dependency_count} workspace entries)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
