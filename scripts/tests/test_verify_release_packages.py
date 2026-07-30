from __future__ import annotations

import json
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import verify_release_packages


VERSION = "1.2.3"


def elf_header(machine: int) -> bytes:
    header = bytearray(20)
    header[:4] = b"\x7fELF"
    header[4] = 2
    header[5] = 1
    header[16:18] = (3).to_bytes(2, "little")
    header[18:20] = machine.to_bytes(2, "little")
    return bytes(header)


def macho_header(cpu_type: int) -> bytes:
    header = bytearray(20)
    header[:4] = b"\xcf\xfa\xed\xfe"
    header[4:8] = cpu_type.to_bytes(4, "little")
    header[12:16] = (2).to_bytes(4, "little")
    return bytes(header)


def write_platform_package(
    root: Path, platform_key: str, binary_header: bytes
) -> Path:
    spec = verify_release_packages.PLATFORMS[platform_key]
    package_dir = root / platform_key
    bin_dir = package_dir / "bin"
    bin_dir.mkdir(parents=True)
    manifest = {
        "name": f"@packet28/{platform_key}",
        "version": VERSION,
        "license": "MIT",
        "os": [spec.npm_os],
        "cpu": [spec.npm_cpu],
        "files": ["bin"],
        "preferUnplugged": True,
    }
    (package_dir / "package.json").write_text(
        json.dumps(manifest), encoding="utf-8"
    )
    for name in verify_release_packages.platform_artifacts(platform_key):
        binary = bin_dir / name
        binary.write_bytes(binary_header)
        binary.chmod(0o755)
    return package_dir


class BinaryHeaderTests(unittest.TestCase):
    def test_binary_identity_recognizes_every_release_architecture(self) -> None:
        cases = (
            (elf_header(62), ("elf", "x86_64")),
            (elf_header(183), ("elf", "arm64")),
            (macho_header(0x01000007), ("macho", "x86_64")),
            (macho_header(0x0100000C), ("macho", "arm64")),
        )
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "binary"
            for header, expected in cases:
                with self.subTest(expected=expected):
                    path.write_bytes(header)
                    self.assertEqual(
                        verify_release_packages.binary_identity(path),
                        expected,
                    )

    def test_binary_identity_rejects_universal_macho(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "binary"
            path.write_bytes(b"\xca\xfe\xba\xbe" + bytes(16))

            with self.assertRaisesRegex(
                verify_release_packages.VerificationError,
                "universal Mach-O",
            ):
                verify_release_packages.binary_identity(path)

    def test_platform_validation_rejects_wrong_architecture(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            package_dir = write_platform_package(
                Path(temp), "linux-arm64", elf_header(62)
            )

            with self.assertRaisesRegex(
                verify_release_packages.VerificationError,
                "expected elf arm64, got elf x86_64",
            ):
                verify_release_packages.validate_platform_binaries(
                    package_dir, "linux-arm64"
                )


class ReleaseArtifactContractTests(unittest.TestCase):
    def test_public_executables_match_workspace_binary_targets(self) -> None:
        completed = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--no-deps",
                "--format-version",
                "1",
            ],
            cwd=verify_release_packages.ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        metadata = json.loads(completed.stdout)
        release_packages = {"suite-cli", "packet28d", "packet28-search-cli"}
        binary_targets = {
            target["name"]
            for package in metadata["packages"]
            if package["name"] in release_packages
            for target in package["targets"]
            if "bin" in target["kind"]
        }

        self.assertEqual(
            set(verify_release_packages.EXECUTABLES),
            binary_targets,
        )

    def test_linux_contract_adds_only_the_instruction_shim(self) -> None:
        executables = verify_release_packages.EXECUTABLES
        self.assertEqual(
            verify_release_packages.platform_artifacts("darwin-arm64"),
            executables,
        )
        self.assertEqual(
            verify_release_packages.platform_artifacts("linux-x64"),
            (*executables, "libcontext_instruct_shim.so"),
        )


class BinaryExecutionTests(unittest.TestCase):
    @mock.patch.object(verify_release_packages, "run_checked_binary")
    @mock.patch.object(
        verify_release_packages, "host_platform_key", return_value="darwin-arm64"
    )
    def test_cross_macos_fallback_is_explicit_and_does_not_execute(
        self,
        _host_platform_key: mock.Mock,
        run_checked_binary: mock.Mock,
    ) -> None:
        executed = verify_release_packages.smoke_platform_binaries(
            Path("/staged"),
            "darwin-x64",
            VERSION,
            "native-or-metadata",
            "Intel execution requires an Intel runner",
        )

        self.assertFalse(executed)
        run_checked_binary.assert_not_called()

    @mock.patch.object(verify_release_packages, "run_checked_binary")
    @mock.patch.object(verify_release_packages.shutil, "which", return_value="/qemu")
    @mock.patch.object(verify_release_packages.sys, "platform", "linux")
    def test_linux_arm64_uses_qemu_for_all_four_executables(
        self,
        _which: mock.Mock,
        run_checked_binary: mock.Mock,
    ) -> None:
        executed = verify_release_packages.smoke_platform_binaries(
            Path("/staged"),
            "linux-arm64",
            VERSION,
            "qemu-aarch64",
            "",
        )

        self.assertTrue(executed)
        self.assertEqual(run_checked_binary.call_count, 4)
        for call in run_checked_binary.call_args_list:
            self.assertEqual(call.args[0][0], "/qemu")

    @mock.patch.object(verify_release_packages, "run_checked_binary")
    @mock.patch.object(
        verify_release_packages, "host_platform_key", return_value="linux-x64"
    )
    def test_native_linux_executes_packaged_preload_smoke(
        self,
        _host_platform_key: mock.Mock,
        run_checked_binary: mock.Mock,
    ) -> None:
        with tempfile.TemporaryDirectory() as temp:
            package_dir = Path(temp)
            bin_dir = package_dir / "bin"
            bin_dir.mkdir()
            (bin_dir / "libcontext_instruct_shim.so").write_bytes(b"fixture")

            executed = verify_release_packages.smoke_platform_binaries(
                package_dir,
                "linux-x64",
                VERSION,
                "native",
                "",
            )

        self.assertTrue(executed)
        self.assertEqual(run_checked_binary.call_count, 6)
        preload = run_checked_binary.call_args_list[4].args[0]
        self.assertEqual(preload[1:3], ["shell", "--root"])
        self.assertEqual(preload[-1], "/bin/true")

    @mock.patch.object(
        verify_release_packages, "host_platform_key", return_value="linux-arm64"
    )
    def test_native_mode_rejects_a_different_host(
        self, _host_platform_key: mock.Mock
    ) -> None:
        with self.assertRaisesRegex(
            verify_release_packages.VerificationError,
            "native smoke requires host linux-x64",
        ):
            verify_release_packages.smoke_platform_binaries(
                Path("/staged"),
                "linux-x64",
                VERSION,
                "native",
                "",
            )


class NpmDryRunTests(unittest.TestCase):
    @mock.patch.object(verify_release_packages.subprocess, "run")
    def test_npm_command_is_offline_and_non_publishing(
        self, run: mock.Mock
    ) -> None:
        run.return_value = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout="{}",
            stderr="",
        )
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            verify_release_packages.npm_command_json(
                root,
                ["publish", "--access", "public"],
                root / "cache",
                root / "npmrc",
            )

        command = run.call_args.args[0]
        self.assertIn("--dry-run", command)
        self.assertIn("--offline", command)
        self.assertIn("--ignore-scripts", command)

    def test_npm_result_rejects_non_executable_cli(self) -> None:
        result = {
            "name": "packet28",
            "version": VERSION,
            "files": [
                {"path": "package.json", "mode": 0o644},
                {"path": "bin/packet28.js", "mode": 0o644},
            ],
        }
        with self.assertRaisesRegex(
            verify_release_packages.VerificationError,
            "non-executable bin/packet28.js",
        ):
            verify_release_packages.validate_npm_result(
                result,
                "packet28",
                VERSION,
                {"package.json", "bin/packet28.js"},
                "pack",
            )


