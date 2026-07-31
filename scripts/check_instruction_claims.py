#!/usr/bin/env python3
"""Reject unsupported instruction-cache savings claims in user-facing evidence."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DIRECTORIES = ("docs", "benchmarks", "releases", "website")
ROOT_PROSE_SUFFIXES = frozenset({".html", ".md", ".rst", ".txt"})
EVIDENCE_SUFFIXES = frozenset(
    {".html", ".json", ".jsonl", ".md", ".rst", ".txt"}
)

QUALIFIER = re.compile(
    r"""(?ix)
    \b(?:
        historical(?:ly)?
        | hypoth(?:esis|eses|etical(?:ly)?)
        | estimat(?:e|ed|es|ing|ion)
        | unverified
        | unsupported
        | not\s+(?:yet\s+)?established
        | not\s+(?:yet\s+)?verified
        | evidence[-\s]+only
        | provider[-\s]+measured
    )\b
    """
)

TOKEN_AMOUNT = (
    r"(?:20\s*k\b|20(?:[,_\s]?000)\b|twenty(?:[\s-]+)thousand\b)"
)
TOKEN_QUANTITY = (
    rf"{TOKEN_AMOUNT}\s*[- ]?\s*"
    r"(?:(?:prompt(?:[- ]cache)?|cache|context)[-\s]+)?tokens?\b"
)
TOKEN_LOSS = (
    r"\b(?:lost|loss|los(?:e|es|ing)|wast(?:e|es|ed|ing|age)|"
    r"discard(?:s|ed|ing)?)\b"
)
HUNDRED_PERCENT = (
    r"(?:100\s*%|100\s+percent\b|one(?:[\s-]+)hundred(?:[\s-]+)percent\b)"
)
CACHE = r"\b(?:(?:provider|prompt|instruction|prefix)[-\s]+)?cache\b"
MISS = r"\b(?:miss(?:es)?|miss[-\s]+rate)\b"
CACHE_MISS = rf"(?:{CACHE}.{{0,40}}{MISS}|{MISS}.{{0,40}}{CACHE})"
GUARANTEE = r"\bguarantee(?:d|s|ing)?\b"
NET_SAVINGS = (
    r"\bnet(?:[-\s]+(?:token|cost|prompt|cache))?"
    r"[-\s]+sav(?:e|es|ed|ing|ings)\b"
)
QUALITY_METRIC = (
    r"\b(?:accuracy|coverage|precision|recall|exact[-\s]+match(?:[-\s]+rate)?)\b"
)


def _either_order(left: str, right: str, distance: int) -> re.Pattern[str]:
    return re.compile(
        rf"(?:{left}.{{0,{distance}}}{right}|{right}.{{0,{distance}}}{left})",
        re.IGNORECASE,
    )


@dataclass(frozen=True)
class ClaimRule:
    identifier: str
    description: str
    pattern: re.Pattern[str]


RULES = (
    ClaimRule(
        identifier="fixed_token_loss",
        description="fixed 20K-token-loss claim",
        pattern=_either_order(TOKEN_QUANTITY, TOKEN_LOSS, 48),
    ),
    ClaimRule(
        identifier="total_cache_miss",
        description="100% cache-miss claim",
        pattern=re.compile(
            rf"^(?=.*{HUNDRED_PERCENT})(?=.*{CACHE_MISS})",
            re.IGNORECASE,
        ),
    ),
    ClaimRule(
        identifier="guaranteed_net_savings",
        description="guaranteed net-savings claim",
        pattern=_either_order(GUARANTEE, NET_SAVINGS, 72),
    ),
)

PERFECT_QUALITY_METRIC = _either_order(HUNDRED_PERCENT, QUALITY_METRIC, 80)
EXPLICIT_CACHE_MISS_RATE = re.compile(
    rf"^(?=.*{HUNDRED_PERCENT})(?=.*{CACHE}.{{0,40}}\bmiss[-\s]+rate\b)",
    re.IGNORECASE,
)
CACHE_MISSES_EQUAL_PERCENT = re.compile(
    rf"(?:"
    rf"{CACHE}.{{0,40}}{MISS}\s+"
    rf"(?:are|is|was|were|remain(?:ed|s)?|reached|hit|at|=)\s*"
    rf"{HUNDRED_PERCENT}"
    rf"|{HUNDRED_PERCENT}\s+"
    rf"(?:of\s+)?{CACHE}.{{0,40}}{MISS}"
    rf")",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class Violation:
    path: Path
    line_number: int
    rule: ClaimRule
    line: str


def _is_supported_evidence_file(path: Path) -> bool:
    return path.is_file() and path.suffix.lower() in EVIDENCE_SUFFIXES


def _files_under(path: Path) -> Iterable[Path]:
    if _is_supported_evidence_file(path):
        yield path
        return
    if not path.is_dir():
        return
    for candidate in path.rglob("*"):
        if _is_supported_evidence_file(candidate):
            yield candidate


def default_evidence_files(root: Path) -> list[Path]:
    """Return deterministic user-facing documentation and evidence inputs."""

    candidates: set[Path] = {
        path
        for path in root.iterdir()
        if path.is_file() and path.suffix.lower() in ROOT_PROSE_SUFFIXES
    }
    for directory in DEFAULT_DIRECTORIES:
        candidates.update(_files_under(root / directory))
    return sorted(candidates, key=lambda path: path.as_posix())


def requested_evidence_files(root: Path, requested: Sequence[str]) -> list[Path]:
    """Resolve explicit file/directory operands without silently ignoring them."""

    candidates: set[Path] = set()
    for raw_path in requested:
        path = Path(raw_path)
        if not path.is_absolute():
            path = root / path
        if not path.exists():
            raise FileNotFoundError(f"claim-check input does not exist: {raw_path}")
        if path.is_file():
            candidates.add(path)
        else:
            candidates.update(_files_under(path))
    return sorted(candidates, key=lambda path: path.as_posix())


def find_violations(paths: Iterable[Path]) -> list[Violation]:
    """Return all unqualified claim violations in deterministic order."""

    violations: list[Violation] = []
    for path in sorted(paths, key=lambda candidate: candidate.as_posix()):
        text = path.read_text(encoding="utf-8")
        for line_number, line in enumerate(text.splitlines(), start=1):
            if QUALIFIER.search(line):
                continue
            for rule in RULES:
                if (
                    rule.identifier == "total_cache_miss"
                    and PERFECT_QUALITY_METRIC.search(line)
                    and not EXPLICIT_CACHE_MISS_RATE.search(line)
                    and not CACHE_MISSES_EQUAL_PERCENT.search(line)
                ):
                    continue
                if rule.pattern.search(line):
                    violations.append(
                        Violation(
                            path=path,
                            line_number=line_number,
                            rule=rule,
                            line=line.strip(),
                        )
                    )
    return violations


def display_path(path: Path, root: Path) -> str:
    try:
        return path.relative_to(root).as_posix()
    except ValueError:
        return path.as_posix()


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Reject unqualified fixed token-loss, total cache-miss, and "
            "guaranteed net-savings claims in user-facing evidence."
        )
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="repository root used for default inputs and relative paths",
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help=(
            "files or directories to inspect; defaults to root prose plus "
            "docs, benchmarks, releases, and website"
        ),
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    root = args.root.resolve()
    try:
        paths = (
            requested_evidence_files(root, args.paths)
            if args.paths
            else default_evidence_files(root)
        )
        violations = find_violations(paths)
    except (OSError, UnicodeError) as error:
        print(f"instruction claim check failed: {error}", file=sys.stderr)
        return 2

    if violations:
        for violation in violations:
            print(
                "instruction claim invariant failed: "
                f"{display_path(violation.path, root)}:{violation.line_number}: "
                f"{violation.rule.identifier}: {violation.line}",
                file=sys.stderr,
            )
        return 1

    print(f"instruction claim invariant passed ({len(paths)} files checked)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
