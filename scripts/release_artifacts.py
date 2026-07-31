#!/usr/bin/env python3
"""Authoritative public runtime-artifact contract for Packet28 releases."""

from __future__ import annotations

import argparse
from typing import Sequence


EXECUTABLES = ("Packet28", "packet28d", "p28", "packet28-agent")
LINUX_RUNTIME_LIBRARIES = ("libcontext_instruct_shim.so",)
ROOT_BINARIES = {
    "packet28": "bin/packet28.js",
    "packet28-agent": "bin/packet28-agent.js",
    "packet28-mcp": "bin/packet28-mcp.js",
    "p28": "bin/p28.js",
}
ROOT_SUPPORT_FILES = ("bin/native-launcher.js",)


def platform_artifacts(platform_key: str) -> tuple[str, ...]:
    """Return every artifact required by one platform package."""

    runtime = LINUX_RUNTIME_LIBRARIES if platform_key.startswith("linux-") else ()
    return (*EXECUTABLES, *runtime)


def parse_args(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("executables")
    platform = subparsers.add_parser("platform")
    platform.add_argument("--platform", required=True)
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    args = parse_args(arguments)
    values = (
        EXECUTABLES
        if args.command == "executables"
        else platform_artifacts(args.platform)
    )
    print("\n".join(values))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
