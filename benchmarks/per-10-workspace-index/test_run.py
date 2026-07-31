from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).with_name("run.py")
SPEC = importlib.util.spec_from_file_location("per10_run", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
PER10 = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PER10
SPEC.loader.exec_module(PER10)


class Per10HarnessTests(unittest.TestCase):
    def test_parse_build_output_preserves_machine_fields(self) -> None:
        parsed = PER10.parse_build_output(
            "build_ms=123.456 generation=7 files=902\n"
        )
        self.assertEqual(
            parsed,
            {"build_ms": 123.456, "generation": 7, "files": 902},
        )

    def test_parse_build_output_rejects_extra_or_malformed_output(self) -> None:
        with self.assertRaises(ValueError):
            PER10.parse_build_output("warning\nbuild_ms=1.0 generation=1 files=2\n")
        with self.assertRaises(ValueError):
            PER10.parse_build_output("build_ms=fast generation=1 files=2\n")

    def test_summary_uses_raw_runs_and_checks_shape_consistency(self) -> None:
        summary = PER10.summarize_runs(
            [
                {
                    "build_ms": 20.0,
                    "wall_ms": 22.0,
                    "generation": 1,
                    "files": 10,
                },
                {
                    "build_ms": 10.0,
                    "wall_ms": 12.0,
                    "generation": 1,
                    "files": 10,
                },
                {
                    "build_ms": 30.0,
                    "wall_ms": 32.0,
                    "generation": 1,
                    "files": 10,
                },
            ]
        )
        self.assertEqual(summary["build_ms"]["median"], 20.0)
        self.assertEqual(summary["wall_ms"]["median"], 22.0)
        self.assertTrue(summary["indexed_files_consistent"])
        self.assertTrue(summary["generations_consistent"])
        self.assertIsNone(PER10.summarize_runs([]))
        self.assertIsNone(
            PER10.summarize_runs(
                [{"wall_ms": 1.0, "exit_code": 2, "stdout": "", "stderr": "failed"}]
            )
        )

    def test_snapshot_includes_tracked_and_untracked_but_not_ignored_state(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            root.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            (root / ".gitignore").write_text(
                ".packet28/\nignored.txt\n", encoding="utf-8"
            )
            (root / "tracked.txt").write_text("tracked\n", encoding="utf-8")
            subprocess.run(
                ["git", "add", ".gitignore", "tracked.txt"], cwd=root, check=True
            )
            (root / "untracked.txt").write_text("untracked\n", encoding="utf-8")
            (root / "ignored.txt").write_text("ignored\n", encoding="utf-8")
            runtime_cache = root / "scripts" / "__pycache__"
            runtime_cache.mkdir(parents=True)
            (runtime_cache / "generated.pyc").write_bytes(b"interpreter-state")
            state = root / ".packet28"
            state.mkdir()
            (state / "index.bin").write_bytes(b"user-state")
            nested_state = root / "nested" / ".packet28"
            nested_state.mkdir(parents=True)
            (nested_state / "tracked.bin").write_bytes(b"tracked-cache-state")
            subprocess.run(
                ["git", "add", "-f", "nested/.packet28/tracked.bin"],
                cwd=root,
                check=True,
            )

            paths = PER10.version_control_visible_paths(root)
            destination = Path(directory) / "snapshot"
            snapshot = PER10.copy_worktree_snapshot(root, destination, paths)

            self.assertTrue((destination / "tracked.txt").is_file())
            self.assertTrue((destination / "untracked.txt").is_file())
            self.assertFalse((destination / "ignored.txt").exists())
            self.assertFalse((destination / ".packet28").exists())
            self.assertFalse((destination / "nested" / ".packet28").exists())
            self.assertFalse((destination / "scripts" / "__pycache__").exists())
            self.assertEqual(snapshot.file_count, 3)
            self.assertEqual((state / "index.bin").read_bytes(), b"user-state")
            matched, _reason = PER10.snapshot_matches_live_worktree(
                root, snapshot, PER10.version_control_visible_paths(root)
            )
            self.assertTrue(matched)
            (root / "tracked.txt").write_text("changed\n", encoding="utf-8")
            matched, reason = PER10.snapshot_matches_live_worktree(
                root, snapshot, PER10.version_control_visible_paths(root)
            )
            self.assertFalse(matched)
            self.assertIn("content or mode changed", reason)

    def test_snapshot_excludes_generated_outputs_from_its_own_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            root.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            (root / "source.rs").write_text("fn main() {}\n", encoding="utf-8")
            (root / "current.json").write_text("{\"old\":true}\n", encoding="utf-8")
            subprocess.run(
                ["git", "add", "source.rs", "current.json"],
                cwd=root,
                check=True,
            )

            excluded = PER10.repository_relative_outputs(
                root,
                (root / "current.json", Path(directory) / "external.json"),
            )
            paths = PER10.version_control_visible_paths(
                root,
                excluded_paths=excluded,
            )
            snapshot = PER10.copy_worktree_snapshot(
                root,
                Path(directory) / "snapshot",
                paths,
            )

            self.assertEqual(excluded, (Path("current.json"),))
            self.assertEqual(snapshot.relative_paths, (Path("source.rs"),))
            self.assertFalse(
                (Path(directory) / "snapshot" / "current.json").exists()
            )

    def test_snapshot_preserves_only_internal_relative_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            root.mkdir()
            (root / "target.txt").write_text("target\n", encoding="utf-8")
            (root / "sub").mkdir()
            (root / "sub" / "link").symlink_to("../target.txt")

            destination = Path(directory) / "snapshot"
            PER10.copy_worktree_snapshot(
                root,
                destination,
                (Path("target.txt"), Path("sub/link")),
            )

            copied_link = destination / "sub" / "link"
            self.assertEqual(os.readlink(copied_link), "../target.txt")
            self.assertEqual(copied_link.resolve(), (destination / "target.txt").resolve())

    def test_snapshot_rejects_symlink_that_escapes_source_or_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "repo"
            root.mkdir()
            (base / "outside.txt").write_text("outside\n", encoding="utf-8")
            (root / "escape").symlink_to("../outside.txt")

            with self.assertRaisesRegex(ValueError, "escaped its snapshot root"):
                PER10.copy_worktree_snapshot(
                    root,
                    base / "snapshot",
                    (Path("escape"),),
                )

    def test_snapshot_rejects_absolute_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "repo"
            root.mkdir()
            target = base / "target.txt"
            target.write_text("target\n", encoding="utf-8")
            (root / "absolute").symlink_to(target)

            with self.assertRaisesRegex(ValueError, "absolute symlink"):
                PER10.copy_worktree_snapshot(
                    root,
                    base / "snapshot",
                    (Path("absolute"),),
                )

    def test_git_selection_ignores_inherited_repository_context(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            selected = base / "selected"
            selected.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=selected, check=True)
            (selected / "selected.rs").write_text("fn selected() {}\n", encoding="utf-8")
            subprocess.run(["git", "add", "selected.rs"], cwd=selected, check=True)

            unrelated = base / "unrelated"
            unrelated.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=unrelated, check=True)
            (unrelated / "unrelated.rs").write_text(
                "fn unrelated() {}\n", encoding="utf-8"
            )
            subprocess.run(["git", "add", "unrelated.rs"], cwd=unrelated, check=True)

            with mock.patch.dict(
                os.environ,
                {
                    "GIT_DIR": str(unrelated / ".git"),
                    "GIT_WORK_TREE": str(unrelated),
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": "core.excludesFile",
                    "GIT_CONFIG_VALUE_0": str(base / "missing-excludes"),
                },
            ):
                paths = PER10.version_control_visible_paths(selected)

            self.assertEqual(paths, (Path("selected.rs"),))

    @unittest.skipUnless(os.name == "posix", "requires byte-preserving POSIX paths")
    def test_snapshot_identity_preserves_non_utf8_git_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            root.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            raw_root = os.fsencode(root)
            raw_name = b"non-utf8-\xff.rs"
            try:
                descriptor = os.open(
                    raw_root + b"/" + raw_name,
                    os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                    0o644,
                )
            except OSError as error:
                self.skipTest(f"filesystem rejected non-UTF-8 fixture: {error}")
            try:
                os.write(descriptor, b"fn byte_path() {}\n")
            finally:
                os.close(descriptor)
            subprocess.run(
                [b"git", b"add", raw_name],
                cwd=raw_root,
                check=True,
            )

            paths = PER10.version_control_visible_paths(root)
            snapshot_root = Path(directory) / "snapshot"
            snapshot = PER10.copy_worktree_snapshot(root, snapshot_root, paths)
            metadata = PER10.git_metadata(root)

            self.assertEqual(os.fsencode(paths[0].as_posix()), raw_name)
            self.assertEqual(snapshot.file_count, 1)
            self.assertTrue(os.path.isfile(os.fsencode(snapshot_root) + b"/" + raw_name))
            json.dumps(metadata, ensure_ascii=True)

    def test_ephemeral_git_identity_is_deterministic_and_hook_free(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            roots = (base / "first", base / "second")
            for root in roots:
                root.mkdir()

            with mock.patch.dict(
                os.environ,
                {
                    "GIT_DIR": str(base / "unrelated.git"),
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": "core.hooksPath",
                    "GIT_CONFIG_VALUE_0": str(base / "host-hooks"),
                },
            ):
                first = PER10.initialize_ephemeral_git_repository(roots[0])
                second = PER10.initialize_ephemeral_git_repository(roots[1])

            self.assertEqual(first["commit"], second["commit"])
            self.assertFalse((roots[0] / ".git" / "hooks").exists())
            self.assertIn(
                "GIT_DIR",
                first["commands"][0]["removed_environment"],
            )

    def test_isolation_rejects_live_or_escaped_fixture_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            workspace = base / "workspace"
            temporary = base / "temporary"
            workspace.mkdir()
            temporary.mkdir()
            isolated = temporary / "run-01"
            PER10.assert_isolated_path(
                isolated, temporary_root=temporary, workspace=workspace
            )
            with self.assertRaises(ValueError):
                PER10.assert_isolated_path(
                    workspace, temporary_root=temporary, workspace=workspace
                )
            with self.assertRaises(ValueError):
                PER10.assert_isolated_path(
                    base / "escaped",
                    temporary_root=temporary,
                    workspace=workspace,
                )

    def test_report_keeps_historical_result_non_comparable(self) -> None:
        document = {
            "status": "complete",
            "historical": {
                "p28_git": "59e54fb",
                "packet28d_version": "0.2.39",
                "workspace_index_build_ms": 10375.754,
            },
            "source": {
                "git": {
                    "head_commit": "a" * 40,
                    "dirty": True,
                },
                "snapshot_sha256": "b" * 64,
            },
            "summary": {
                "build_ms": {"min": 10.0, "median": 11.0, "max": 12.0},
                "wall_ms": {"min": 13.0, "median": 14.0, "max": 15.0},
            },
            "runs": [
                {
                    "iteration": 1,
                    "build_ms": 11.0,
                    "wall_ms": 14.0,
                    "generation": 1,
                    "files": 20,
                }
            ],
            "blocker": None,
            "build": {"stderr": ""},
        }
        report = PER10.render_readme(document)
        self.assertIn("10,375.754 ms", report)
        self.assertIn("no cross-environment speedup ratio is claimed", report)
        self.assertIn("final-tree evidence only when its recorded HEAD", report)
        self.assertIn("preliminary result predates", report)
        self.assertNotIn("speedup:", report.lower())

        document["source"]["input_sha256"] = {"Cargo.lock": "c" * 64}
        hardened_report = PER10.render_readme(document)
        self.assertIn("every `.packet28` subtree", hardened_report)
        self.assertIn("resolved executable, effective command environment", hardened_report)
        self.assertNotIn("preliminary result predates", hardened_report)

    def test_evidence_requires_three_cold_iterations(self) -> None:
        with self.assertRaisesRegex(ValueError, "at least 3"):
            PER10.validate_iterations(2)
        PER10.validate_iterations(3)

    def test_source_inputs_require_regular_locked_workspace_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            harness = root / "benchmarks" / "per-10-workspace-index"
            harness.mkdir(parents=True)
            (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
            (root / "Cargo.lock").write_text("version = 3\n", encoding="utf-8")
            (harness / "run.py").write_text("SCHEMA = 1\n", encoding="utf-8")

            identities = PER10.source_input_sha256(root)

            self.assertEqual(
                set(identities),
                {
                    "Cargo.toml",
                    "Cargo.lock",
                    "benchmarks/per-10-workspace-index/run.py",
                },
            )

    def test_historical_report_matches_pinned_identity(self) -> None:
        self.assertEqual(
            PER10.validate_historical_report(PER10.HISTORICAL_REPORT),
            PER10.EXPECTED_HISTORICAL_SHA256,
        )
        with tempfile.TemporaryDirectory() as directory:
            changed = Path(directory) / "historical.md"
            changed.write_text("changed evidence\n", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "digest changed"):
                PER10.validate_historical_report(changed)

    def test_command_record_includes_executable_and_effective_environment(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.dict(
                os.environ,
                {"RUSTFLAGS": "-Ctarget-cpu=baseline", "GIT_DIR": "/unsafe"},
            ):
                record = PER10.run_command(
                    [sys.executable, "-c", "print('ok')"],
                    cwd=Path(directory),
                    environment={"CARGO_NET_OFFLINE": "true"},
                    remove_environment=("GIT_DIR",),
                )

        serialized = record.to_json()
        self.assertEqual(record.exit_code, 0)
        self.assertTrue(Path(serialized["resolved_executable"]).is_file())
        self.assertEqual(
            serialized["environment"]["RUSTFLAGS"], "-Ctarget-cpu=baseline"
        )
        self.assertEqual(serialized["environment"]["CARGO_NET_OFFLINE"], "true")
        self.assertEqual(serialized["removed_environment"], ["GIT_DIR"])

    def test_machine_metadata_has_required_reproduction_fields(self) -> None:
        metadata = PER10.machine_metadata()
        self.assertTrue(
            {
                "operating_system",
                "os_release",
                "architecture",
                "cpu_model",
                "logical_cpu_count",
                "total_memory_bytes",
                "python",
                "uname",
            }.issubset(metadata)
        )
        self.assertGreater(metadata["logical_cpu_count"], 0)

    def test_raw_report_is_valid_json_and_does_not_touch_historical_file(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            historical = root / "historical.md"
            historical.write_text("historical evidence\n", encoding="utf-8")
            before = PER10.sha256_file(historical)
            raw = root / "raw.json"
            readme = root / "README.md"
            document = {
                "schema": PER10.SCHEMA,
                "status": "blocked",
                "historical": {
                    "p28_git": "59e54fb",
                    "packet28d_version": "0.2.39",
                    "workspace_index_build_ms": 10375.754,
                },
                "source": {
                    "git": {"head_commit": "a" * 40, "dirty": True},
                    "snapshot_sha256": "b" * 64,
                },
                "summary": None,
                "runs": [],
                "blocker": {"kind": "locked_release_build_failed", "message": "x"},
                "build": {"stderr": "lock is stale"},
            }

            PER10.write_reports(document, raw_path=raw, readme_path=readme)

            self.assertEqual(json.loads(raw.read_text())["schema"], PER10.SCHEMA)
            self.assertEqual(PER10.sha256_file(historical), before)
            self.assertIn("locked_release_build_failed", readme.read_text())


if __name__ == "__main__":
    unittest.main()
