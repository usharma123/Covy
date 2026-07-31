from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "verify_release_version.py"
FIXTURE = ROOT / "scripts" / "fixtures" / "release-version" / "valid"


class ReleaseVersionPreflightTests(unittest.TestCase):
    def run_preflight(self, root: Path, tag: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root), "--tag", tag],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

    def copied_fixture(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        root = Path(temporary.name) / "fixture"
        shutil.copytree(FIXTURE, root)
        return temporary, root

    def test_matching_versions_and_notes_pass(self) -> None:
        result = self.run_preflight(FIXTURE, "v1.2.3")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_malformed_tag_fails(self) -> None:
        result = self.run_preflight(FIXTURE, "1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("vMAJOR.MINOR.PATCH", result.stderr)

    def test_tag_mismatch_fails(self) -> None:
        result = self.run_preflight(FIXTURE, "v1.2.4")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected '1.2.4'", result.stderr)

    def test_root_npm_version_mismatch_fails(self) -> None:
        temporary, root = self.copied_fixture()
        self.addCleanup(temporary.cleanup)
        package = root / "npm" / "packet28" / "package.json"
        data = json.loads(package.read_text(encoding="utf-8"))
        data["version"] = "1.2.4"
        package.write_text(json.dumps(data), encoding="utf-8")

        result = self.run_preflight(root, "v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("root npm package version", result.stderr)

    def test_platform_template_version_mismatch_fails(self) -> None:
        temporary, root = self.copied_fixture()
        self.addCleanup(temporary.cleanup)
        package = root / "npm" / "platform-template" / "package.json"
        data = json.loads(package.read_text(encoding="utf-8"))
        data["version"] = "1.2.4"
        package.write_text(json.dumps(data), encoding="utf-8")

        result = self.run_preflight(root, "v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("platform npm template version", result.stderr)

    def test_optional_platform_dependency_mismatch_fails(self) -> None:
        temporary, root = self.copied_fixture()
        self.addCleanup(temporary.cleanup)
        package = root / "npm" / "packet28" / "package.json"
        data = json.loads(package.read_text(encoding="utf-8"))
        data["optionalDependencies"]["@packet28/linux-arm64"] = "1.2.4"
        package.write_text(json.dumps(data), encoding="utf-8")

        result = self.run_preflight(root, "v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("root npm dependency @packet28/linux-arm64", result.stderr)

    def test_missing_platform_dependency_fails(self) -> None:
        temporary, root = self.copied_fixture()
        self.addCleanup(temporary.cleanup)
        package = root / "npm" / "packet28" / "package.json"
        data = json.loads(package.read_text(encoding="utf-8"))
        del data["optionalDependencies"]["@packet28/darwin-x64"]
        package.write_text(json.dumps(data), encoding="utf-8")

        result = self.run_preflight(root, "v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("root npm dependency @packet28/darwin-x64", result.stderr)

    def test_missing_release_notes_fail(self) -> None:
        temporary, root = self.copied_fixture()
        self.addCleanup(temporary.cleanup)
        (root / "docs" / "releases" / "v1.2.3.md").unlink()

        result = self.run_preflight(root, "v1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("release notes are missing or empty", result.stderr)


if __name__ == "__main__":
    unittest.main()
