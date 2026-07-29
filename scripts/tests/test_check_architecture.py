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
    "context-kernel-builtins",
    "context-kernel-core",
    "context-kernel-mechanism",
    "context-memory-core",
    "packet28-reducer-core",
    "packet28-search-core",
    "packet28-search-cli",
)
KERNEL_BUILTIN_TARGETS = (
    "agenty.state.snapshot",
    "agenty.state.write",
    "buildy.reduce",
    "contextq.assemble",
    "contextq.correlate",
    "contextq.manage",
    "diffy.analyze",
    "governed.assemble",
    "guardy.check",
    "mapy.query",
    "mapy.repo",
    "packet28.broker_memory.write",
    "packet28.instruction.summarize",
    "proxy.run",
    "stacky.slice",
    "testy.impact",
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
    include_builtin_mechanism_edge: bool = True,
) -> dict[str, object]:
    edges = {source: list(dependencies) for source, dependencies in edges.items()}
    edges.setdefault("context-kernel-core", []).append(
        ("context-kernel-builtins", None)
    )
    if include_builtin_mechanism_edge:
        edges.setdefault("context-kernel-builtins", []).append(
            ("context-kernel-mechanism", None)
        )
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


def write_kernel_sources(root: Path, mechanism_source: str) -> None:
    mechanism = root / "crates" / "context-kernel-mechanism" / "src"
    builtins = root / "crates" / "context-kernel-builtins" / "src"
    mechanism.mkdir(parents=True)
    builtins.mkdir(parents=True)
    (mechanism / "lib.rs").write_text(mechanism_source, encoding="utf-8")
    (builtins / "kernel_registry.rs").write_text(
        "\n".join(
            f'kernel.register_reducer("{target}");'
            for target in KERNEL_BUILTIN_TARGETS
        ),
        encoding="utf-8",
    )


class ArchitectureDependencyTests(unittest.TestCase):
    def run_checker(
        self, fixture: dict[str, object], *, source_root: Path | None = None
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "metadata.json"
            path.write_text(json.dumps(fixture), encoding="utf-8")
            command = [sys.executable, str(SCRIPT), "--metadata", str(path)]
            if source_root is not None:
                command.extend(["--source-root", str(source_root)])
            return subprocess.run(
                command,
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

    def test_kernel_mechanism_must_not_reach_concrete_builtins_transitively(
        self,
    ) -> None:
        fixture = metadata(
            {
                "context-kernel-mechanism": [("adapter-bridge", None)],
                "adapter-bridge": [("guardy-core", None)],
            },
            additional_packages=("adapter-bridge", "guardy-core"),
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-mechanism -> adapter-bridge -> guardy-core",
            result.stderr,
        )

    def test_kernel_mechanism_must_not_depend_on_concrete_builtins_directly(
        self,
    ) -> None:
        fixture = metadata(
            {"context-kernel-mechanism": [("guardy-core", None)]},
            additional_packages=("guardy-core",),
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-mechanism -> guardy-core",
            result.stderr,
        )

    def test_kernel_core_must_remain_a_single_edge_compatibility_facade(
        self,
    ) -> None:
        fixture = metadata(
            {
                "context-kernel-core": [
                    ("context-kernel-mechanism", None),
                ],
            }
        )

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-core compatibility facade must have exactly one "
            "normal dependency",
            result.stderr,
        )

    def test_kernel_builtins_must_depend_directly_on_mechanism(self) -> None:
        fixture = metadata({}, include_builtin_mechanism_edge=False)

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-builtins must depend directly on "
            "context-kernel-mechanism",
            result.stderr,
        )

    def test_kernel_mechanism_source_rejects_builtin_target_literals(
        self,
    ) -> None:
        fixture = metadata({})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_kernel_sources(
                root,
                'const TARGET: &str = "guardy.check";\n',
            )

            result = self.run_checker(fixture, source_root=root)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-mechanism source contains concrete built-in "
            "literal 'guardy.check'",
            result.stderr,
        )

    def test_kernel_mechanism_source_rejects_concrete_crate_identifiers(
        self,
    ) -> None:
        fixture = metadata({})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_kernel_sources(root, "use guardy_core::ContextConfig;\n")

            result = self.run_checker(fixture, source_root=root)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-mechanism source contains concrete built-in "
            "literal 'guardy_core'",
            result.stderr,
        )

    def test_kernel_registry_must_own_each_target_exactly_once(self) -> None:
        fixture = metadata({})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_kernel_sources(root, "")
            registry = (
                root
                / "crates"
                / "context-kernel-builtins"
                / "src"
                / "kernel_registry.rs"
            )
            registry.write_text(
                registry.read_text(encoding="utf-8")
                + '\nconst DUPLICATE: &str = "guardy.check";\n',
                encoding="utf-8",
            )

            result = self.run_checker(fixture, source_root=root)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-builtins registry must own exactly one "
            "registration literal for 'guardy.check'",
            result.stderr,
        )

    def test_tokio_is_restricted_to_process_orchestration_boundaries(self) -> None:
        fixture = metadata(
            {
                "context-kernel-core": [("tokio", None)],
                "packet28d": [("tokio", None)],
                "suite-cli": [("tokio", None)],
            },
            additional_packages=("packet28d", "suite-cli", "tokio"),
        )
        fixture["workspace_members"].remove("tokio")

        result = self.run_checker(fixture)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "context-kernel-core reaches async runtime outside an "
            "orchestration boundary: context-kernel-core -> tokio",
            result.stderr,
        )
        self.assertNotIn(
            "packet28d reaches async runtime outside", result.stderr
        )
        self.assertNotIn(
            "suite-cli reaches async runtime outside", result.stderr
        )


if __name__ == "__main__":
    unittest.main()
