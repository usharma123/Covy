from __future__ import annotations

import unittest

from scripts import check_architecture_audit_ledger as ledger


class ArchitectureAuditLedgerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.ledger_text = ledger.LEDGER_PATH.read_text(encoding="utf-8")
        cls.audit_bytes = ledger.AUDIT_PATH.read_bytes()

    def test_committed_ledger_is_exhaustive_and_traceable(self) -> None:
        self.assertEqual(ledger.validate_text(self.ledger_text, self.audit_bytes), [])

    def test_missing_source_row_is_rejected(self) -> None:
        mutated = "\n".join(
            line
            for line in self.ledger_text.splitlines()
            if not line.startswith("| PROV-01 |")
        )
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertTrue(any("missing source-derived rows: PROV-01" in error for error in errors))

    def test_missing_coordinate_is_rejected(self) -> None:
        mutated = self.ledger_text.replace("Header L45–55", "Header", 1)
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("ledger row PROV-01 has no HTML line coordinate", errors)

    def test_different_valid_coordinate_is_rejected(self) -> None:
        mutated = self.ledger_text.replace("Header L45–55", "Header L48–51", 1)
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertTrue(any("source-coordinate map changed" in error for error in errors))

    def test_unknown_source_row_is_rejected(self) -> None:
        row = (
            "| FAKE-01 | Header L45–55 | Baseline. | HISTORICAL | Preserve. | "
            "Check. | Evidence. | Reference only | EVIDENCE ONLY |\n"
        )
        mutated = self.ledger_text.replace(
            ledger.SOURCE_ROWS_END,
            row + ledger.SOURCE_ROWS_END,
            1,
        )
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("unexpected source-derived rows: FAKE-01", errors)

    def test_duplicate_source_row_is_rejected(self) -> None:
        row = next(
            line for line in self.ledger_text.splitlines() if line.startswith("| PROV-01 |")
        )
        mutated = self.ledger_text.replace(row, f"{row}\n{row}", 1)
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("duplicate source-derived rows: PROV-01", errors)

    def test_blank_required_field_is_rejected(self) -> None:
        mutated = self.ledger_text.replace(
            "| HISTORICAL | Preserve the exact artifact identity",
            "|  | Preserve the exact artifact identity",
            1,
        )
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("ledger row PROV-01 has empty current-source validation", errors)

    def test_done_row_requires_a_commit_hash(self) -> None:
        mutated = self.ledger_text.replace(
            "| `8e18750` | DONE |",
            "| PENDING | DONE |",
            1,
        )
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("ledger row API-01E is DONE without a closing commit hash", errors)

    def test_legacy_alias_drift_is_rejected(self) -> None:
        mutated = self.ledger_text.replace(
            "| `COR-01` | `COR-01A`, `COR-01B` |",
            "| `COR-01` | `COR-01A` |",
            1,
        )
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("incorrect legacy alias targets: COR-01", errors)

    def test_source_relationship_drift_is_rejected(self) -> None:
        mutated = self.ledger_text.replace(
            "| `PROV-03C` | `alias_of` | `DOC-01` |",
            "| `PROV-03C` | `alias_of` | `DOC-02A` |",
            1,
        )
        errors = ledger.validate_text(mutated, self.audit_bytes)
        self.assertIn("incorrect source relationships: PROV-03C", errors)

    def test_audit_artifact_drift_is_rejected(self) -> None:
        errors = ledger.validate_text(self.ledger_text, self.audit_bytes + b" ")
        self.assertTrue(any("audit SHA-256" in error for error in errors))
        self.assertTrue(any("audit size" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
