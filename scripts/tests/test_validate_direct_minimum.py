from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import validate_direct_minimum


ROOT_MANIFEST = """\
[workspace]
members = ["crates/example"]
resolver = "2"
"""

MEMBER_MANIFEST = """\
[package]
name = "example"
version = "0.1.0"
edition = "2021"
"""


def write_fixture(root: Path, manifest_sha256: str) -> None:
    (root / "crates" / "example").mkdir(parents=True)
    (root / "Cargo.toml").write_text(ROOT_MANIFEST, encoding="utf-8")
    (root / "crates" / "example" / "Cargo.toml").write_text(
        MEMBER_MANIFEST,
        encoding="utf-8",
    )
    (root / "Cargo.lock").write_text("canonical\n", encoding="utf-8")
    (root / validate_direct_minimum.CONFIG_NAME).write_text(
        "\n".join(
            [
                "format = 1",
                'toolchain = "nightly-test"',
                f'manifest-sha256 = "{manifest_sha256}"',
                "",
                "[[transitive-pins]]",
                'package = "derived"',
                'version = "1.2.3"',
                'reason = "runtime compatibility"',
                "",
            ]
        ),
        encoding="utf-8",
    )


class RepositoryDirectMinimumTests(unittest.TestCase):
    def test_repository_manifest_digest_matches_committed_graph(self) -> None:
        config = validate_direct_minimum.load_config(
            validate_direct_minimum.ROOT
        )

        self.assertEqual(
            config.manifest_sha256,
            validate_direct_minimum.manifest_digest(
                validate_direct_minimum.ROOT
            ),
        )
        self.assertTrue(
            (
                validate_direct_minimum.ROOT
                / validate_direct_minimum.LOCK_NAME
            ).is_file()
        )
        validate_direct_minimum.validate_transitive_pins(
            validate_direct_minimum.ROOT / validate_direct_minimum.LOCK_NAME,
            config,
        )

    def test_nightly_command_names_the_real_direct_minimum_resolver(self) -> None:
        self.assertEqual(
            validate_direct_minimum.nightly_cargo_command(
                "nightly-test",
                ["-Z", "direct-minimal-versions", "generate-lockfile"],
                offline=True,
            ),
            [
                "rustup",
                "run",
                "nightly-test",
                "cargo",
                "-Z",
                "direct-minimal-versions",
                "generate-lockfile",
                "--offline",
            ],
        )


class DirectMinimumFixtureTests(unittest.TestCase):
    def test_transitive_pin_must_match_the_committed_lock(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            write_fixture(root, "0" * 64)
            config = validate_direct_minimum.load_config(root)
            lock = root / validate_direct_minimum.LOCK_NAME
            lock.write_text(
                'version = 4\n\n[[package]]\nname = "derived"\n'
                'version = "1.2.4"\n',
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                validate_direct_minimum.DirectMinimumError,
                "must resolve only to 1.2.3",
            ):
                validate_direct_minimum.validate_transitive_pins(lock, config)

    def test_manifest_digest_covers_every_workspace_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            write_fixture(root, "0" * 64)
            before = validate_direct_minimum.manifest_digest(root)

            member = root / "crates" / "example" / "Cargo.toml"
            member.write_text(
                MEMBER_MANIFEST + "\n[dependencies]\nserde = \"1\"\n",
                encoding="utf-8",
            )

            self.assertNotEqual(
                before,
                validate_direct_minimum.manifest_digest(root),
            )

    def test_stale_manifest_fails_before_cargo_runs(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            write_fixture(root, "0" * 64)
            (root / validate_direct_minimum.LOCK_NAME).write_text(
                "minimum\n",
                encoding="utf-8",
            )
            config = validate_direct_minimum.load_config(root)

            with mock.patch.object(validate_direct_minimum, "run") as run:
                with self.assertRaisesRegex(
                    validate_direct_minimum.DirectMinimumError,
                    "manifests changed without refreshing",
                ):
                    validate_direct_minimum.validate_lock(
                        root,
                        config,
                        offline=True,
                    )
            run.assert_not_called()

    def test_refresh_uses_pinned_resolver_and_transitive_exception(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            write_fixture(root, "0" * 64)
            config = validate_direct_minimum.load_config(root)
            commands: list[list[str]] = []

            def fake_run(
                command: list[str],
                cwd: Path,
                _target_dir: Path,
            ) -> None:
                commands.append(command)
                if "generate-lockfile" in command:
                    (cwd / "Cargo.lock").write_text(
                        "direct minimum\n",
                        encoding="utf-8",
                    )

            with mock.patch.object(
                validate_direct_minimum,
                "run",
                side_effect=fake_run,
            ):
                validate_direct_minimum.refresh_lock(
                    root,
                    config,
                    offline=True,
                )

            self.assertIn(
                [
                    "rustup",
                    "run",
                    "nightly-test",
                    "cargo",
                    "-Z",
                    "direct-minimal-versions",
                    "generate-lockfile",
                    "--offline",
                ],
                commands,
            )
            self.assertIn(
                [
                    "rustup",
                    "run",
                    "nightly-test",
                    "cargo",
                    "update",
                    "-p",
                    "derived",
                    "--precise",
                    "1.2.3",
                    "--offline",
                ],
                commands,
            )
            self.assertEqual(
                (root / validate_direct_minimum.LOCK_NAME).read_text(
                    encoding="utf-8"
                ),
                "direct minimum\n",
            )
            refreshed = validate_direct_minimum.load_config(root)
            self.assertEqual(
                refreshed.manifest_sha256,
                validate_direct_minimum.manifest_digest(root),
            )


if __name__ == "__main__":
    unittest.main()
