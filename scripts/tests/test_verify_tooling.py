from __future__ import annotations

import unittest

from scripts import verify_tooling


class ToolingPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.toolchain = (verify_tooling.ROOT / "rust-toolchain.toml").read_text(
            encoding="utf-8"
        )
        cls.justfile = (verify_tooling.ROOT / "Justfile").read_text(
            encoding="utf-8"
        )

    def test_repository_tooling_matches_canonical_surface(self) -> None:
        self.assertEqual(
            verify_tooling.tooling_errors(self.toolchain, self.justfile),
            [],
        )

    def test_rejects_moving_toolchain_or_extra_components(self) -> None:
        moving = self.toolchain.replace(
            'channel = "1.93.1"',
            'channel = "stable"',
        ).replace(
            'components = ["clippy", "rustfmt"]',
            'components = ["clippy", "rustfmt", "rust-src"]',
        )

        errors = verify_tooling.tooling_errors(moving, self.justfile)

        self.assertIn("toolchain channel must be exactly 1.93.1", errors)
        self.assertIn(
            "toolchain components must be exactly clippy and rustfmt",
            errors,
        )

    def test_rejects_recipe_drift_from_canonical_gate(self) -> None:
        drifted = self.justfile.replace(
            "    scripts/validate_full_gate.sh\n",
            "    cargo test --workspace\n",
            1,
        )

        self.assertIn(
            "Justfile recipe 'ci' must delegate exactly to "
            "('scripts/validate_full_gate.sh',), found "
            "('cargo test --workspace',)",
            verify_tooling.tooling_errors(self.toolchain, drifted),
        )

    def test_rejects_unreviewed_or_bypass_recipes(self) -> None:
        unsafe = self.justfile + "\npublish:\n    git push --force origin main\n"

        errors = verify_tooling.tooling_errors(self.toolchain, unsafe)

        self.assertIn("Justfile has unreviewed recipes: publish", errors)
        self.assertIn("Justfile contains forbidden bypass: git push --force", errors)


if __name__ == "__main__":
    unittest.main()
