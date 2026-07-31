#!/usr/bin/env python3
"""Verify staged release binaries and npm packages without publishing them."""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Sequence

try:
    from scripts.release_artifacts import (
        EXECUTABLES,
        LINUX_RUNTIME_LIBRARIES,
        ROOT_BINARIES,
        ROOT_SUPPORT_FILES,
        platform_artifacts,
    )
except ModuleNotFoundError:
    from release_artifacts import (  # type: ignore[no-redef]
        EXECUTABLES,
        LINUX_RUNTIME_LIBRARIES,
        ROOT_BINARIES,
        ROOT_SUPPORT_FILES,
        platform_artifacts,
    )

ROOT = Path(__file__).resolve().parent.parent
BINARY_NAMES = EXECUTABLES
NPM_TIMEOUT_SECONDS = 60
BINARY_TIMEOUT_SECONDS = 20


class VerificationError(RuntimeError):
    """A release package violates a checked invariant."""


@dataclass(frozen=True)
class PlatformSpec:
    """Expected binary and npm metadata for one published platform."""

    binary_format: str
    architecture: str
    npm_os: str
    npm_cpu: str


PLATFORMS = {
    "darwin-arm64": PlatformSpec("macho", "arm64", "darwin", "arm64"),
    "darwin-x64": PlatformSpec("macho", "x86_64", "darwin", "x64"),
    "linux-x64": PlatformSpec("elf", "x86_64", "linux", "x64"),
    "linux-arm64": PlatformSpec("elf", "arm64", "linux", "arm64"),
}


def require(condition: bool, message: str) -> None:
    """Raise a verification error when ``condition`` is false."""

    if not condition:
        raise VerificationError(message)


def read_json(path: Path) -> dict[str, Any]:
    """Read a JSON object with a release-focused diagnostic."""

    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise VerificationError(f"cannot read JSON object {path}: {error}") from error
    require(isinstance(value, dict), f"{path} must contain a JSON object")
    return value


def workspace_version(root: Path) -> str:
    """Return the workspace package version from ``Cargo.toml``."""

    cargo_path = root / "Cargo.toml"
    try:
        cargo = tomllib.loads(cargo_path.read_text(encoding="utf-8"))
        version = cargo["workspace"]["package"]["version"]
    except (OSError, tomllib.TOMLDecodeError, KeyError, TypeError) as error:
        raise VerificationError(
            f"cannot read workspace package version from {cargo_path}: {error}"
        ) from error
    require(isinstance(version, str) and version, "workspace version must be a string")
    return version


def platform_from_package_name(name: object) -> str | None:
    """Return a supported platform key for an npm package name."""

    if not isinstance(name, str) or not name.startswith("@packet28/"):
        return None
    platform_key = name.removeprefix("@packet28/")
    return platform_key if platform_key in PLATFORMS else None


def validate_platform_manifest(
    package_dir: Path, platform_key: str, expected_version: str
) -> dict[str, Any]:
    """Validate metadata for a staged platform package."""

    spec = PLATFORMS[platform_key]
    manifest_path = package_dir / "package.json"
    manifest = read_json(manifest_path)
    expected = {
        "name": f"@packet28/{platform_key}",
        "version": expected_version,
        "os": [spec.npm_os],
        "cpu": [spec.npm_cpu],
        "files": ["bin"],
        "preferUnplugged": True,
        "license": "MIT",
    }
    for key, value in expected.items():
        require(
            manifest.get(key) == value,
            f"{manifest_path}: expected {key}={value!r}, got {manifest.get(key)!r}",
        )
    return manifest


def validate_root_manifest(
    package_dir: Path, expected_version: str
) -> dict[str, Any]:
    """Validate metadata for the root npm package."""

    manifest_path = package_dir / "package.json"
    manifest = read_json(manifest_path)
    require(
        manifest.get("name") == "packet28",
        f"{manifest_path}: root package name must be 'packet28'",
    )
    require(
        manifest.get("version") == expected_version,
        f"{manifest_path}: expected version {expected_version!r}, "
        f"got {manifest.get('version')!r}",
    )
    require(
        manifest.get("bin") == ROOT_BINARIES,
        f"{manifest_path}: root bin mapping does not match the published CLI surface",
    )
    require(
        manifest.get("files") == ["bin", "vendor"],
        f"{manifest_path}: root files allowlist must be ['bin', 'vendor']",
    )
    expected_dependencies = {
        f"@packet28/{platform_key}": expected_version for platform_key in PLATFORMS
    }
    require(
        manifest.get("optionalDependencies") == expected_dependencies,
        f"{manifest_path}: optional platform dependencies must all use "
        f"{expected_version}",
    )
    return manifest


