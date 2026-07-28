#!/usr/bin/env python3
"""Reject forbidden normal-dependency paths in the Cargo workspace graph."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent

# Keep the explicit list as a backstop for metadata fixtures and packages whose
# binary target is temporarily absent during a refactor. Binary workspace
# packages discovered from Cargo metadata are forbidden as well.
CLI_PACKAGES = frozenset(
    {
        "covy-cli",
        "diffy-cli",
        "packet28-search-cli",
        "suite-cli",
        "testy-cli",
    }
)

# Async orchestration belongs at process boundaries. Keeping Tokio out of the
# reusable core graph prevents synchronous repository scans, SQLite, and
# serialization from being mislabeled as non-blocking merely because a runtime
# happens to be available.
TOKIO_RUNTIME_OWNERS = frozenset({"packet28d", "suite-cli"})


@dataclass(frozen=True)
class ArchitectureRule:
    source: str
    forbidden: frozenset[str]


BASE_RULES = (
    ArchitectureRule(
        source="context-instruct-shim",
        forbidden=frozenset(
            {
                "packet28-daemon-core",
                "context-kernel-core",
                "context-memory-core",
                "packet28-reducer-core",
                "packet28-search-core",
            }
        ),
    ),
    ArchitectureRule(
        source="packet28-daemon-core",
        forbidden=frozenset(
            {
                "context-kernel-core",
                "context-memory-core",
                "packet28-reducer-core",
                "packet28-search-core",
            }
        ),
    ),
    ArchitectureRule(
        source="packet28-search-cli",
        forbidden=frozenset(
            {
                "packet28-daemon-core",
                "context-kernel-core",
                "context-memory-core",
            }
        ),
    ),
    ArchitectureRule(
        source="suite-packet-core",
        forbidden=frozenset({"packet28-daemon-protocol"}),
    ),
)


class MetadataError(ValueError):
    """Cargo metadata did not contain the graph required by this check."""


class CargoMetadataGraph:
    """The resolved normal-dependency graph from Cargo metadata."""

    def __init__(self, metadata: dict[str, Any]) -> None:
        packages = metadata.get("packages")
        resolve = metadata.get("resolve")
        workspace_members = metadata.get("workspace_members")
        if not isinstance(packages, list):
            raise MetadataError("metadata is missing the packages array")
        if not isinstance(resolve, dict) or not isinstance(
            resolve.get("nodes"), list
        ):
            raise MetadataError("metadata is missing the resolved dependency graph")
        if not isinstance(workspace_members, list):
            raise MetadataError("metadata is missing the workspace_members array")

        self.names: dict[str, str] = {}
        self.packages: dict[str, dict[str, Any]] = {}
        for package in packages:
            if not isinstance(package, dict):
                raise MetadataError("metadata contains a non-object package")
            package_id = package.get("id")
            name = package.get("name")
            if not isinstance(package_id, str) or not isinstance(name, str):
                raise MetadataError("metadata contains a package without an id or name")
            if package_id in self.packages:
                raise MetadataError(
                    f"metadata contains duplicate package id {package_id}"
                )
            self.packages[package_id] = package
            self.names[package_id] = name

        self.workspace_members = set(workspace_members)
        unknown_members = self.workspace_members.difference(self.packages)
        if unknown_members:
            unknown = ", ".join(sorted(unknown_members))
            raise MetadataError(
                f"workspace members are absent from packages: {unknown}"
            )

        self.edges: dict[str, set[str]] = {
            package_id: set() for package_id in self.packages
        }
        for node in resolve["nodes"]:
            if not isinstance(node, dict):
                raise MetadataError("metadata contains a non-object resolve node")
            package_id = node.get("id")
            deps = node.get("deps")
            if not isinstance(package_id, str) or package_id not in self.packages:
                raise MetadataError("metadata contains an unknown resolve node")
            if not isinstance(deps, list):
                raise MetadataError(
                    f"resolve node {package_id} is missing dependency kinds"
                )
            for dependency in deps:
                if not isinstance(dependency, dict):
                    raise MetadataError(
                        f"resolve node {package_id} contains a malformed dependency"
                    )
                dependency_id = dependency.get("pkg")
                dependency_kinds = dependency.get("dep_kinds")
                if not isinstance(dependency_id, str):
                    raise MetadataError(
                        f"resolve node {package_id} contains a dependency without an id"
                    )
                if dependency_id not in self.packages:
                    raise MetadataError(
                        f"resolve node {package_id} references unknown {dependency_id}"
                    )
                if not isinstance(dependency_kinds, list):
                    raise MetadataError(
                        f"dependency {package_id} -> {dependency_id} lacks dep_kinds"
                    )
                if self._has_normal_kind(dependency_kinds):
                    self.edges[package_id].add(dependency_id)

        self.workspace_package_ids_by_name: dict[str, list[str]] = {}
        for package_id in self.workspace_members:
            self.workspace_package_ids_by_name.setdefault(
                self.names[package_id], []
            ).append(package_id)

    @staticmethod
    def _has_normal_kind(dependency_kinds: list[Any]) -> bool:
        # Cargo represents a normal dependency as kind=null. Accept "normal"
        # too so deterministic fixtures remain readable and future-compatible.
        return any(
            isinstance(kind, dict) and kind.get("kind") in (None, "normal")
            for kind in dependency_kinds
        )

    def workspace_package_id(self, name: str) -> str:
        matches = self.workspace_package_ids_by_name.get(name, [])
        if not matches:
            raise MetadataError(f"required workspace package is missing: {name}")
        if len(matches) > 1:
            raise MetadataError(f"workspace package name is ambiguous: {name}")
        return matches[0]

    def workspace_cli_packages(self) -> set[str]:
        result = set(CLI_PACKAGES)
        for package_id in self.workspace_members:
            package = self.packages[package_id]
            targets = package.get("targets", [])
            if not isinstance(targets, list):
                raise MetadataError(
                    f"package {self.names[package_id]} has malformed targets"
                )
            if any(
                isinstance(target, dict)
                and isinstance(target.get("kind"), list)
                and "bin" in target["kind"]
                for target in targets
            ):
                result.add(self.names[package_id])
        return result

    def shortest_forbidden_paths(
        self, source_name: str, forbidden_names: set[str] | frozenset[str]
    ) -> dict[str, list[str]]:
        source = self.workspace_package_id(source_name)
        predecessors: dict[str, str | None] = {source: None}
        queue = deque([source])

        while queue:
            current = queue.popleft()
            dependencies = sorted(
                self.edges.get(current, ()),
                key=lambda package_id: (self.names[package_id], package_id),
            )
            for dependency in dependencies:
                if dependency in predecessors:
                    continue
                predecessors[dependency] = current
                queue.append(dependency)

        found: dict[str, list[str]] = {}
        for package_id in predecessors:
            name = self.names[package_id]
            if package_id == source or name not in forbidden_names:
                continue
            path_ids: list[str] = []
            cursor: str | None = package_id
            while cursor is not None:
                path_ids.append(cursor)
                cursor = predecessors[cursor]
            path = [self.names[item] for item in reversed(path_ids)]
            previous = found.get(name)
            if previous is None or len(path) < len(previous):
                found[name] = path
        return found


def architecture_rules(graph: CargoMetadataGraph) -> tuple[ArchitectureRule, ...]:
    protocol_forbidden = {
        "packet28-daemon-core",
        "packet28d",
        "context-kernel-core",
        "context-memory-core",
        "packet28-reducer-core",
        "packet28-search-core",
        *graph.workspace_cli_packages(),
    }
    return (
        ArchitectureRule(
            source="packet28-daemon-protocol",
            forbidden=frozenset(protocol_forbidden),
        ),
        *BASE_RULES,
    )


def check_architecture(metadata: dict[str, Any]) -> list[str]:
    graph = CargoMetadataGraph(metadata)
    errors: list[str] = []
    for rule in architecture_rules(graph):
        paths = graph.shortest_forbidden_paths(rule.source, rule.forbidden)
        for forbidden, path in sorted(paths.items()):
            errors.append(
                f"{rule.source} reaches forbidden normal dependency {forbidden}: "
                f"{' -> '.join(path)}"
            )
    if "tokio" in graph.names.values():
        for package_id in sorted(
            graph.workspace_members,
            key=lambda candidate: graph.names[candidate],
        ):
            source = graph.names[package_id]
            if source in TOKIO_RUNTIME_OWNERS:
                continue
            paths = graph.shortest_forbidden_paths(source, {"tokio"})
            if path := paths.get("tokio"):
                errors.append(
                    f"{source} reaches async runtime outside an orchestration "
                    f"boundary: {' -> '.join(path)}"
                )
    return errors


def read_metadata(path: Path | None) -> dict[str, Any]:
    if path is not None:
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except OSError as error:
            raise MetadataError(
                f"cannot read metadata fixture {path}: {error}"
            ) from error
        except json.JSONDecodeError as error:
            raise MetadataError(
                f"metadata fixture {path} is not valid JSON: {error}"
            ) from error
    else:
        command = ["cargo", "metadata", "--locked", "--format-version", "1"]
        result = subprocess.run(
            command,
            cwd=ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or "cargo metadata failed without output"
            raise MetadataError(detail)
        try:
            value = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise MetadataError(
                f"cargo metadata emitted invalid JSON: {error}"
            ) from error

    if not isinstance(value, dict):
        raise MetadataError("metadata root must be a JSON object")
    return value


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="check Cargo normal-dependency architecture boundaries"
    )
    parser.add_argument(
        "--metadata",
        type=Path,
        metavar="PATH",
        help=(
            "read Cargo metadata JSON from PATH instead of invoking "
            "'cargo metadata --locked --format-version 1'"
        ),
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        errors = check_architecture(read_metadata(args.metadata))
    except MetadataError as error:
        print(f"architecture dependency invariant failed: {error}", file=sys.stderr)
        return 1

    if errors:
        for error in errors:
            print(f"architecture dependency invariant failed: {error}", file=sys.stderr)
        return 1

    print("architecture dependency invariant passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
