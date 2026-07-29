#!/usr/bin/env python3
"""Validate the observation-level architecture-audit ledger."""

from __future__ import annotations

import hashlib
import re
import sys
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
AUDIT_PATH = ROOT / "docs" / "audits" / "architecture-review-20260728-114543.html"
LEDGER_PATH = ROOT / "docs" / "architecture-audit-remediation-20260728.md"
AUDIT_SHA256 = "126de8fc65b42bf5000ad1744293a1507e4abf18c1cbabc57dbb9e1a217195a9"
AUDIT_BYTES = 64_714
AUDIT_LINES = 741
COORDINATE_MAP_SHA256 = "444d943ffaea56121d307b6ecd97ca0a35d962d186b78ef3e930f6c751f8aa22"


def lettered(prefix: str, suffixes: str) -> tuple[str, ...]:
    return tuple(f"{prefix}{suffix}" for suffix in suffixes)


ROW_GROUPS = {
    "provenance/verdict": (
        "PROV-01",
        "PROV-02",
        *lettered("PROV-03", "ABCDEF"),
        "PROV-04",
        *lettered("PROV-05", "ABC"),
        "PROV-06",
        "PROV-07",
        *lettered("PROV-08", "AB"),
        "SUM-01",
        "SUM-02",
    ),
    "scorecard": tuple(f"SCORE-{number:02d}" for number in range(1, 10)),
    "correctness/lifecycle": (
        *lettered("COR-01", "AB"),
        "COR-02",
        "COR-03",
        "COR-04",
        *lettered("COR-05", "AB"),
        *lettered("COR-06", "AB"),
        *lettered("COR-07", "AB"),
        *lettered("COR-08", "AB"),
    ),
    "crate/module architecture": (
        *lettered("ARC-01", "ABC"),
        *lettered("ARC-02", "AB"),
        "ARC-03",
        *lettered("ARC-04", "AB"),
        *lettered("ARC-05", "AB"),
        "ARC-06",
        "ARC-07",
        "ARC-08",
    ),
    "repro/CI/release": (
        "REP-01",
        "REP-02",
        *lettered("REP-03", "ABCD"),
        "REP-04",
        *lettered("REP-05", "ABCD"),
        "REP-06",
        "REP-07",
        *lettered("REP-08", "AB"),
        "REP-09",
        "REP-10",
    ),
    "public errors": tuple(f"API-01{suffix}" for suffix in "ABCDE"),
    "documentation": ("DOC-01", *lettered("DOC-02", "ABCD")),
    "test infrastructure": (
        "TST-01",
        "TST-02",
        *lettered("TST-03", "ABCD"),
        "TST-04",
        *lettered("TST-05", "ABC"),
        "TST-06",
        *lettered("TST-07", "AB"),
        *lettered("TST-08", "AB"),
    ),
    "persistence/performance": (
        *lettered("PER-01", "AB"),
        "PER-02",
        *lettered("PER-03", "AB"),
        *lettered("PER-04", "AB"),
        *lettered("PER-05", "AB"),
        *lettered("PER-06", "ABC"),
        "PER-07",
        "PER-08",
        *lettered("PER-09", "ABCD"),
        "PER-10",
        *lettered("PER-11", "ABC"),
    ),
    "Tokio boundary": (
        "ASY-00",
        *lettered("ASY-01", "ABC"),
        *lettered("ASY-02", "AB"),
        *lettered("ASY-03", "AB"),
        *lettered("ASY-04", "ABC"),
    ),
    "stable-prefix experiment": (
        "EXP-01",
        *lettered("EXP-02", "ABC"),
        *lettered("EXP-03", "ABCDEF"),
        *lettered("EXP-04", "ABCDE"),
        "EXP-05",
    ),
    "positive invariants": (
        "INV-01",
        "INV-02",
        "INV-03",
        *lettered("INV-04", "AB"),
        "INV-05",
        *lettered("INV-06", "ABCDE"),
    ),
    "sequence/order": (
        "SEQ-00",
        "SEQ-01",
        *lettered("SEQ-02", "ABC"),
        *lettered("SEQ-03", "ABCDEF"),
    ),
    "external references": tuple(f"EXT-{number:02d}" for number in range(1, 11)),
}
EXPECTED_IDS = frozenset(row_id for rows in ROW_GROUPS.values() for row_id in rows)

