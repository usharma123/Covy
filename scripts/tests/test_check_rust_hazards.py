import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_rust_hazards.py"
SPEC = importlib.util.spec_from_file_location("check_rust_hazards", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
hazards = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(hazards)


class RustHazardPolicyTests(unittest.TestCase):
    def test_current_unsafe_sources_are_reviewed(self):
        self.assertEqual(hazards.unexpected_unsafe_files(ROOT), set())
        self.assertEqual(
            hazards.unsafe_source_files(ROOT),
            set(hazards.ALLOWED_UNSAFE_FILES),
        )

    def test_current_panic_expectations_are_narrow_and_reviewed(self):
        inventory, errors = hazards.panic_override_inventory(ROOT)

        self.assertEqual(errors, [])
        self.assertEqual(inventory, hazards.REVIEWED_PANIC_EXPECTATIONS)
        self.assertEqual(hazards.panic_override_errors(ROOT), [])

    def test_unreviewed_unsafe_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "core-algorithm" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "fn read(pointer: *const u8) -> u8 { unsafe { *pointer } }\n",
                encoding="utf-8",
            )

            self.assertEqual(
                hazards.unexpected_unsafe_files(root),
                {"crates/core-algorithm/src/lib.rs"},
            )

    def test_stale_allowlist_entry_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            self.assertEqual(
                hazards.stale_unsafe_allowlist_entries(root),
                set(hazards.ALLOWED_UNSAFE_FILES),
            )

    def test_allow_and_unreasoned_panic_overrides_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "core-algorithm" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "#![allow(clippy::unwrap_used)]\n"
                "#[expect(clippy::panic_in_result_fn)]\n"
                "pub fn value() { panic!(\"broken\") }\n",
                encoding="utf-8",
            )

            inventory, errors = hazards.panic_override_inventory(root)

            self.assertEqual(
                inventory,
                hazards.Counter(
                    {
                        (
                            "crates/core-algorithm/src/lib.rs",
                            "clippy::panic_in_result_fn",
                        ): 1
                    }
                ),
            )
            self.assertTrue(any("may not use #[allow]" in error for error in errors))
            self.assertTrue(any("must include a reason" in error for error in errors))

    def test_clippy_scopes_panics_to_production_targets(self):
        unsafe_command, panic_command = hazards.clippy_commands()

        self.assertIn("--all-targets", unsafe_command)
        self.assertNotIn("--all-targets", panic_command)
        self.assertIn("--lib", panic_command)
        self.assertIn("--bins", panic_command)
        self.assertIn("--locked", unsafe_command)
        self.assertIn("--locked", panic_command)
        for lint in hazards.PANIC_LINTS:
            self.assertIn(lint, panic_command)

    def test_clippy_failures_are_propagated(self):
        commands = hazards.clippy_commands()
        with mock.patch.object(hazards.subprocess, "run") as run:
            hazards.run_clippy(Path("/tmp/repo"), commands)

        self.assertEqual(run.call_count, 2)
        for call in run.call_args_list:
            self.assertTrue(call.kwargs["check"])
            self.assertEqual(call.kwargs["cwd"], Path("/tmp/repo"))


if __name__ == "__main__":
    unittest.main()
