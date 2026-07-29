from __future__ import annotations

import unittest

from scripts import verify_ci_policy


class AutofixSecurityPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = (
            verify_ci_policy.WORKFLOW_DIR / "codex-autofix.yml"
        ).read_text(encoding="utf-8")

    def test_repository_workflow_satisfies_security_boundary(self) -> None:
        self.assertEqual(
            verify_ci_policy.autofix_security_errors(self.workflow),
            [],
        )

    def test_rejects_automatic_untrusted_branch_execution(self) -> None:
        unsafe = self.workflow.replace(
            "        github.event.workflow_run.head_branch == "
            "github.event.repository.default_branch &&\n",
            "",
        )

        errors = verify_ci_policy.autofix_security_errors(unsafe)

        self.assertIn(
            "automatic runs are not restricted to the trusted default branch",
            errors,
        )

    def test_rejects_candidate_supplied_driver(self) -> None:
        unsafe = self.workflow.replace(
            "TRUSTED_AUTOFIX: "
            "${{ github.workspace }}/trusted-control/scripts/ci/codex_autofix.sh",
            "TRUSTED_AUTOFIX_REMOVED: true",
        )
        unsafe += "\n      - name: Unsafe candidate driver\n"
        unsafe += "        run: scripts/ci/codex_autofix.sh\n"

        errors = verify_ci_policy.autofix_security_errors(unsafe)

        self.assertIn("trusted autofix driver is not selected explicitly", errors)
        self.assertIn(
            "candidate checkout may not supply the executed autofix driver",
            errors,
        )

    def test_rejects_secrets_or_write_credentials_in_execution_job(self) -> None:
        unsafe = self.workflow.replace(
            "      CODEX_AUTOFIX_VERIFY: cargo test --locked --workspace --all-targets\n",
            "      CODEX_AUTOFIX_VERIFY: cargo test --locked --workspace --all-targets\n"
            "      OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}\n",
        ).replace(
            "      contents: read\n    runs-on: ubuntu-latest",
            "      contents: write\n    runs-on: ubuntu-latest",
            1,
        )

        errors = verify_ci_policy.autofix_security_errors(unsafe)

        self.assertIn(
            "OpenAI credentials must be scoped to exactly one execution step",
            errors,
        )
        self.assertIn(
            "autofix execution job may not have write permissions",
            errors,
        )

    def test_rejects_force_push(self) -> None:
        unsafe = self.workflow.replace(
            '          git push -u origin "$branch"\n',
            '          git push --force-with-lease origin "$branch"\n',
        )

        self.assertIn(
            "autofix publication may not force-push",
            verify_ci_policy.autofix_security_errors(unsafe),
        )


if __name__ == "__main__":
    unittest.main()