LEGACY_ALIASES = {
    "COR-01": ("COR-01A", "COR-01B"),
    "COR-05": ("COR-05A", "COR-05B"),
    "COR-06": ("COR-06A", "COR-06B"),
    "COR-07": ("COR-07A", "COR-07B"),
    "COR-08": ("COR-08A", "COR-08B"),
    "ARC-01": ("ARC-01A", "ARC-01B", "ARC-01C"),
    "ARC-02": ("ARC-02A", "ARC-02B"),
    "ARC-04": ("ARC-04A", "ARC-04B"),
    "ARC-05": ("ARC-05A", "ARC-05B"),
    "REP-03": ("REP-03A", "REP-03B", "REP-03C", "REP-03D"),
    "REP-05": ("REP-05A", "REP-05B", "REP-05C", "REP-05D"),
    "REP-08": ("REP-08A", "REP-08B"),
    "API-01": ("API-01A", "API-01B", "API-01C", "API-01D", "API-01E"),
    "DOC-02": ("DOC-02A", "DOC-02B", "DOC-02C", "DOC-02D"),
    "TST-03": ("TST-03A", "TST-03B", "TST-03C", "TST-03D"),
    "TST-05": ("TST-05A", "TST-05B", "TST-05C"),
    "TST-07": ("TST-07A", "TST-07B"),
    "TST-08": ("TST-08A", "TST-08B"),
    "PER-01": ("PER-01A", "PER-01B"),
    "PER-03": ("PER-03A", "PER-03B"),
    "PER-04": ("PER-04A", "PER-04B"),
    "PER-05": ("PER-05A", "PER-05B"),
    "PER-06": ("PER-06A", "PER-06B", "PER-06C"),
    "PER-09": ("PER-09A", "PER-09B", "PER-09C", "PER-09D"),
    "PER-11": ("PER-11A", "PER-11B", "PER-11C"),
    "ASY-01": ("ASY-01A", "ASY-01B", "ASY-01C"),
    "ASY-02": ("ASY-02A", "ASY-02B"),
    "ASY-03": ("ASY-03A", "ASY-03B"),
    "ASY-04": ("ASY-04A", "ASY-04B", "ASY-04C"),
    "EXP-02": ("EXP-02A", "EXP-02B", "EXP-02C"),
    "EXP-03": ("EXP-03A", "EXP-03B", "EXP-03C", "EXP-03D", "EXP-03E", "EXP-03F"),
    "EXP-04": ("EXP-04A", "EXP-04B", "EXP-04C", "EXP-04D", "EXP-04E"),
    "INV-04": ("INV-04A", "INV-04B"),
    "INV-06": ("INV-06A", "INV-06B", "INV-06C", "INV-06D", "INV-06E"),
}
SOURCE_RELATIONSHIPS = {
    "PROV-03C": ("alias_of", ("DOC-01",)),
    "PROV-07": ("alias_of", ("PER-10",)),
    "COR-01B": ("alias_of", ("REP-05C",)),
    "REP-05D": ("alias_of", ("PER-09A",)),
    "SUM-02": ("alias_of", ("SEQ-00",)),
    "COR-07B": ("alias_of", ("ASY-02A",)),
    "PROV-03E": ("facet_of", ("REP-05B", "REP-05C", "REP-05D")),
    "PROV-03B": ("facet_of", ("TST-07A", "INV-03")),
    "ASY-02B": ("facet_of", ("COR-04", "COR-07A", "COR-07B")),
    "SEQ-02C": ("facet_of", ("SEQ-02A",)),
}

