#!/usr/bin/env python3
"""Verify immutable CI inputs and locked repository Cargo commands."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW_DIR = ROOT / ".github" / "workflows"
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")
USES = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)
CARGO_GRAPH_COMMAND = re.compile(
    r"""(?x)
    (?:
        ^ | :\s+ | run_cmd\s+ | &&\s+ | \|\|\s+ |
        ["']\s* | :-\s* | ^-\s+
    )
    cargo\s+(?:build|check|clippy|test|doc|package)\b
    """
)
WORKFLOW_FILES = sorted(
    [*WORKFLOW_DIR.glob("*.yml"), *WORKFLOW_DIR.glob("*.yaml")]
)
ACTION_FILES = [
    *WORKFLOW_FILES,
    ROOT / "scripts" / "ci" / "github-actions.yml",
]

LOCKED_COMMAND_FILES = [
    *WORKFLOW_FILES,
    ROOT / "scripts" / "validate_refactor_batch.sh",
    ROOT / "scripts" / "validate_full_gate.sh",
    ROOT / "scripts" / "ci" / "codex_autofix.sh",
    ROOT / "scripts" / "ci" / "github-actions.yml",
    ROOT / "scripts" / "ci" / "gitlab-ci.yml",
    ROOT / "scripts" / "run_packet28_claude_code_experiment.sh",
    ROOT / "scripts" / "bench_apache.sh",
    ROOT / "scripts" / "test_token_usage.py",
    ROOT / "npm" / "build-npm.sh",
    ROOT / "README.md",
]

CROSS_REVISION = "88f49ff79e777bef6d3564531636ee4d3cc2f8d2"
CARGO_DENY_VERSION = "0.20.2"
CARGO_DENY_CHECKSUMS = {
    "aarch64-apple-darwin": (
        "fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3"
    ),
    "aarch64-unknown-linux-musl": (
        "995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d"
    ),
    "x86_64-apple-darwin": (
        "248da7f581724e470071990c088ffc55c811981715f4cbdb258621fb79f8b7a6"
    ),
    "x86_64-unknown-linux-musl": (
        "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"
    ),
}
CURRENT_RUST = "1.93.1"
MSRV_RUST = "1.88.0"
APPROVED_ACTION_REVISIONS = {
    "actions/checkout": {
        "11d5960a326750d5838078e36cf38b85af677262",
        "fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
    },
    "actions/download-artifact": {
        "37930b1c2abaa49bbe596cd826c3c89aef350131"
    },
    "actions/setup-node": {
        "249970729cb0ef3589644e2896645e5dc5ba9c38",
        "a0853c24544627f65ddf259abe73b1d18a591444",
    },
    "actions/upload-artifact": {
        "b7c566a772e6b6bfb58ed0dc250532a479d7789f"
    },
    "dtolnay/rust-toolchain": {
        "4cda84d5c5c54efe2404f9d843567869ab1699d4"
    },
    "github/codeql-action/upload-sarif": {
        "4187e74d05793876e9989daffde9c3e66b4acd07"
    },
    "Swatinem/rust-cache": {
        "e18b497796c12c097a38f9edb9d0641fb99eee32"
    },
}


def relative(path: Path) -> str:
    return str(path.relative_to(ROOT))


def verify_action_references(errors: list[str]) -> None:
    count = 0
    for workflow in ACTION_FILES:
        text = workflow.read_text(encoding="utf-8")
        for value in USES.findall(text):
            count += 1
            if value.startswith("./"):
                continue
            if "@" not in value:
                errors.append(f"{relative(workflow)}: action has no revision: {value}")
                continue
            revision = value.rsplit("@", 1)[1]
            if not FULL_SHA.fullmatch(revision):
                errors.append(
                    f"{relative(workflow)}: action is not pinned to a full SHA: {value}"
                )
                continue
            action = value.rsplit("@", 1)[0]
            approved = APPROVED_ACTION_REVISIONS.get(action)
            if approved is None:
                errors.append(
                    f"{relative(workflow)}: action lacks an approved revision: {action}"
                )
            elif revision not in approved:
                errors.append(
                    f"{relative(workflow)}: action revision is not approved: {value}"
                )
    if count == 0:
        errors.append("no GitHub Action references were found")


def verify_rust_toolchains(errors: list[str]) -> None:
    msrv_count = 0
    for path in ACTION_FILES:
        lines = path.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            if "uses: dtolnay/rust-toolchain@" not in line:
                continue
            nearby = "\n".join(lines[index + 1 : index + 6])
            matched = re.search(r"^\s*toolchain:\s*(\S+)\s*$", nearby, re.MULTILINE)
            if matched is None:
                errors.append(
                    f"{relative(path)}:{index + 1}: Rust action lacks exact toolchain input"
                )
                continue
            toolchain = matched.group(1)
            if toolchain == MSRV_RUST:
                msrv_count += 1
                if path != WORKFLOW_DIR / "build.yml":
                    errors.append(
                        f"{relative(path)}:{index + 1}: MSRV toolchain is only valid in build.yml"
                    )
            elif toolchain != CURRENT_RUST:
                errors.append(
                    f"{relative(path)}:{index + 1}: unreviewed Rust toolchain {toolchain}"
                )
    if msrv_count != 1:
        errors.append(
            f"expected exactly one {MSRV_RUST} MSRV action, found {msrv_count}"
        )


def verify_locked_commands(errors: list[str]) -> None:
    for path in LOCKED_COMMAND_FILES:
        if not path.exists():
            errors.append(f"required CI/gate file is missing: {relative(path)}")
            continue
        for line_number, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), start=1
        ):
            stripped = line.strip()
            if stripped.startswith("echo "):
                continue
            if CARGO_GRAPH_COMMAND.search(stripped) and "--locked" not in line:
                errors.append(
                    f"{relative(path)}:{line_number}: Cargo graph command lacks --locked"
                )
            if re.search(r"\bcross\s+build\b", line) and "--locked" not in line:
                errors.append(
                    f"{relative(path)}:{line_number}: cross build lacks --locked"
                )


def verify_workflow_wiring(errors: list[str]) -> None:
    build = (WORKFLOW_DIR / "build.yml").read_text(encoding="utf-8")
    release = (WORKFLOW_DIR / "release.yml").read_text(encoding="utf-8")
    full_gate = (ROOT / "scripts" / "validate_full_gate.sh").read_text(
        encoding="utf-8"
    )

    if "scripts/validate_full_gate.sh" not in build:
        errors.append("build workflow does not invoke the canonical full gate")
    if "scripts/validate_full_gate.sh --msrv" not in build:
        errors.append("build workflow does not invoke the canonical MSRV gate")
    if 'scripts/validate_full_gate.sh --release-tag "$GITHUB_REF_NAME"' not in release:
        errors.append("release workflow does not run the tag-aware canonical gate")
    if "run_cmd python3 scripts/check_architecture.py" not in full_gate:
        errors.append("canonical gate does not run the architecture checker")
    if (
        "run_cmd python3 -m unittest scripts.tests.test_check_architecture"
        not in full_gate
    ):
        errors.append("canonical gate does not run architecture-checker unit tests")
    if "run_cmd python3 scripts/check_instruction_claims.py" not in full_gate:
        errors.append("canonical gate does not run the instruction-claim checker")
    if "run_cmd python3 scripts/check_rust_hazards.py" not in full_gate:
        errors.append("canonical gate does not run the Rust hazard-policy checker")
    if "run_cmd cargo deny --locked check" not in full_gate:
        errors.append("canonical gate does not run cargo-deny against the lockfile")

    expected_cross = (
        "cargo install cross --git https://github.com/cross-rs/cross "
        f"--rev {CROSS_REVISION} --locked"
    )
    if expected_cross not in release:
        errors.append("release cross installer is not pinned to the reviewed revision")
    if 'node-version: "22.23.1"' not in (
        WORKFLOW_DIR / "codex-autofix.yml"
    ).read_text(encoding="utf-8"):
        errors.append("Codex autofix Node toolchain is not pinned to 22.23.1")
    if "node-version: 20.20.2" not in release:
        errors.append("release Node toolchain is not pinned to 20.20.2")

    gitlab = (ROOT / "scripts" / "ci" / "gitlab-ci.yml").read_text(
        encoding="utf-8"
    )
    expected_gitlab_image = (
        "image: rust:1.93.1@sha256:"
        "ecbe59a8408895edd02d9ef422504b8501dd9fa1526de27a45b73406d734d659"
    )
    if expected_gitlab_image not in gitlab:
        errors.append("GitLab Rust image is not pinned by exact version and digest")

    for workflow_name in ("build.yml", "release.yml"):
        text = (WORKFLOW_DIR / workflow_name).read_text(encoding="utf-8")
        if 'scripts/install_cargo_deny.sh "$RUNNER_TEMP/cargo-deny-bin"' not in text:
            errors.append(
                f"{workflow_name} does not use the checksum-verifying cargo-deny installer"
            )
        if 'echo "$RUNNER_TEMP/cargo-deny-bin" >> "$GITHUB_PATH"' not in text:
            errors.append(f"{workflow_name} does not expose the verified cargo-deny")

    installer = (ROOT / "scripts" / "install_cargo_deny.sh").read_text(
        encoding="utf-8"
    )
    if f'version="{CARGO_DENY_VERSION}"' not in installer:
        errors.append("cargo-deny installer version is not the reviewed exact version")
    for target, checksum in CARGO_DENY_CHECKSUMS.items():
        if target not in installer or checksum not in installer:
            errors.append(
                f"cargo-deny installer lacks reviewed checksum for {target}"
            )
    required_installer_fragments = (
        "--proto '=https' --tlsv1.2",
        '"$actual_sha256" != "$expected_sha256"',
        '"$destination/cargo-deny" --version',
    )
    for fragment in required_installer_fragments:
        if fragment not in installer:
            errors.append(
                f"cargo-deny installer lacks integrity invariant: {fragment}"
            )


def main() -> int:
    errors: list[str] = []
    verify_action_references(errors)
    verify_rust_toolchains(errors)
    verify_locked_commands(errors)
    verify_workflow_wiring(errors)
    if errors:
        for error in errors:
            print(f"ci policy invariant failed: {error}", file=sys.stderr)
        return 1
    print("ci policy invariant passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
