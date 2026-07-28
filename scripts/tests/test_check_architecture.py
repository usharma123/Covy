from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_architecture.py"
REQUIRED_PACKAGES = (
    "packet28-daemon-protocol",
    "context-instruct-shim",
    "packet28-daemon-core",
    "suite-packet-core",
    "context-kernel-core",
    "context-memory-core",
    "packet28-reducer-core",
    "packet28-search-core",
    "packet28-search-cli",
)


def package(name: str, *, binary: bool = False) -> dict[str, object]:
    kinds = ["bin"] if binary else ["lib"]
    return {
        "id": name,
        "name": name,
        "targets": [{"name": name, "kind": kinds}],
    }


def dependency(name: str, kind: str | None = None) -> dict[str, object]:
    return {
        "name": name.replace("-", "_"),
        "pkg": name,
        "dep_kinds": [{"kind": kind, "target": None}],
    }


def metadata(
    edges: dict[str, Iterable[tuple[str, str | None]]],
    *,
    additional_packages: Iterable[str] = (),
) -> dict[str, object]:
    package_names = set(REQUIRED_PACKAGES)
    package_names.update(additional_packages)
    for source, dependencies in edges.items():
        package_names.add(source)
        package_names.update(target for target, _kind in dependencies)

    packages = [
        package(name, binary=name == "packet28-search-cli")
        for name in sorted(package_names)
    ]
    nodes = [
        {
            "id": name,
            "deps": [
                dependency(target, kind)
                for target, kind in edges.get(name, ())
            ],
        }
        for name in sorted(package_names)
    ]
    return {
        "packages": packages,
        "workspace_members": sorted(package_names),
        "resolve": {"nodes": nodes},
    }


class ArchitectureDependencyTests(unittest.TestCase):
    def run_checker(
        self, fixture: dict[str, object]
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "metadata.json"
            path.write_text(json.dumps(fixture), encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(SCRIPT), "--metadata", str(path)],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

    def test_good_graph_passes_and_ignores_non_normal_edges(self) -> None:
        fixture = metadata(
            {
                "packet28-daemon-protocol": [
                    ("suite-packet-core", None),
                    ("packet28-daemon-core", "dev"),
                    ("context-kernel-core", "build"),
                ],
                "context-instruct-shim": [
                    ("packet28-daemon-protocol", None),
                ],
                "packet28-daemon-core": [
                    ("packet28-daemon-protocol", None),
                ],
                "packet28-search-cli": [
                    ("packet28-daemon-protocol", None),
                ],
            }
        )

        result = self.run_checker(fixture)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("architecture dependency invariant passed", result.stdout)

    def test_forbidden_direct_dependency_fails(self) -> None:
        fixture = metadata(
            {
                "packet28-daemon-protocol": [
                    ("packet28-daemon-core", None),
                ],
            }
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "packet28-daemon-protocol reaches forbidden normal dependency "
            "packet28-daemon-core",
            result.stderr,
        )
        self.assertIn(
            "packet28-daemon-protocol -> packet28-daemon-core",
            result.stderr,
        )

    def test_forbidden_transitive_dependency_fails_with_full_path(self) -> None:
        fixture = metadata(
            {
                "context-instruct-shim": [("adapter-bridge", None)],
                "adapter-bridge": [("context-memory-core", None)],
            },
            additional_packages=("adapter-bridge",),
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-instruct-shim reaches forbidden normal dependency "
            "context-memory-core",
            result.stderr,
        )
        self.assertIn(
            "context-instruct-shim -> adapter-bridge -> context-memory-core",
            result.stderr,
        )

    def test_packet28_search_cli_may_depend_on_reducer_and_search(self) -> None:
        fixture = metadata(
            {
                "packet28-daemon-protocol": [
                    ("suite-packet-core", None),
                ],
                "packet28-search-cli": [
                    ("packet28-daemon-protocol", None),
                    ("packet28-reducer-core", None),
                    ("packet28-search-core", None),
                ],
                "packet28-reducer-core": [
                    ("suite-packet-core", None),
                ],
                "packet28-search-core": [
                    ("suite-packet-core", None),
                ],
            }
        )

        result = self.run_checker(fixture)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("architecture dependency invariant passed", result.stdout)

    def test_packet28_search_cli_must_not_depend_on_daemon_runtime(self) -> None:
        fixture = metadata(
            {
                "packet28-search-cli": [
                    ("packet28-daemon-protocol", None),
                    ("packet28-daemon-core", None),
                ],
                "packet28-daemon-core": [
                    ("packet28-daemon-protocol", None),
                ],
            }
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "packet28-search-cli reaches forbidden normal dependency "
            "packet28-daemon-core",
            result.stderr,
        )

    def test_daemon_core_must_not_reach_search_implementation(self) -> None:
        fixture = metadata(
            {
                "packet28-daemon-core": [
                    ("packet28-daemon-protocol", None),
                    ("adapter-bridge", None),
                ],
                "adapter-bridge": [("packet28-search-core", None)],
            },
            additional_packages=("adapter-bridge",),
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "packet28-daemon-core -> adapter-bridge -> packet28-search-core",
            result.stderr,
        )


if __name__ == "__main__":
    unittest.main()