VALIDATION_PREFIXES = (
    "HISTORICAL",
    "CONFIRMED CURRENT",
    "ALREADY FIXED",
    "PARTIALLY FIXED",
    "NOT REPRODUCIBLE",
    "PRESERVED",
    "REVALIDATION REQUIRED",
    "REFERENCE",
)
STATUSES = {
    "DONE",
    "PARTIAL",
    "PENDING",
    "REGRESSED",
    "EVIDENCE ONLY",
    "NOT REPRODUCIBLE",
    "PRESERVE",
    "REFERENCE",
}
ROW_RE = re.compile(r"^\|\s*([A-Z][A-Z0-9-]+)\s*\|")
SOURCE_ID_RE = re.compile(r"^[A-Z]+-\d{2}(?:[A-Z])?$")
COORDINATE_RE = re.compile(r"\bL(\d+)(?:[–-](\d+))?")
ANCHOR_RE = re.compile(r"#([a-z][a-z0-9-]+)")
COMMIT_RE = re.compile(r"`[0-9a-f]{7,40}`")
SOURCE_ROWS_BEGIN = "<!-- BEGIN: SOURCE-DERIVED-AUDIT-ROWS -->"
SOURCE_ROWS_END = "<!-- END: SOURCE-DERIVED-AUDIT-ROWS -->"
ALIAS_BEGIN = "<!-- BEGIN: LEGACY-AUDIT-ID-ALIASES -->"
ALIAS_END = "<!-- END: LEGACY-AUDIT-ID-ALIASES -->"
RELATIONSHIP_BEGIN = "<!-- BEGIN: SOURCE-AUDIT-RELATIONSHIPS -->"
RELATIONSHIP_END = "<!-- END: SOURCE-AUDIT-RELATIONSHIPS -->"


