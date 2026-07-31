import importlib.util
import os
import pathlib
import subprocess
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("run_instruction_prefix_experiment.py")
SPEC = importlib.util.spec_from_file_location("instruction_prefix_runner", MODULE_PATH)
RUNNER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(RUNNER)


class InstructionPrefixRunnerTests(unittest.TestCase):
    def test_summary_keeps_provider_metrics_explicitly_unknown(self):
        metadata = {
            "git": {"head": "abc", "snapshot_sha256": "dirty"},
        }
        records = []
        for mode in ("passthrough", "stable", "adaptive"):
            records.extend(
                {
                    "mode": mode,
                    "renderer_cache_eligible": mode == "stable",
                    "renderer_cache_hit": False,
                    "rendered_prefix_sha256": f"{mode}-{index}",
                }
                for index in range(6)
            )
        result = {
            "ok": True,
            "records": records,
            "assertions": [],
            "provider_metrics_by_mode": {
                "stable": {
                    "churn_rate": {
                        "state": "unknown",
                        "reason": "provider telemetry unavailable",
                    }
                }
            },
        }

        summary = RUNNER.render_summary(metadata, result)

        self.assertIn("made no provider request", summary)
        self.assertIn("provider telemetry unavailable", summary)
        self.assertNotIn("guaranteed", summary.lower())

    def test_atomic_write_replaces_complete_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "artifact.json"
            RUNNER.atomic_write(path, "first\n")
            RUNNER.atomic_write(path, "second\n")

            self.assertEqual(path.read_text(encoding="utf-8"), "second\n")
            self.assertEqual(list(path.parent.glob(".artifact.json.*")), [])

    @unittest.skipUnless(hasattr(os, "symlink"), "requires symbolic links")
    def test_source_snapshot_excludes_outputs_and_does_not_dereference_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "repo"
            root.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            source = root / "source.txt"
            output = root / "artifacts"
            output.mkdir()
            result = output / "result.json"
            source.write_text("source-v1\n", encoding="utf-8")
            result.write_text('{"result":1}\n', encoding="utf-8")
            subprocess.run(
                ["git", "add", "source.txt", "artifacts/result.json"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                [
                    "git",
                    "-c",
                    "user.name=Experiment test",
                    "-c",
                    "user.email=experiment@invalid.example",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    "fixture",
                ],
                cwd=root,
                check=True,
            )

            baseline = RUNNER.git_snapshot_sha256(root, output)
            result.write_text('{"result":2}\n', encoding="utf-8")
            self.assertEqual(RUNNER.git_snapshot_sha256(root, output), baseline)

            external = pathlib.Path(directory) / "external.txt"
            external.write_text("outside-v1\n", encoding="utf-8")
            os.symlink(external, root / "external-link")
            with_link = RUNNER.git_snapshot_sha256(root, output)
            external.write_text("outside-v2\n", encoding="utf-8")
            self.assertEqual(RUNNER.git_snapshot_sha256(root, output), with_link)

            source.write_text("source-v2\n", encoding="utf-8")
            self.assertNotEqual(RUNNER.git_snapshot_sha256(root, output), with_link)


if __name__ == "__main__":
    unittest.main()