class SourceDryRunTests(unittest.TestCase):
    @mock.patch.object(verify_release_packages, "verify_npm_dry_run")
    def test_source_dry_run_materializes_all_five_packages(
        self, verify_npm_dry_run: mock.Mock
    ) -> None:
        observed_names: list[str] = []
        observed_modes: list[int] = []

        def observe(package_dir: Path, version: str) -> None:
            self.assertEqual(version, VERSION)
            manifest = json.loads(
                (package_dir / "package.json").read_text(encoding="utf-8")
            )
            observed_names.append(manifest["name"])
            for relative in manifest.get("bin", {}).values():
                observed_modes.append((package_dir / relative).stat().st_mode)

        verify_npm_dry_run.side_effect = observe
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "npm/packet28/bin").mkdir(parents=True)
            (root / "npm/platform-template").mkdir(parents=True)
            (root / "Cargo.toml").write_text(
                f'[workspace.package]\nversion = "{VERSION}"\n',
                encoding="utf-8",
            )
            root_manifest = {
                "name": "packet28",
                "version": VERSION,
                "bin": verify_release_packages.ROOT_BINARIES,
                "files": ["bin", "vendor"],
                "optionalDependencies": {
                    f"@packet28/{platform_key}": VERSION
                    for platform_key in verify_release_packages.PLATFORMS
                },
            }
            (root / "npm/packet28/package.json").write_text(
                json.dumps(root_manifest), encoding="utf-8"
            )
            for relative in verify_release_packages.ROOT_BINARIES.values():
                wrapper = root / "npm/packet28" / relative
                wrapper.write_text("#!/usr/bin/env node\n", encoding="utf-8")
                wrapper.chmod(0o644)
            for relative in verify_release_packages.ROOT_SUPPORT_FILES:
                support = root / "npm/packet28" / relative
                support.write_text("export {};\n", encoding="utf-8")
            template = {
                "name": "@packet28/PLATFORM",
                "version": VERSION,
                "description": "Platform binaries (PLATFORM)",
                "license": "MIT",
                "os": ["OS"],
                "cpu": ["CPU"],
                "files": ["bin"],
                "preferUnplugged": True,
            }
            (root / "npm/platform-template/package.json").write_text(
                json.dumps(template), encoding="utf-8"
            )

            verify_release_packages.verify_source_dry_run(root)

        self.assertEqual(
            observed_names,
            [
                "packet28",
                "@packet28/darwin-arm64",
                "@packet28/darwin-x64",
                "@packet28/linux-x64",
                "@packet28/linux-arm64",
            ],
        )
        self.assertTrue(all(mode & stat.S_IXUSR for mode in observed_modes))


if __name__ == "__main__":
    unittest.main()