def binary_identity(path: Path) -> tuple[str, str]:
    """Read a thin ELF or Mach-O header and return format plus architecture."""

    try:
        with path.open("rb") as binary:
            header = binary.read(20)
    except OSError as error:
        raise VerificationError(f"cannot read release binary {path}: {error}") from error

    if header.startswith(b"\x7fELF"):
        require(len(header) >= 20, f"{path}: truncated ELF header")
        require(header[4] == 2, f"{path}: release ELF must be 64-bit")
        require(header[5] == 1, f"{path}: release ELF must be little-endian")
        elf_type = int.from_bytes(header[16:18], "little")
        require(
            elf_type in {2, 3},
            f"{path}: ELF must be an executable or position-independent executable",
        )
        machine = int.from_bytes(header[18:20], "little")
        architecture = {62: "x86_64", 183: "arm64"}.get(machine)
        require(
            architecture is not None,
            f"{path}: unsupported ELF machine value {machine}",
        )
        return ("elf", architecture)

    if header[:4] == b"\xcf\xfa\xed\xfe":
        require(len(header) >= 16, f"{path}: truncated Mach-O header")
        cpu_type = int.from_bytes(header[4:8], "little")
        file_type = int.from_bytes(header[12:16], "little")
        require(file_type == 2, f"{path}: Mach-O file type must be MH_EXECUTE")
        architecture = {
            0x01000007: "x86_64",
            0x0100000C: "arm64",
        }.get(cpu_type)
        require(
            architecture is not None,
            f"{path}: unsupported Mach-O CPU type {cpu_type:#x}",
        )
        return ("macho", architecture)

    fat_magics = {
        b"\xca\xfe\xba\xbe",
        b"\xbe\xba\xfe\xca",
        b"\xca\xfe\xba\xbf",
        b"\xbf\xba\xfe\xca",
    }
    if header[:4] in fat_magics:
        raise VerificationError(
            f"{path}: universal Mach-O is not allowed in a single-target package"
        )
    raise VerificationError(f"{path}: expected a 64-bit ELF or thin Mach-O binary")


def validate_platform_binaries(package_dir: Path, platform_key: str) -> None:
    """Validate file shape, mode, and architecture for all staged artifacts."""

    spec = PLATFORMS[platform_key]
    bin_dir = package_dir / "bin"
    require(bin_dir.is_dir(), f"{bin_dir}: staged bin directory is missing")
    entries = {entry.name for entry in bin_dir.iterdir()}
    expected_names = set(platform_artifacts(platform_key))
    require(
        entries == expected_names,
        f"{bin_dir}: expected exactly {sorted(expected_names)}, got {sorted(entries)}",
    )
    for name in platform_artifacts(platform_key):
        artifact = bin_dir / name
        require(
            not artifact.is_symlink(),
            f"{artifact}: release artifact may not be a symlink",
        )
        require(
            artifact.is_file(),
            f"{artifact}: release artifact must be a regular file",
        )
        mode = artifact.stat().st_mode
        require(
            bool(mode & stat.S_IXUSR),
            f"{artifact}: release artifact is not owner-executable",
        )
        actual = binary_identity(artifact)
        expected = (spec.binary_format, spec.architecture)
        require(
            actual == expected,
            f"{artifact}: expected {expected[0]} {expected[1]}, "
            f"got {actual[0]} {actual[1]}",
        )


def host_platform_key() -> str | None:
    """Map the current Python host to a release platform key."""

    host_os = {"darwin": "darwin", "linux": "linux"}.get(sys.platform)
    machine = platform.machine().lower()
    host_cpu = {
        "aarch64": "arm64",
        "arm64": "arm64",
        "amd64": "x64",
        "x86_64": "x64",
    }.get(machine)
    if host_os is None or host_cpu is None:
        return None
    key = f"{host_os}-{host_cpu}"
    return key if key in PLATFORMS else None


