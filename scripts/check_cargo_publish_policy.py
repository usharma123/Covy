#!/usr/bin/env python3
"""Enforce Packet28's explicit Cargo publication and package-content policy."""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib
from typing import Mapping, Sequence


ROOT = Path(__file__).resolve().parents[1]
POLICY_PATH = Path("scripts/cargo_publish_policy.toml")
SEMVER = re.compile(
    r"^(?P<major>0|[1-9]\d*)\."
    r"(?P<minor>0|[1-9]\d*)\."
    r"(?P<patch>0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)


def run_cargo(root: Path, arguments: Sequence[str]) -> str:
    """Run a read-only Cargo policy command and return stdout."""

    result = subprocess.run(
        ["cargo", *arguments],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"cargo {' '.join(arguments)} failed: {detail}")
    return result.stdout


def load_metadata(root: Path) -> dict[str, object]:
    """Load the locked workspace package graph."""

    output = run_cargo(
        root,
        ("metadata", "--locked", "--no-deps", "--format-version", "1"),
    )
    document = json.loads(output)
    if not isinstance(document, dict):
        raise ValueError("cargo metadata did not return an object")
    return document


def load_policy(root: Path) -> dict[str, object]:
    """Load the repository's reviewed Cargo publication decision."""

    with (root / POLICY_PATH).open("rb") as policy_file:
        document = tomllib.load(policy_file)
    if not isinstance(document, dict):
        raise ValueError(f"{POLICY_PATH} did not contain a TOML table")
    return document


def workspace_packages(metadata: Mapping[str, object]) -> dict[str, dict[str, object]]:
    """Return workspace packages keyed by package name."""

    member_ids = set(metadata.get("workspace_members", []))
    packages: dict[str, dict[str, object]] = {}
    for candidate in metadata.get("packages", []):
        if not isinstance(candidate, dict) or candidate.get("id") not in member_ids:
            continue
        name = candidate.get("name")
        if isinstance(name, str):
            packages[name] = candidate
    return packages


def string_list(
    section: Mapping[str, object],
    key: str,
    label: str,
    errors: list[str],
    *,
    require_sorted: bool = True,
) -> list[str]:
    """Read a sorted, duplicate-free string list from a policy section."""

    value = section.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        errors.append(f"{label} must be a string array")
        return []
    items = list(value)
    if require_sorted and items != sorted(items):
        errors.append(f"{label} must be sorted")
    if len(items) != len(set(items)):
        errors.append(f"{label} contains duplicate entries")
    return items


def section(
    policy: Mapping[str, object], name: str, errors: list[str]
) -> Mapping[str, object]:
    """Return a policy table or record a type error."""

    value = policy.get(name)
    if not isinstance(value, dict):
        errors.append(f"policy is missing [{name}]")
        return {}
    return value


def parse_semver(value: str) -> tuple[int, int, int] | None:
    """Parse the numeric release prefix needed for internal requirements."""

    matched = SEMVER.match(value)
    if matched is None:
        return None
    return tuple(int(matched.group(part)) for part in ("major", "minor", "patch"))


def requirement_allows(requirement: str, version: str) -> bool:
    """Return whether a simple reviewed Cargo requirement accepts a version."""

    candidate = parse_semver(version)
    if candidate is None:
        return False
    if requirement == "*":
        return True

    operator = "^"
    base_text = requirement
    for prefix in ("^", "~", "="):
        if requirement.startswith(prefix):
            operator = prefix
            base_text = requirement[len(prefix) :]
            break
    base = parse_semver(base_text)
    if base is None:
        return False
    if operator == "=":
        return candidate == base
    if candidate < base:
        return False
    major, minor, patch = base
    if operator == "~":
        upper = (major, minor + 1, 0)
    elif major > 0:
        upper = (major + 1, 0, 0)
    elif minor > 0:
        upper = (0, minor + 1, 0)
    else:
        upper = (0, 0, patch + 1)
    return candidate < upper


def publication_order_errors(
    packages: Mapping[str, Mapping[str, object]],
    published: set[str],
    order: Sequence[str],
) -> list[str]:
    """Validate a complete dependency-first publication order."""

    errors: list[str] = []
    if published.difference(packages):
        return errors
    if len(order) != len(set(order)):
        errors.append("publish.order contains duplicate package names")
        return errors
    if set(order) != published:
        missing = sorted(published.difference(order))
        extra = sorted(set(order).difference(published))
        if missing:
            errors.append(f"publish.order is missing: {', '.join(missing)}")
        if extra:
            errors.append(f"publish.order contains private packages: {', '.join(extra)}")
        return errors

    position = {name: index for index, name in enumerate(order)}
    for name in order:
        package = packages[name]
        for dependency in package.get("dependencies", []):
            if not isinstance(dependency, dict):
                continue
            dependency_name = dependency.get("name")
            kind = dependency.get("kind")
            if (
                dependency_name in published
                and kind in (None, "build")
                and position[dependency_name] >= position[name]
            ):
                errors.append(
                    f"publish.order places {name} before its {dependency_name} dependency"
                )
    return errors


def package_file_errors(
    package: str,
    entries: Sequence[str],
    file_policy: Mapping[str, object],
) -> list[str]:
    """Return path traversal and sensitive-file violations for one package."""

    errors: list[str] = []
    local_errors: list[str] = []
    forbidden_components = {
        item.casefold()
        for item in string_list(
            file_policy,
            "forbidden_components",
            "package_files.forbidden_components",
            local_errors,
        )
    }
    forbidden_names = {
        item.casefold()
        for item in string_list(
            file_policy,
            "forbidden_names",
            "package_files.forbidden_names",
            local_errors,
        )
    }
    forbidden_suffixes = tuple(
        item.casefold()
        for item in string_list(
            file_policy,
            "forbidden_suffixes",
            "package_files.forbidden_suffixes",
            local_errors,
        )
    )
    errors.extend(local_errors)

    for raw_entry in entries:
        if not raw_entry or raw_entry.startswith("/") or "\\" in raw_entry:
            errors.append(f"{package}: unsafe package path {raw_entry!r}")
            continue
        path = PurePosixPath(raw_entry)
        lowered_parts = tuple(part.casefold() for part in path.parts)
        if ".." in path.parts:
            errors.append(f"{package}: package path escapes its crate: {raw_entry}")
            continue
        blocked = forbidden_components.intersection(lowered_parts)
        if blocked:
            errors.append(
                f"{package}: package path contains forbidden component "
                f"{sorted(blocked)[0]!r}: {raw_entry}"
            )
        lowered_name = path.name.casefold()
        if lowered_name in forbidden_names or lowered_name.endswith(forbidden_suffixes):
            errors.append(f"{package}: sensitive file would enter package: {raw_entry}")
    return errors


def package_symlink_errors(
    package: Mapping[str, object], entries: Sequence[str]
) -> list[str]:
    """Reject package-listed symlinks that resolve outside their crate."""

    manifest_path = package.get("manifest_path")
    name = package.get("name", "<unknown>")
    if not isinstance(manifest_path, str):
        return [f"{name}: Cargo metadata has no manifest path"]
    crate_root = Path(manifest_path).parent.resolve()
    errors: list[str] = []
    for entry in entries:
        source = crate_root / entry
        if not source.is_symlink():
            continue
        try:
            source.resolve().relative_to(crate_root)
        except (OSError, ValueError):
            errors.append(f"{name}: package symlink escapes its crate: {entry}")
    return errors


def policy_errors(
    metadata: Mapping[str, object],
    policy: Mapping[str, object],
    package_files: Mapping[str, Sequence[str]] | None = None,
) -> list[str]:
    """Return all Cargo publication, dependency, metadata, and file violations."""

    errors: list[str] = []
    if policy.get("schema_version") != 1:
        errors.append("cargo publish policy schema_version must be 1")
    registry = policy.get("registry")
    if registry != "crates-io":
        errors.append("cargo publish policy registry must be crates-io")
        registry = "crates-io"

    publish_section = section(policy, "publish", errors)
    private_section = section(policy, "private", errors)
    metadata_policy = section(policy, "metadata", errors)
    file_policy = section(policy, "package_files", errors)

    published_list = string_list(
        publish_section, "packages", "publish.packages", errors
    )
    order = string_list(
        publish_section,
        "order",
        "publish.order",
        errors,
        require_sorted=False,
    )
    private_list = string_list(private_section, "packages", "private.packages", errors)
    published = set(published_list)
    private = set(private_list)

    overlap = sorted(published.intersection(private))
    if overlap:
        errors.append(
            "packages cannot be both published and private: " + ", ".join(overlap)
        )

    packages = workspace_packages(metadata)
    workspace_names = set(packages)
    classified = published.union(private)
    unclassified = sorted(workspace_names.difference(classified))
    unknown = sorted(classified.difference(workspace_names))
    if unclassified:
        errors.append(f"workspace packages lack a publish decision: {', '.join(unclassified)}")
    if unknown:
        errors.append(f"publish policy names unknown packages: {', '.join(unknown)}")

    expected_metadata = {
        "license": metadata_policy.get("license"),
        "repository": metadata_policy.get("repository"),
        "homepage": metadata_policy.get("homepage"),
    }
    for field, expected in expected_metadata.items():
        if not isinstance(expected, str) or not expected:
            errors.append(f"metadata.{field} must be a non-empty string")
    generic_descriptions = set(
        string_list(
            metadata_policy,
            "generic_descriptions",
            "metadata.generic_descriptions",
            errors,
        )
    )

    for name, package in sorted(packages.items()):
        publish_value = package.get("publish")
        if name in private and publish_value != []:
            errors.append(f"{name}: private package must set publish = false")
        if name in published and publish_value != [registry]:
            errors.append(
                f"{name}: public package must allow only the {registry} registry"
            )

        for field, expected in expected_metadata.items():
            if isinstance(expected, str) and expected and package.get(field) != expected:
                errors.append(f"{name}: {field} metadata must be {expected!r}")
        description = package.get("description")
        if not isinstance(description, str) or not description.strip():
            errors.append(f"{name}: description metadata is missing")
        rust_version = package.get("rust_version")
        if not isinstance(rust_version, str) or not rust_version:
            errors.append(f"{name}: rust-version metadata is missing")

        if name in published:
            if description in generic_descriptions:
                errors.append(f"{name}: public description is generic, not crate-specific")
            readme = package.get("readme")
            if not isinstance(readme, str) or not readme:
                errors.append(f"{name}: public package readme metadata is missing")
            else:
                manifest_path = package.get("manifest_path")
                if isinstance(manifest_path, str):
                    crate_root = Path(manifest_path).parent.resolve()
                    readme_path = crate_root / readme
                    try:
                        readme_path.resolve().relative_to(crate_root)
                    except (OSError, ValueError):
                        errors.append(
                            f"{name}: public package readme escapes its crate: {readme}"
                        )
                    else:
                        if not readme_path.is_file():
                            errors.append(
                                f"{name}: public package readme does not exist: {readme}"
                            )
            keywords = package.get("keywords")
            if (
                not isinstance(keywords, list)
                or not all(isinstance(keyword, str) and keyword for keyword in keywords)
                or not 1 <= len(keywords) <= 5
            ):
                errors.append(f"{name}: public package needs one to five keywords")
            categories = package.get("categories")
            if (
                not isinstance(categories, list)
                or not all(
                    isinstance(category, str) and category for category in categories
                )
                or not 1 <= len(categories) <= 5
            ):
                errors.append(f"{name}: public package needs one to five categories")

        for dependency in package.get("dependencies", []):
            if not isinstance(dependency, dict):
                continue
            dependency_name = dependency.get("name")
            dependency_path = dependency.get("path")
            if not isinstance(dependency_name, str):
                if dependency_path is not None:
                    errors.append(
                        f"{name}: path dependency {dependency_name!r} is outside the workspace"
                    )
                continue
            if dependency_name not in packages:
                if dependency_path is None:
                    continue
                errors.append(
                    f"{name}: path dependency {dependency_name!r} is outside the workspace"
                )
                continue

            dependency_manifest = packages[dependency_name].get("manifest_path")
            expected_path = (
                Path(dependency_manifest).parent.resolve()
                if isinstance(dependency_manifest, str)
                else None
            )
            actual_path = (
                Path(dependency_path).resolve()
                if isinstance(dependency_path, str)
                else None
            )
            if actual_path is None:
                errors.append(
                    f"{name}: internal dependency {dependency_name} must resolve "
                    "through its workspace path"
                )
            elif expected_path is None or actual_path != expected_path:
                errors.append(
                    f"{name}: internal dependency {dependency_name} resolves to "
                    f"{actual_path}, expected {expected_path}"
                )

            requirement = dependency.get("req")
            dependency_version = packages[dependency_name].get("version")
            if requirement == "*":
                errors.append(
                    f"{name}: internal dependency {dependency_name} requirement "
                    "'*' is unconstrained"
                )
            elif (
                not isinstance(requirement, str)
                or not isinstance(dependency_version, str)
                or not requirement_allows(requirement, dependency_version)
            ):
                errors.append(
                    f"{name}: internal dependency {dependency_name} requirement "
                    f"{requirement!r} excludes workspace version {dependency_version!r}"
                )
            if name in published and dependency_name not in published:
                kind = dependency.get("kind") or "normal"
                errors.append(
                    f"{name}: public package has unpublished {kind} dependency "
                    f"{dependency_name}"
                )

    errors.extend(publication_order_errors(packages, published, order))

    if package_files is not None:
        missing_lists = sorted(workspace_names.difference(package_files))
        extra_lists = sorted(set(package_files).difference(workspace_names))
        if missing_lists:
            errors.append(
                "package file inventory is missing: " + ", ".join(missing_lists)
            )
        if extra_lists:
            errors.append(
                "package file inventory contains unknown packages: "
                + ", ".join(extra_lists)
            )
        for name, entries in sorted(package_files.items()):
            errors.extend(package_file_errors(name, entries, file_policy))
            package = packages.get(name)
            if package is not None:
                errors.extend(package_symlink_errors(package, entries))
    return errors


def collect_package_files(
    root: Path, packages: Mapping[str, Mapping[str, object]]
) -> dict[str, list[str]]:
    """Ask Cargo for the exact source files assembled for every member."""

    inventories: dict[str, list[str]] = {}
    for name in sorted(packages):
        output = run_cargo(
            root,
            ("package", "--locked", "--allow-dirty", "--list", "-p", name),
        )
        inventories[name] = [line for line in output.splitlines() if line]
    return inventories


def main(argv: Sequence[str] | None = None) -> int:
    """Run the repository policy check."""

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    root = args.root.resolve()

    try:
        metadata = load_metadata(root)
        policy = load_policy(root)
        packages = workspace_packages(metadata)
        package_files = collect_package_files(root, packages)
        errors = policy_errors(metadata, policy, package_files)
    except (OSError, RuntimeError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"cargo publish policy check failed: {error}", file=sys.stderr)
        return 1

    if errors:
        for error in errors:
            print(f"cargo publish policy invariant failed: {error}", file=sys.stderr)
        return 1

    policy_publish = policy.get("publish")
    published = (
        policy_publish.get("packages")
        if isinstance(policy_publish, dict)
        else []
    )
    published_count = len(published) if isinstance(published, list) else 0
    print(
        "cargo publish policy invariant passed "
        f"({len(packages)} members, {published_count} crates.io packages)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
