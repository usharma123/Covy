from __future__ import annotations

import contextlib
import io
import re
import subprocess
import unittest
from unittest import mock

from scripts import check_architecture_audit_ledger as ledger


class ArchitectureAuditLedgerTests(unittest.TestCase):
    FINAL_COMMIT = "a" * 40
    FINAL_TREE = "b" * 40
    FINAL_ARTIFACT = "captured output at 2026-07-29T12:00:00Z"

    @classmethod
    def setUpClass(cls) -> None:
        cls.ledger_text = ledger.LEDGER_PATH.read_text(encoding="utf-8")
        cls.audit_bytes = ledger.AUDIT_PATH.read_bytes()

    @classmethod
    def final_gate_row(
        cls,
        gate: str,
        command: str = "`true`",
        result: str = "**PASS**",
        artifact: str | None = None,
    ) -> str:
        artifact = artifact if artifact is not None else cls.FINAL_ARTIFACT
        return f"| {gate} | {command} | {result} | {artifact} |"

    @classmethod
    def completed_ledger(
        cls,
        commit: str | None = None,
        tree: str | None = None,
    ) -> str:
        commit = commit if commit is not None else cls.FINAL_COMMIT
        tree = tree if tree is not None else cls.FINAL_TREE
        completed = re.sub(
            r"^\| Integration commit \| `[^`]+` \|$",
            f"| Integration commit | `{commit}` |",
            cls.ledger_text,
            count=1,
            flags=re.MULTILINE,
        )
        completed = re.sub(
            r"^\| Integration tree \| `[^`]+` \|$",
            f"| Integration tree | `{tree}` |",
            completed,
            count=1,
            flags=re.MULTILINE,
        )
        final_gate = "\n".join(
            [
                "| Gate | Exact command | Result | Artifact / timestamp |",
                "|---|---|---|---|",
                *(cls.final_gate_row(gate) for gate in ledger.REQUIRED_FINAL_GATES),
            ]
        )
        before, tail = completed.split(ledger.FINAL_GATE_BEGIN, 1)
        _old_block, after = tail.split(ledger.FINAL_GATE_END, 1)
        return (
            before
            + ledger.FINAL_GATE_BEGIN
            + "\n"
            + final_gate
            + "\n"
            + ledger.FINAL_GATE_END
            + after
        )

    def test_committed_provisional_ledger_is_exhaustive_and_traceable(self) -> None:
        self.assertEqual(ledger.validate_text(self.ledger_text, self.audit_bytes), [])

    def test_completed_finalization_is_accepted(self) -> None:
        self.assertEqual(
            ledger.validate_finalization(
                self.completed_ledger(),
                self.FINAL_COMMIT,
                self.FINAL_TREE,
            ),
            [],
        )

    def test_stale_integration_commit_and_tree_are_rejected(self) -> None:
        stale_commit = "c" * 40
        stale_tree = "d" * 40
        errors = ledger.validate_finalization(
            self.completed_ledger(stale_commit, stale_tree),
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        self.assertIn(
            "ledger-snapshot Integration commit is "
            f"{stale_commit}, expected {self.FINAL_COMMIT}",
            errors,
        )
        self.assertIn(
            "ledger-snapshot Integration tree is "
            f"{stale_tree}, expected {self.FINAL_TREE}",
            errors,
        )

    def test_pending_required_gate_is_rejected(self) -> None:
        completed = self.completed_ledger()
        passing = self.final_gate_row("Formatting")
        pending = self.final_gate_row(
            "Formatting",
            command="`PENDING`",
            result="PENDING",
            artifact="PENDING",
        )
        errors = ledger.validate_finalization(
            completed.replace(passing, pending, 1),
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        self.assertIn("final gate Formatting has a placeholder exact command", errors)
        self.assertIn("final gate Formatting result is not PASS: PENDING", errors)
        self.assertIn(
            "final gate Formatting has a placeholder artifact/timestamp",
            errors,
        )

    def test_known_fail_result_is_rejected(self) -> None:
        completed = self.completed_ledger()
        passing = self.final_gate_row("README generated statistics")
        known_fail = self.final_gate_row(
            "README generated statistics",
            result="**KNOWN FAIL**",
        )
        errors = ledger.validate_finalization(
            completed.replace(passing, known_fail, 1),
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        self.assertIn(
            "final gate README generated statistics result is not PASS: "
            "**KNOWN FAIL**",
            errors,
        )

    def test_provisional_pass_result_is_rejected(self) -> None:
        completed = self.completed_ledger()
        passing = self.final_gate_row("Ledger/source-anchor validation")
        provisional = self.final_gate_row(
            "Ledger/source-anchor validation",
            result="**PASS at provisional snapshot**",
        )
        errors = ledger.validate_finalization(
            completed.replace(passing, provisional, 1),
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        self.assertIn(
            "final gate Ledger/source-anchor validation result is not PASS: "
            "**PASS at provisional snapshot**",
            errors,
        )

    def test_duplicate_required_gate_is_rejected(self) -> None:
        completed = self.completed_ledger()
        row = self.final_gate_row("Formatting")
        errors = ledger.validate_finalization(
            completed.replace(row, f"{row}\n{row}", 1),
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        self.assertIn("duplicate final gates: Formatting", errors)

    def test_missing_required_gate_is_rejected(self) -> None:
        completed = self.completed_ledger()
        row = self.final_gate_row("Formatting")
        errors = ledger.validate_finalization(
            completed.replace(f"{row}\n", "", 1),
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        self.assertIn("missing required final gates: Formatting", errors)

    def test_final_cli_resolves_the_selected_source_revision(self) -> None:
        resolved = (self.FINAL_COMMIT, self.FINAL_TREE)
        with (
            mock.patch.object(ledger, "validate_text", return_value=[]),
            mock.patch.object(
                ledger,
                "resolve_source_revision",
                return_value=resolved,
            ) as resolve,
            mock.patch.object(
                ledger,
                "validate_finalization",
                return_value=[],
            ) as validate_final,
            mock.patch.object(
                ledger,
                "validate_closing_commits",
                return_value=[],
            ) as validate_commits,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(
                ledger.main(["--final", "--source-rev", "HEAD^"]),
                0,
            )
        resolve.assert_called_once_with("HEAD^")
        validate_final.assert_called_once_with(
            self.ledger_text,
            self.FINAL_COMMIT,
            self.FINAL_TREE,
        )
        validate_commits.assert_called_once_with(
            self.ledger_text,
            self.FINAL_COMMIT,
        )

    def test_source_revision_resolution_matches_git(self) -> None:
        commit = subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD^{commit}"],
            cwd=ledger.ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        ).stdout.strip()
        tree = subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD^{tree}"],
            cwd=ledger.ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        ).stdout.strip()
        self.assertEqual(ledger.resolve_source_revision("HEAD"), (commit, tree))

    def test_finalization_rejects_an_unresolvable_closing_commit(self) -> None:
        completed = self.completed_ledger().replace(
            "`8e18750`",
            "`deadbee`",
            1,
        )

        def resolve(reference: str) -> str:
            if reference == "deadbee":
                raise subprocess.CalledProcessError(
                    128,
                    ["git", "rev-parse"],
                    stderr="unknown revision",
                )
            return reference.ljust(40, "0")

        errors = ledger.validate_closing_commits(
            completed,
            self.FINAL_COMMIT,
            resolver=resolve,
            ancestor_check=lambda _ancestor, _descendant: True,
        )

        self.assertTrue(
            any(
                "closing commit deadbee for rows API-01E does not resolve"
                in error
                for error in errors
            )
        )

    def test_finalization_rejects_a_closing_commit_outside_source_history(
        self,
    ) -> None:
        completed = self.completed_ledger()

        errors = ledger.validate_closing_commits(
            completed,
            self.FINAL_COMMIT,
            resolver=lambda reference: reference.ljust(40, "0"),
            ancestor_check=lambda ancestor, _descendant: not ancestor.startswith(
                "8e18750"
            ),
        )

        self.assertTrue(
            any(
                "closing commit 8e18750 for rows API-01E is not reachable"
                in error
                for error in errors
            )
        )

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
