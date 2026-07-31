from __future__ import annotations

import unittest

from scripts.ci.validate_autofix_run import validate_run


def trusted_run() -> dict[str, object]:
    repository = {"full_name": "packet28/packet28"}
    return {
        "status": "completed",
        "conclusion": "failure",
        "name": "Build",
        "head_branch": "main",
        "head_sha": "a" * 40,
        "html_url": "https://github.com/packet28/packet28/actions/runs/7",
        "repository": repository,
        "head_repository": repository.copy(),
    }


class AutofixRunTrustTests(unittest.TestCase):
    def resolve(self, payload: dict[str, object]) -> dict[str, str]:
        return validate_run(
            payload,
            repository="packet28/packet28",
            default_branch="main",
        )

    def test_accepts_failed_default_branch_run_from_same_repository(self) -> None:
        self.assertEqual(
            self.resolve(trusted_run()),
            {
                "target_ref": "a" * 40,
                "run_url": "https://github.com/packet28/packet28/actions/runs/7",
            },
        )

    def test_rejects_arbitrary_branch_or_fork_run(self) -> None:
        for field, value in (
            ("head_branch", "attacker"),
            ("head_repository", {"full_name": "attacker/fork"}),
            ("repository", {"full_name": "attacker/fork"}),
        ):
            with self.subTest(field=field):
                payload = trusted_run()
                payload[field] = value
                with self.assertRaises(ValueError):
                    self.resolve(payload)

    def test_rejects_nonfailed_or_unapproved_workflow(self) -> None:
        for field, value in (
            ("status", "in_progress"),
            ("conclusion", "success"),
            ("name", "Codex Autofix"),
        ):
            with self.subTest(field=field):
                payload = trusted_run()
                payload[field] = value
                with self.assertRaises(ValueError):
                    self.resolve(payload)

    def test_rejects_mutable_or_malformed_run_identity(self) -> None:
        for field, value in (
            ("head_sha", "main"),
            ("head_sha", "A" * 40),
            ("html_url", "safe\nforged-output=true"),
        ):
            with self.subTest(field=field):
                payload = trusted_run()
                payload[field] = value
                with self.assertRaises(ValueError):
                    self.resolve(payload)
