from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from scripts import check_instruction_claims


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_instruction_claims.py"


class InstructionClaimCheckTests(unittest.TestCase):
    def violations_for(self, line: str) -> list[check_instruction_claims.Violation]:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "summary.md"
            path.write_text(f"{line}\n", encoding="utf-8")
            return check_instruction_claims.find_violations([path])

    def test_rejects_unqualified_claim_variants(self) -> None:
        cases = {
            "The stable prefix loses 20K tokens on every request.": "fixed_token_loss",
            "20,000 prompt tokens were lost after compaction.": "fixed_token_loss",
            "Twenty-thousand cache tokens are wasted per handoff.": "fixed_token_loss",
            "Provider cache has a 100% miss rate.": "total_cache_miss",
            "Prompt-cache misses are 100 percent.": "total_cache_miss",
            "This mode guarantees net token savings.": "guaranteed_net_savings",
            "Net savings are guaranteed for every provider.": "guaranteed_net_savings",
        }

        for line, expected_rule in cases.items():
            with self.subTest(line=line):
                violations = self.violations_for(line)
                self.assertEqual(
                    [violation.rule.identifier for violation in violations],
                    [expected_rule],
                )

    def test_accepts_explicit_evidence_qualifiers(self) -> None:
        lines = (
            "Historical: 20K tokens were lost in an earlier sampled run.",
            "Hypothesis: provider cache has a 100% miss rate.",
            "Estimated 20,000 prompt tokens were lost after compaction.",
            "The 100% cache miss rate remains unverified.",
            "Unsupported claim: this mode guarantees net savings.",
            "Guaranteed net savings are not established.",
            "Evidence-only: net savings are guaranteed in the model.",
            "Provider-measured: prompt cache misses were 100%.",
        )

        for line in lines:
            with self.subTest(line=line):
                self.assertEqual(self.violations_for(line), [])

    def test_does_not_flag_unrelated_perfect_metrics(self) -> None:
        lines = (
            "Parser accuracy is 100%.",
            "Coverage reached 100%.",
            "The exact-match rate was 100 percent.",
            "The cache hit rate was 100%.",
            "Instruction adherence was 100% in this fixture.",
            "Cache-miss branch coverage is 100%.",
            "100% coverage for prompt-cache miss failure paths.",
            "Cache miss detection accuracy reached 100%.",
        )

        for line in lines:
            with self.subTest(line=line):
                self.assertEqual(self.violations_for(line), [])

    def test_default_scan_covers_docs_benchmarks_and_generated_summaries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "docs" / "experiments" / "run").mkdir(parents=True)
            (root / "benchmarks").mkdir()
            (root / "src").mkdir()
            (root / "README.md").write_text("Coverage is 100%.\n", encoding="utf-8")
            (root / "docs" / "guide.md").write_text(
                "Provider cache has a 100% miss rate.\n", encoding="utf-8"
            )
            (root / "docs" / "experiments" / "run" / "summary.json").write_text(
                '{"summary":"This guarantees net savings."}\n', encoding="utf-8"
            )
            (root / "benchmarks" / "summary.md").write_text(
                "Hypothesis: 20K tokens were lost.\n", encoding="utf-8"
            )
            (root / "src" / "ignored.rs").write_text(
                "Provider cache has a 100% miss rate.\n", encoding="utf-8"
            )

            files = check_instruction_claims.default_evidence_files(root)
            violations = check_instruction_claims.find_violations(files)

            self.assertEqual(
                [
                    (
                        violation.path.relative_to(root).as_posix(),
                        violation.rule.identifier,
                    )
                    for violation in violations
                ],
                [
                    ("docs/experiments/run/summary.json", "guaranteed_net_savings"),
                    ("docs/guide.md", "total_cache_miss"),
                ],
            )

    def test_cli_reports_sorted_locations_and_nonzero_status(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "docs").mkdir()
            (root / "docs" / "z.md").write_text(
                "This guarantees net savings.\n", encoding="utf-8"
            )
            (root / "docs" / "a.md").write_text(
                "20K tokens were lost.\n", encoding="utf-8"
            )

            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(root)],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            locations = [
                line.split(": ", 1)[1].split(": ", 1)[0]
                for line in result.stderr.splitlines()
            ]
            self.assertEqual(locations, ["docs/a.md:1", "docs/z.md:1"])

    def test_cli_passes_qualified_repository_fixture(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "docs").mkdir()
            (root / "docs" / "summary.md").write_text(
                "Evidence-only: provider cache has a 100% miss rate.\n",
                encoding="utf-8",
            )

            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(root)],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("instruction claim invariant passed", result.stdout)


if __name__ == "__main__":
    unittest.main()