def markdown_cells(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def validate_text(ledger: str, audit_bytes: bytes) -> list[str]:
    errors: list[str] = []
    digest = hashlib.sha256(audit_bytes).hexdigest()
    if digest != AUDIT_SHA256:
        errors.append(f"audit SHA-256 is {digest}, expected {AUDIT_SHA256}")
    if len(audit_bytes) != AUDIT_BYTES:
        errors.append(f"audit size is {len(audit_bytes)} bytes, expected {AUDIT_BYTES}")

    audit = audit_bytes.decode("utf-8")
    audit_line_count = len(audit.splitlines())
    if audit_line_count != AUDIT_LINES:
        errors.append(f"audit has {audit_line_count} lines, expected {AUDIT_LINES}")

    if ledger.count(SOURCE_ROWS_BEGIN) != 1 or ledger.count(SOURCE_ROWS_END) != 1:
        return errors + ["ledger must contain exactly one source-derived row register"]
    before_rows, source_tail = ledger.split(SOURCE_ROWS_BEGIN, 1)
    source_block, _after_rows = source_tail.split(SOURCE_ROWS_END, 1)
    source_start_line = before_rows.count("\n") + 1

    expected_header = (
        "| ID | Source | Immutable baseline observation | Current-source validation | "
        "Implementation / evidence | Test / mechanical invariant | Focused evidence | "
        "Closing commit | Status |"
    )
    if source_block.count(expected_header) != len(ROW_GROUPS):
        errors.append(
            "source-derived tables do not all use the normalized nine-cell header"
        )

    rows: dict[str, tuple[int, list[str]]] = {}
    seen: Counter[str] = Counter()
    unexpected: set[str] = set()
    for block_line_number, line in enumerate(source_block.splitlines(), start=1):
        line_number = source_start_line + block_line_number
        match = ROW_RE.match(line)
        if not match:
            continue
        row_id = match.group(1)
        if not SOURCE_ID_RE.fullmatch(row_id):
            continue
        if row_id not in EXPECTED_IDS:
            unexpected.add(row_id)
            continue
        seen[row_id] += 1
        cells = markdown_cells(line)
        if len(cells) != 9:
            errors.append(
                f"ledger line {line_number} row {row_id} has {len(cells)} cells, expected 9"
            )
            continue
        rows[row_id] = (line_number, cells)

    actual_ids = set(rows)
    missing = sorted(EXPECTED_IDS - actual_ids)
    extra = sorted(unexpected)
    duplicates = sorted(row_id for row_id, count in seen.items() if count != 1)
    if missing:
        errors.append(f"missing source-derived rows: {', '.join(missing)}")
    if extra:
        errors.append(f"unexpected source-derived rows: {', '.join(extra)}")
    if duplicates:
        errors.append(f"duplicate source-derived rows: {', '.join(duplicates)}")
    if len(EXPECTED_IDS) != 176:
        errors.append(f"validator expected-ID set has {len(EXPECTED_IDS)} rows, expected 176")

    for row_id, (line_number, cells) in rows.items():
        (
            identifier,
            source,
            baseline,
            validation,
            implementation,
            mechanical,
            evidence,
            commit,
            status,
        ) = cells
        if identifier != row_id:
            errors.append(f"ledger line {line_number} has inconsistent ID {identifier}")
        for label, value in (
            ("source", source),
            ("baseline observation", baseline),
            ("current-source validation", validation),
            ("implementation/evidence", implementation),
            ("test/mechanical invariant", mechanical),
            ("focused evidence", evidence),
            ("closing commit", commit),
            ("status", status),
        ):
            if not value or value == "—":
                errors.append(f"ledger row {row_id} has empty {label}")

        coordinates = COORDINATE_RE.findall(source)
        if not coordinates:
            errors.append(f"ledger row {row_id} has no HTML line coordinate")
        for start_text, end_text in coordinates:
            start = int(start_text)
            end = int(end_text or start_text)
            if start < 1 or end < start or end > audit_line_count:
                errors.append(
                    f"ledger row {row_id} has invalid HTML coordinate L{start}–{end}"
                )
        for anchor in ANCHOR_RE.findall(source):
            if f'id="{anchor}"' not in audit:
                errors.append(f"ledger row {row_id} names unknown HTML anchor #{anchor}")
        if not validation.startswith(VALIDATION_PREFIXES):
            errors.append(
                f"ledger row {row_id} has unknown validation class: {validation}"
            )
        if status not in STATUSES:
            errors.append(f"ledger row {row_id} has unknown status: {status}")
        if status == "DONE" and COMMIT_RE.search(commit) is None:
            errors.append(f"ledger row {row_id} is DONE without a closing commit hash")

    if len(rows) == len(EXPECTED_IDS):
        coordinate_payload = "".join(
            f"{row_id}\t{rows[row_id][1][1]}\n" for row_id in sorted(rows)
        )
        coordinate_digest = hashlib.sha256(coordinate_payload.encode("utf-8")).hexdigest()
        if coordinate_digest != COORDINATE_MAP_SHA256:
            errors.append(
                "source-coordinate map changed: "
                f"{coordinate_digest}, expected {COORDINATE_MAP_SHA256}"
            )

    errors.extend(validate_aliases(ledger))
    errors.extend(validate_source_relationships(ledger))
    return errors


def validate_aliases(ledger: str) -> list[str]:
    errors: list[str] = []
    if ledger.count(ALIAS_BEGIN) != 1 or ledger.count(ALIAS_END) != 1:
        return ["ledger must contain exactly one legacy-alias register"]
    block = ledger.split(ALIAS_BEGIN, 1)[1].split(ALIAS_END, 1)[0]
    aliases: dict[str, tuple[str, ...]] = {}
    for line in block.splitlines():
        cells = markdown_cells(line)
        if len(cells) != 3 or not re.fullmatch(r"`[A-Z][A-Z0-9-]+`", cells[0]):
            continue
        legacy = cells[0].strip("`")
        targets = tuple(re.findall(r"`([A-Z][A-Z0-9-]+)`", cells[1]))
        if legacy in aliases:
            errors.append(f"duplicate legacy alias: {legacy}")
        aliases[legacy] = targets

    if aliases != LEGACY_ALIASES:
        missing = sorted(set(LEGACY_ALIASES) - set(aliases))
        extra = sorted(set(aliases) - set(LEGACY_ALIASES))
        changed = sorted(
            legacy
            for legacy in set(aliases).intersection(LEGACY_ALIASES)
            if aliases[legacy] != LEGACY_ALIASES[legacy]
        )
        if missing:
            errors.append(f"missing legacy aliases: {', '.join(missing)}")
        if extra:
            errors.append(f"unexpected legacy aliases: {', '.join(extra)}")
        if changed:
            errors.append(f"incorrect legacy alias targets: {', '.join(changed)}")
    for legacy, targets in aliases.items():
        unknown = sorted(set(targets) - EXPECTED_IDS)
        if unknown:
            errors.append(f"legacy alias {legacy} targets unknown rows: {', '.join(unknown)}")
    return errors


def validate_source_relationships(ledger: str) -> list[str]:
    errors: list[str] = []
    if ledger.count(RELATIONSHIP_BEGIN) != 1 or ledger.count(RELATIONSHIP_END) != 1:
        return ["ledger must contain exactly one source-relationship register"]
    block = ledger.split(RELATIONSHIP_BEGIN, 1)[1].split(RELATIONSHIP_END, 1)[0]
    relationships: dict[str, tuple[str, tuple[str, ...]]] = {}
    for line in block.splitlines():
        cells = markdown_cells(line)
        if len(cells) != 4 or not re.fullmatch(r"`[A-Z][A-Z0-9-]+`", cells[0]):
            continue
        source = cells[0].strip("`")
        relation = cells[1].strip("`")
        targets = tuple(re.findall(r"`([A-Z][A-Z0-9-]+)`", cells[2]))
        if source in relationships:
            errors.append(f"duplicate source relationship: {source}")
        relationships[source] = (relation, targets)

    if relationships != SOURCE_RELATIONSHIPS:
        missing = sorted(set(SOURCE_RELATIONSHIPS) - set(relationships))
        extra = sorted(set(relationships) - set(SOURCE_RELATIONSHIPS))
        changed = sorted(
            source
            for source in set(relationships).intersection(SOURCE_RELATIONSHIPS)
            if relationships[source] != SOURCE_RELATIONSHIPS[source]
        )
        if missing:
            errors.append(f"missing source relationships: {', '.join(missing)}")
        if extra:
            errors.append(f"unexpected source relationships: {', '.join(extra)}")
        if changed:
            errors.append(f"incorrect source relationships: {', '.join(changed)}")
    for source, (_relation, targets) in relationships.items():
        unknown = sorted(({source} | set(targets)) - EXPECTED_IDS)
        if unknown:
            errors.append(
                f"source relationship {source} names unknown rows: {', '.join(unknown)}"
            )
    alias_graph = {
        source: tuple(target for target in targets if target in relationships)
        for source, (relation, targets) in relationships.items()
        if relation == "alias_of"
    }
    for start in alias_graph:
        pending = list(alias_graph[start])
        visited: set[str] = set()
        while pending:
            target = pending.pop()
            if target == start:
                errors.append(f"source alias cycle includes {start}")
                break
            if target not in visited:
                visited.add(target)
                pending.extend(alias_graph.get(target, ()))
    return errors


def main() -> int:
    errors = validate_text(
        LEDGER_PATH.read_text(encoding="utf-8"),
        AUDIT_PATH.read_bytes(),
    )
    if errors:
        for error in errors:
            print(f"audit ledger invariant failed: {error}", file=sys.stderr)
        return 1
    print(
        "architecture audit ledger invariant passed: "
        f"{len(EXPECTED_IDS)} source rows, {len(LEGACY_ALIASES)} legacy aliases, "
        f"{len(SOURCE_RELATIONSHIPS)} source relationships"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
