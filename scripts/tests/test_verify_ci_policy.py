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


class ReleasePermissionPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = (
            verify_ci_policy.WORKFLOW_DIR / "release.yml"
        ).read_text(encoding="utf-8")

    def test_repository_release_permissions_are_least_privilege(self) -> None:
        self.assertEqual(
            verify_ci_policy.release_permission_errors(self.workflow),
            [],
        )

    def test_rejects_workflow_wide_publication_permissions(self) -> None:
        unsafe = self.workflow.replace(
            "permissions:\n  contents: read",
            "permissions:\n  contents: write\n  id-token: write",
            1,
        )

        errors = verify_ci_policy.release_permission_errors(unsafe)

        self.assertIn(
            "workflow-wide release permissions may not grant writes",
            errors,
        )
        self.assertIn(
            "contents write permission must occur only in the publish job",
            errors,
        )
        self.assertIn(
            "OIDC write permission must occur only in the publish job",
            errors,
        )

    def test_rejects_publication_permissions_on_build_job(self) -> None:
        unsafe = self.workflow.replace(
            "  build:\n    needs: release-gates\n    permissions:\n"
            "      contents: read",
            "  build:\n    needs: release-gates\n    permissions:\n"
            "      contents: write\n      id-token: write",
            1,
        )

        self.assertIn(
            "build job may not receive publication permissions",
            verify_ci_policy.release_permission_errors(unsafe),
        )

    def test_rejects_publish_job_without_oidc(self) -> None:
        unsafe = self.workflow.replace("      id-token: write\n", "", 1)

        errors = verify_ci_policy.release_permission_errors(unsafe)

        self.assertIn(
            "publish job lacks scoped id-token: write permission",
            errors,
        )
        self.assertIn(
            "OIDC write permission must occur only in the publish job",
            errors,
        )


class DependabotPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.config = (
            verify_ci_policy.ROOT / ".github" / "dependabot.yml"
        ).read_text(encoding="utf-8")

    def test_repository_dependency_lane_is_reviewed_and_bounded(self) -> None:
        self.assertEqual(
            verify_ci_policy.dependabot_policy_errors(self.config),
            [],
        )

    def test_rejects_missing_ecosystem_or_unbounded_prs(self) -> None:
        unsafe = self.config.replace(
            '- package-ecosystem: "github-actions"',
            '- package-ecosystem: "docker"',
            1,
        ).replace("    open-pull-requests-limit: 5\n", "", 1)

        errors = verify_ci_policy.dependabot_policy_errors(unsafe)

        self.assertIn(
            'Dependabot must configure "github-actions" exactly once',
            errors,
        )
        self.assertIn(
            "every dependency ecosystem must bound open update PRs at five",
            errors,
        )

    def test_rejects_automatic_merge(self) -> None:
        unsafe = self.config + "\nauto-merge: true\n"

        self.assertIn(
            "dependency updates must not bypass review through auto-merge",
            verify_ci_policy.dependabot_policy_errors(unsafe),
        )


class ReleasePackageSmokePolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.build = (verify_ci_policy.WORKFLOW_DIR / "build.yml").read_text(
            encoding="utf-8"
        )
        cls.release = (verify_ci_policy.WORKFLOW_DIR / "release.yml").read_text(
            encoding="utf-8"
        )
        cls.full_gate = (
            verify_ci_policy.ROOT / "scripts" / "validate_full_gate.sh"
        ).read_text(encoding="utf-8")
        cls.package_verifier = (
            verify_ci_policy.ROOT / "scripts" / "verify_release_packages.py"
        ).read_text(encoding="utf-8")

    def errors(
        self,
        *,
        build: str | None = None,
        release: str | None = None,
        full_gate: str | None = None,
        package_verifier: str | None = None,
    ) -> list[str]:
        return verify_ci_policy.release_package_smoke_errors(
            build if build is not None else self.build,
            release if release is not None else self.release,
            full_gate if full_gate is not None else self.full_gate,
            (
                package_verifier
                if package_verifier is not None
                else self.package_verifier
            ),
        )

    def test_repository_release_package_smoke_is_complete(self) -> None:
        self.assertEqual(self.errors(), [])

    def test_rejects_missing_pre_upload_binary_smoke(self) -> None:
        unsafe = self.release.replace(
            "python3 scripts/verify_release_packages.py platform",
            "python3 scripts/removed_release_package_verifier.py platform",
            1,
        )

        self.assertIn(
            "staged platform verifier is not invoked in the build job",
            self.errors(release=unsafe),
        )

    def test_rejects_linux_arm64_without_emulated_execution(self) -> None:
        unsafe = self.release.replace(
            "smoke_mode: qemu-aarch64",
            "smoke_mode: native-or-metadata",
            1,
        )

        errors = self.errors(release=unsafe)

        self.assertTrue(
            any("smoke_mode: qemu-aarch64" in error for error in errors)
        )

    def test_rejects_silent_macos_cross_architecture_skip(self) -> None:
        unsafe = self.release.replace(
            "x86_64 execution requires an Intel runner or Rosetta and remains an "
            "external release check.",
            "cross build",
            1,
        )

        self.assertIn(
            "macOS x86_64 execution limitation is not explicit",
            self.errors(release=unsafe),
        )

    def test_rejects_artifact_transfer_that_loses_executable_modes(self) -> None:
        unsafe = self.release.replace(
            "path: dist/pkg-${{ matrix.platform }}.tar.gz",
            "path: dist/@packet28/${{ matrix.platform }}",
            1,
        )

        self.assertIn(
            "platform artifact does not preserve executable metadata",
            self.errors(release=unsafe),
        )

    def test_rejects_missing_pre_tag_package_dry_run(self) -> None:
        unsafe = self.full_gate.replace(
            "run_cmd python3 scripts/verify_release_packages.py source\n",
            "",
            1,
        )

        self.assertIn(
            "canonical gate lacks the pre-tag npm package dry-run",
            self.errors(full_gate=unsafe),
        )

    def test_rejects_online_npm_publish_dry_run(self) -> None:
        unsafe = self.package_verifier.replace('"--offline",', '"--online",', 1)

        self.assertIn(
            "package verifier does not force npm offline",
            self.errors(package_verifier=unsafe),
        )


if __name__ == "__main__":
    unittest.main()
