import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_test_harness.py"
SPEC = importlib.util.spec_from_file_location("check_test_harness", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
harness_policy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(harness_policy)


class TestHarnessPolicyTests(unittest.TestCase):
    def test_current_inventory_is_reviewed_and_complete(self):
        errors, inventory = harness_policy.audit_repository(ROOT)

        self.assertEqual(errors, [])
        self.assertEqual(
            inventory["manual_lifecycle"],
            set(harness_policy.MANUAL_LIFECYCLE_ALLOWLIST),
        )
        self.assertEqual(
            inventory["nested_cargo"],
            {harness_policy.HARNESS_PATH},
        )
        self.assertEqual(
            inventory["direct_git"],
            {harness_policy.HARNESS_PATH},
        )
        self.assertEqual(inventory["support_sync_child"], set())
        self.assertEqual(inventory["manual_cleanup"], set())
        self.assertEqual(
            inventory["mcp_framing"],
            set(harness_policy.MCP_FRAMING_ALLOWLIST),
        )
        self.assertEqual(
            inventory["raw_socket"],
            set(harness_policy.SOCKET_ALLOWLIST),
        )
        self.assertIn(
            "crates/suite-cli/tests/process_harness_e2e.rs",
            inventory["raw_process_client"],
        )

    def test_unreviewed_lifecycle_build_cleanup_and_protocol_code_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "suite-cli" / "tests" / "bad_e2e.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                """
fn bad() {
    std::process::Command::new(
        "cargo"
    ).spawn().unwrap().wait().unwrap();
    std::process::Command::new(
        "git"
    ).status().unwrap();
    std::fs::remove_dir_all("fixture").unwrap();
    let _ = "Content-Length: 2";
    let _ = std::net::TcpListener::bind("127.0.0.1:0");
}
""",
                encoding="utf-8",
            )

            errors, _ = harness_policy.audit_repository(root)

        rendered = "\n".join(errors)
        self.assertIn("manual child lifecycle", rendered)
        self.assertIn("nested Cargo build", rendered)
        self.assertIn("Git fixture process", rendered)
        self.assertIn("manual filesystem cleanup", rendered)
        self.assertIn("MCP client framing", rendered)
        self.assertIn("raw socket lifecycle", rendered)

    def test_support_helpers_cannot_add_synchronous_child_waits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = (
                root
                / "crates"
                / "suite-cli"
                / "tests"
                / "support"
                / "shallow_helper.rs"
            )
            source.parent.mkdir(parents=True)
            source.write_text(
                'fn run(mut command: Command) { command.output().unwrap(); }\n',
                encoding="utf-8",
            )

            errors, _ = harness_policy.audit_repository(root)

        self.assertTrue(
            any("synchronous support child" in error for error in errors),
            errors,
        )

    def test_raw_std_process_must_name_its_bounded_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "suite-cli" / "tests" / "raw_e2e.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                'fn command() { let _ = std::process::Command::new("tool"); }\n',
                encoding="utf-8",
            )

            errors, _ = harness_policy.audit_repository(root)

        self.assertTrue(
            any("lacks a bounded harness owner" in error for error in errors),
            errors,
        )

    def test_reviewed_socket_fixture_must_keep_elapsed_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / next(iter(harness_policy.SOCKET_ALLOWLIST))
            source.parent.mkdir(parents=True)
            source.write_text(
                'fn probe() { let _ = TcpListener::bind("127.0.0.1:0"); }\n',
                encoding="utf-8",
            )

            errors, _ = harness_policy.audit_repository(root)

        self.assertTrue(
            any("lost deadline marker" in error for error in errors),
            errors,
        )


if __name__ == "__main__":
    unittest.main()
