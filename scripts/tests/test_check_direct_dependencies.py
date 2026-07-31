import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from check_direct_dependencies import audit


class DirectDependencyAuditTests(unittest.TestCase):
    def workspace(
        self,
        *,
        dependencies: str,
        source: str,
        build_source: str = "",
        workspace_dependencies: str = 'serde = "1"',
    ) -> Path:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        crate = root / "crates" / "app"
        (crate / "src").mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            textwrap.dedent(
                f"""\
                [workspace]
                members = ["crates/app"]

                [workspace.dependencies]
                {workspace_dependencies}
                """
            ),
            encoding="utf-8",
        )
        (crate / "Cargo.toml").write_text(
            textwrap.dedent(
                f"""\
                [package]
                name = "app"
                version = "0.1.0"
                edition = "2021"

                {dependencies}
                """
            ),
            encoding="utf-8",
        )
        (crate / "src" / "lib.rs").write_text(
            textwrap.dedent(source), encoding="utf-8"
        )
        if build_source:
            (crate / "build.rs").write_text(
                textwrap.dedent(build_source), encoding="utf-8"
            )
        return root

    def test_accepts_normal_dev_build_target_and_renamed_uses(self) -> None:
        root = self.workspace(
            dependencies="""\
                [dependencies]
                serde.workspace = true
                json = { package = "serde_json", version = "1" }

                [dev-dependencies]
                tempfile = "3"

                [build-dependencies]
                cc = "1"

                [target.'cfg(unix)'.dependencies]
                libc = "0.2"
            """,
            source="""\
                use serde::Serialize;
                pub fn value() -> json::Value { json::Value::Null }
                #[cfg(test)]
                fn fixture() { let _ = tempfile::tempdir(); }
                #[cfg(unix)]
                const STDOUT: i32 = libc::STDOUT_FILENO;
            """,
            build_source="fn main() { let _ = cc::Build::new(); }",
        )

        result = audit(root)

        self.assertEqual(result.errors, ())

    def test_rejects_unused_member_dependency_despite_docs_mention(self) -> None:
        root = self.workspace(
            dependencies="""\
                [dependencies]
                serde.workspace = true
                regex = "1"
            """,
            source="use serde::Serialize;",
        )
        (root / "crates" / "app" / "README.md").write_text(
            "regex::Regex is discussed here", encoding="utf-8"
        )

        result = audit(root)

        self.assertEqual(
            result.errors,
            (
                "crates/app/Cargo.toml: [dependencies] dependency `regex` "
                "has no use in normal Rust targets",
            ),
        )

    def test_rejects_dependency_named_only_in_rust_comments_and_strings(self) -> None:
        root = self.workspace(
            dependencies="""\
                [dependencies]
                serde.workspace = true
                regex = "1"
            """,
            source=r'''\
                use serde::Serialize;
                // regex::Regex is not a dependency use.
                /* nested /* regex::Regex */ comment */
                const NORMAL: &str = "regex::Regex";
                const RAW: &str = r#"regex::Regex"#;
            ''',
        )

        result = audit(root)

        self.assertEqual(
            result.errors,
            (
                "crates/app/Cargo.toml: [dependencies] dependency `regex` "
                "has no use in normal Rust targets",
            ),
        )

    def test_rejects_uninherited_workspace_dependency(self) -> None:
        root = self.workspace(
            dependencies="""\
                [dependencies]
                serde.workspace = true
            """,
            source="use serde::Serialize;",
            workspace_dependencies='serde = "1"\norphan = "1"',
        )

        result = audit(root)

        self.assertEqual(
            result.errors,
            (
                "Cargo.toml: [workspace.dependencies] dependency `orphan` "
                "is not inherited by any workspace member",
            ),
        )

    def test_discovers_standalone_package_outside_workspace_members(self) -> None:
        root = self.workspace(
            dependencies="""\
                [dependencies]
                serde.workspace = true
            """,
            source="use serde::Serialize;",
        )
        standalone = root / "fuzz"
        (standalone / "src").mkdir(parents=True)
        (standalone / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [workspace]

                [package]
                name = "standalone"
                version = "0.1.0"

                [dependencies]
                serde_json = "1"
                """
            ),
            encoding="utf-8",
        )
        (standalone / "src" / "lib.rs").write_text(
            "pub fn fuzz() {}", encoding="utf-8"
        )

        result = audit(root)

        self.assertEqual(
            result.errors,
            (
                "fuzz/Cargo.toml: [dependencies] dependency `serde_json` "
                "has no use in normal Rust targets",
            ),
        )


if __name__ == "__main__":
    unittest.main()
