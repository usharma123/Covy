#!/usr/bin/env python3
"""Verify Packet28's pinned Rust toolchain and thin Just task surface."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CURRENT_RUST = "1.93.1"
MSRV_RUST = "1.88.0"
RECIPE = re.compile(r"^(?P<name>[a-z][a-z0-9-]*)(?:\s+[^:]*)?:\s*$")

EXPECTED_RECIPES = {
    "default": ("@just --list",),
    "fmt": ("cargo fmt --all -- --check",),
    "check": (
        "cargo check --workspace --all-targets --all-features --locked",
    ),
    "build": (
        "cargo build --workspace --all-targets --all-features --locked",
    ),
    "lint": (
        "cargo clippy --workspace --all-targets --all-features --locked "
        "-- -D warnings",
    ),
    "test": (
        "cargo test --workspace --all-targets --all-features --locked",
    ),
    "doctest": ("cargo test --workspace --doc --all-features --locked",),
    "docs": (
        'env RUSTDOCFLAGS="-D warnings -D rustdoc::broken_intra_doc_links" '
        "cargo doc --workspace --all-features --no-deps --locked",
    ),
    "deny": ("cargo deny --locked check",),
    "fast": ("scripts/validate_refactor_batch.sh",),
    "ci": ("scripts/validate_full_gate.sh",),
    "msrv": (
        f"rustup run {MSRV_RUST} scripts/validate_full_gate.sh --msrv",
    ),
    "release-check": (
        'scripts/validate_full_gate.sh --release-tag "{{tag}}"',
    ),
    "package": (
        "python3 scripts/package_cargo_workspace.py",
    ),
}


def parse_recipes(text: str) -> dict[str, tuple[str, ...]]:
    """Return recipe names and their non-comment command lines."""

    recipes: dict[str, list[str]] = {}
    current: str | None = None
    for raw_line in text.splitlines():
        matched = RECIPE.fullmatch(raw_line)
        if matched:
            current = matched.group("name")
            recipes.setdefault(current, [])
            continue
        if raw_line and raw_line[0].isspace() and current is not None:
            command = raw_line.strip()
            if command and not command.startswith("#"):
                recipes[current].append(command)
            continue
        if raw_line.strip() and not raw_line.lstrip().startswith(("#", "set ")):
            current = None
    return {name: tuple(commands) for name, commands in recipes.items()}


def tooling_errors(toolchain_text: str, justfile_text: str) -> list[str]:
    """Return reproducibility or task-surface policy violations."""

    errors: list[str] = []
    try:
        document = tomllib.loads(toolchain_text)
    except tomllib.TOMLDecodeError as error:
        errors.append(f"rust-toolchain.toml is invalid TOML: {error}")
        document = {}

    toolchain = document.get("toolchain")
    if not isinstance(toolchain, dict):
        errors.append("rust-toolchain.toml lacks [toolchain]")
    else:
        if toolchain.get("channel") != CURRENT_RUST:
            errors.append(f"toolchain channel must be exactly {CURRENT_RUST}")
        if toolchain.get("profile") != "minimal":
            errors.append("toolchain profile must be minimal")
        components = toolchain.get("components")
        if components != ["clippy", "rustfmt"]:
            errors.append(
                "toolchain components must be exactly clippy and rustfmt"
            )

    recipes = parse_recipes(justfile_text)
    missing = sorted(set(EXPECTED_RECIPES).difference(recipes))
    extra = sorted(set(recipes).difference(EXPECTED_RECIPES))
    if missing:
        errors.append(f"Justfile is missing recipes: {', '.join(missing)}")
    if extra:
        errors.append(f"Justfile has unreviewed recipes: {', '.join(extra)}")
    for name, expected in EXPECTED_RECIPES.items():
        actual = recipes.get(name)
        if actual is not None and actual != expected:
            errors.append(
                f"Justfile recipe {name!r} must delegate exactly to "
                f"{expected!r}, found {actual!r}"
            )

    forbidden = ("--no-verify", "git push --force", "|| true")
    for marker in forbidden:
        if marker in justfile_text:
            errors.append(f"Justfile contains forbidden bypass: {marker}")
    return errors


def main() -> int:
    errors = tooling_errors(
        (ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"),
        (ROOT / "Justfile").read_text(encoding="utf-8"),
    )
    if errors:
        for error in errors:
            print(f"tooling policy invariant failed: {error}", file=sys.stderr)
        return 1
    print(
        "tooling policy invariant passed "
        f"(Rust {CURRENT_RUST}, {len(EXPECTED_RECIPES)} Just recipes)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