def run_checked_binary(command: Sequence[str], expected_text: str, label: str) -> None:
    """Run one release smoke command and check its identifying output."""

    try:
        completed = subprocess.run(
            list(command),
            check=False,
            capture_output=True,
            text=True,
            timeout=BINARY_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise VerificationError(f"{label} smoke could not run: {error}") from error
    output = f"{completed.stdout}\n{completed.stderr}"
    require(
        completed.returncode == 0,
        f"{label} smoke exited {completed.returncode}: {output.strip()}",
    )
    require(
        expected_text in output,
        f"{label} smoke output did not contain {expected_text!r}: {output.strip()}",
    )


def smoke_platform_binaries(
    package_dir: Path,
    platform_key: str,
    expected_version: str,
    run_mode: str,
    skip_reason: str,
) -> bool:
    """Execute staged binaries when the configured platform boundary permits it."""

    host_key = host_platform_key()
    prefix: list[str] = []
    if run_mode == "native":
        require(
            host_key == platform_key,
            f"native smoke requires host {platform_key}, got {host_key or 'unsupported'}",
        )
    elif run_mode == "native-or-metadata":
        if host_key != platform_key:
            require(
                bool(skip_reason.strip()),
                "cross-architecture metadata-only fallback requires an explicit reason",
            )
            print(
                f"binary execution skipped for {platform_key}: {skip_reason} "
                f"(host={host_key or 'unsupported'})"
            )
            return False
    elif run_mode == "qemu-aarch64":
        require(
            platform_key == "linux-arm64",
            "qemu-aarch64 mode is only valid for linux-arm64",
        )
        require(
            sys.platform == "linux",
            "qemu-aarch64 release smoke must run on a Linux host",
        )
        emulator = shutil.which("qemu-aarch64")
        require(emulator is not None, "qemu-aarch64 is not installed")
        prefix = [emulator]
    else:
        raise VerificationError(f"unknown binary smoke mode {run_mode!r}")

    bin_dir = package_dir / "bin"
    commands = (
        ("Packet28 --version", bin_dir / "Packet28", ["--version"], expected_version),
        (
            "packet28d --version",
            bin_dir / "packet28d",
            ["--version"],
            expected_version,
        ),
        ("p28 --help", bin_dir / "p28", ["--help"], "Usage:"),
        (
            "packet28-agent --help",
            bin_dir / "packet28-agent",
            ["--help"],
            "Usage:",
        ),
    )
    for label, binary, arguments, expected_text in commands:
        run_checked_binary(
            [*prefix, str(binary), *arguments],
            expected_text,
            label,
        )

    if platform_key.startswith("linux-") and run_mode == "native":
        require(
            (bin_dir / LINUX_RUNTIME_LIBRARIES[0]).is_file(),
            "native Linux preload smoke requires the packaged instruction shim",
        )
        with tempfile.TemporaryDirectory(prefix="packet28-preload-smoke-") as temp:
            try:
                run_checked_binary(
                    [
                        str(bin_dir / "Packet28"),
                        "shell",
                        "--root",
                        temp,
                        "/bin/true",
                    ],
                    "",
                    "Packet28 packaged Linux preload",
                )
            finally:
                run_checked_binary(
                    [
                        str(bin_dir / "Packet28"),
                        "daemon",
                        "stop",
                        "--root",
                        temp,
                    ],
                    "",
                    "Packet28 preload-smoke daemon stop",
                )
    return True


def npm_command_json(
    package_dir: Path, arguments: Sequence[str], cache_dir: Path, user_config: Path
) -> Any:
    """Run an offline npm dry-run command and parse its JSON output."""

    command = [
        "npm",
        *arguments,
        "--dry-run",
        "--json",
        "--ignore-scripts",
        "--offline",
        "--cache",
        str(cache_dir),
        "--userconfig",
        str(user_config),
    ]
    try:
        completed = subprocess.run(
            command,
            cwd=package_dir,
            check=False,
            capture_output=True,
            text=True,
            timeout=NPM_TIMEOUT_SECONDS,
            env={
                **os.environ,
                "NPM_CONFIG_AUDIT": "false",
                "NPM_CONFIG_FUND": "false",
                "NPM_CONFIG_UPDATE_NOTIFIER": "false",
            },
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise VerificationError(
            f"npm {' '.join(arguments)} could not run in {package_dir}: {error}"
        ) from error
    require(
        completed.returncode == 0,
        f"npm {' '.join(arguments)} failed in {package_dir}: "
        f"{completed.stderr.strip()}",
    )
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise VerificationError(
            f"npm {' '.join(arguments)} returned invalid JSON in {package_dir}: "
            f"{completed.stdout.strip()}"
        ) from error


def npm_result_object(result: Any, command_name: str) -> dict[str, Any]:
    """Normalize npm pack/publish JSON to one package result."""

    if command_name == "pack":
        require(
            isinstance(result, list) and len(result) == 1,
            "npm pack dry-run must describe exactly one package",
        )
        result = result[0]
    require(
        isinstance(result, dict),
        f"npm {command_name} dry-run result must be a JSON object",
    )
    return result


def validate_npm_result(
    result: dict[str, Any],
    expected_name: str,
    expected_version: str,
    expected_files: set[str],
    command_name: str,
    executable_files: set[str] | None = None,
) -> None:
    """Validate npm's generated package metadata and file allowlist."""

    require(
        result.get("name") == expected_name,
        f"npm {command_name} dry-run named {result.get('name')!r}, "
        f"expected {expected_name!r}",
    )
    require(
        result.get("version") == expected_version,
        f"npm {command_name} dry-run version was {result.get('version')!r}, "
        f"expected {expected_version!r}",
    )
    files = result.get("files")
    require(
        isinstance(files, list),
        f"npm {command_name} dry-run did not return a file list",
    )
    file_modes: dict[str, int] = {}
    for entry in files:
        require(
            isinstance(entry, dict)
            and isinstance(entry.get("path"), str)
            and isinstance(entry.get("mode"), int),
            f"npm {command_name} dry-run returned malformed file metadata",
        )
        file_modes[entry["path"]] = entry["mode"]
    require(
        set(file_modes) == expected_files,
        f"npm {command_name} dry-run expected files {sorted(expected_files)}, "
        f"got {sorted(file_modes)}",
    )
    required_executables = (
        {path for path in expected_files if path.startswith("bin/")}
        if executable_files is None
        else executable_files
    )
    for path, mode in file_modes.items():
        if path in required_executables:
            require(
                bool(mode & stat.S_IXUSR),
                f"npm {command_name} dry-run would publish non-executable {path}",
            )


def verify_npm_dry_run(package_dir: Path, expected_version: str) -> None:
    """Run offline npm pack and publish dry-runs for one staged package."""

    manifest = read_json(package_dir / "package.json")
    name = manifest.get("name")
    platform_key = platform_from_package_name(name)
    if name == "packet28":
        validate_root_manifest(package_dir, expected_version)
        expected_files = {
            "package.json",
            *(f"bin/{Path(path).name}" for path in ROOT_BINARIES.values()),
            *ROOT_SUPPORT_FILES,
        }
        executable_files = set(ROOT_BINARIES.values())
    elif platform_key is not None:
        validate_platform_manifest(package_dir, platform_key, expected_version)
        expected_files = {
            "package.json",
            *(f"bin/{name}" for name in platform_artifacts(platform_key)),
        }
        executable_files = expected_files - {"package.json"}
    else:
        raise VerificationError(
            f"{package_dir / 'package.json'}: unsupported package name {name!r}"
        )

    with tempfile.TemporaryDirectory(prefix="packet28-npm-dry-run-") as temp:
        temp_path = Path(temp)
        cache_dir = temp_path / "cache"
        user_config = temp_path / "npmrc"
        user_config.write_text("", encoding="utf-8")
        pack = npm_result_object(
            npm_command_json(package_dir, ["pack"], cache_dir, user_config),
            "pack",
        )
        publish = npm_result_object(
            npm_command_json(
                package_dir,
                ["publish", "--access", "public"],
                cache_dir,
                user_config,
            ),
            "publish",
        )
    require(isinstance(name, str), "validated npm package name must be a string")
    validate_npm_result(
        pack,
        name,
        expected_version,
        expected_files,
        "pack",
        executable_files,
    )
    validate_npm_result(
        publish,
        name,
        expected_version,
        expected_files,
        "publish",
        executable_files,
    )
    require(
        pack.get("integrity") == publish.get("integrity"),
        "npm pack and publish dry-runs produced different package integrity values",
    )
    print(f"offline npm pack/publish dry-run passed: {name}@{expected_version}")


def render_platform_manifest(
    template: dict[str, Any], platform_key: str, version: str
) -> dict[str, Any]:
    """Render one platform manifest from the checked-in release template."""

    spec = PLATFORMS[platform_key]
    rendered = dict(template)
    rendered["name"] = f"@packet28/{platform_key}"
    rendered["version"] = version
    rendered["description"] = str(rendered.get("description", "")).replace(
        "PLATFORM", platform_key
    )
    rendered["os"] = [spec.npm_os]
    rendered["cpu"] = [spec.npm_cpu]
    return rendered


def verify_source_dry_run(root: Path) -> None:
    """Materialize all npm package shapes and dry-run them before a release tag."""

    version = workspace_version(root)
    root_source = root / "npm" / "packet28"
    root_manifest = read_json(root_source / "package.json")
    require(
        root_manifest.get("version") == version,
        "root npm package version must match the Cargo workspace before release",
    )
    template_path = root / "npm" / "platform-template" / "package.json"
    template = read_json(template_path)
    require(
        template.get("version") == version,
        "platform npm template version must match the Cargo workspace before release",
    )
    require(
        template.get("name") == "@packet28/PLATFORM",
        f"{template_path}: platform name placeholder is missing",
    )

    with tempfile.TemporaryDirectory(prefix="packet28-source-packages-") as temp:
        staging = Path(temp)
        root_package = staging / "packet28"
        shutil.copytree(root_source, root_package)
        for relative in ROOT_BINARIES.values():
            script = root_package / relative
            require(script.is_file(), f"{script}: root npm CLI wrapper is missing")
            script.chmod(script.stat().st_mode | stat.S_IXUSR)
        for relative in ROOT_SUPPORT_FILES:
            support = root_package / relative
            require(support.is_file(), f"{support}: root npm support file is missing")
        verify_npm_dry_run(root_package, version)

        for platform_key in PLATFORMS:
            package_dir = staging / platform_key
            bin_dir = package_dir / "bin"
            bin_dir.mkdir(parents=True)
            manifest = render_platform_manifest(template, platform_key, version)
            (package_dir / "package.json").write_text(
                f"{json.dumps(manifest, indent=2)}\n",
                encoding="utf-8",
            )
            for name in platform_artifacts(platform_key):
                fixture = bin_dir / name
                fixture.write_bytes(b"Packet28 release-package dry-run fixture\n")
                fixture.chmod(0o755)
            verify_npm_dry_run(package_dir, version)


def verify_platform_package(
    package_dir: Path,
    platform_key: str,
    expected_version: str,
    run_mode: str,
    skip_reason: str,
) -> None:
    """Verify one real staged platform package before artifact upload."""

    require(platform_key in PLATFORMS, f"unsupported platform {platform_key!r}")
    validate_platform_manifest(package_dir, platform_key, expected_version)
    validate_platform_binaries(package_dir, platform_key)
    executed = smoke_platform_binaries(
        package_dir,
        platform_key,
        expected_version,
        run_mode,
        skip_reason,
    )
    verify_npm_dry_run(package_dir, expected_version)
    execution = "executed" if executed else "metadata-only"
    print(
        f"staged platform package passed: {platform_key}@{expected_version} "
        f"({execution})"
    )


def parse_args(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    """Parse the release verification command line."""

    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    source = subparsers.add_parser(
        "source",
        help="materialize every npm package shape and run offline dry-runs",
    )
    source.add_argument("--root", type=Path, default=ROOT)

    npm = subparsers.add_parser(
        "npm",
        help="run offline npm pack/publish dry-runs for one staged package",
    )
    npm.add_argument("--package-dir", type=Path, required=True)
    npm.add_argument("--version", required=True)

    platform_package = subparsers.add_parser(
        "platform",
        help="verify and smoke one staged binary package",
    )
    platform_package.add_argument("--package-dir", type=Path, required=True)
    platform_package.add_argument("--platform", choices=sorted(PLATFORMS), required=True)
    platform_package.add_argument("--version", required=True)
    platform_package.add_argument(
        "--run-mode",
        choices=("native", "native-or-metadata", "qemu-aarch64"),
        required=True,
    )
    platform_package.add_argument("--skip-reason", default="")
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    """Run the requested verification and return a process exit code."""

    args = parse_args(arguments)
    try:
        if args.command == "source":
            verify_source_dry_run(args.root.resolve())
        elif args.command == "npm":
            verify_npm_dry_run(args.package_dir.resolve(), args.version)
        else:
            verify_platform_package(
                args.package_dir.resolve(),
                args.platform,
                args.version,
                args.run_mode,
                args.skip_reason,
            )
    except VerificationError as error:
        print(f"release package verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
