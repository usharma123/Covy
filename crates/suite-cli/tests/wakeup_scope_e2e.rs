use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_wakeup_scope_scopes_context_by_path_symbol_and_intent() {
    let home = TempDir::new().unwrap();
    for (content, keywords) in [
        (
            "AuthFlow refactor notes live in src/auth.rs and should guide scoped wakeup",
            "AuthFlow,refactor,src/auth.rs",
        ),
        (
            "BillingFlow refactor notes live in src/billing.rs and should stay out of auth wakeup",
            "BillingFlow,refactor,src/billing.rs",
        ),
    ] {
        suite_cmd()
            .env("HOME", home.path())
            .args([
                "memory",
                "store",
                content,
                "--topic",
                "scoped-wakeup",
                "--importance",
                "high",
                "--keywords",
                keywords,
                "--project",
                "coverage-a",
                "--json",
            ])
            .assert()
            .success();
    }

    suite_cmd()
        .env("HOME", home.path())
        .args([
            "wakeup",
            "--query",
            "refactor",
            "--project",
            "coverage-a",
            "--path",
            "src/auth.rs",
            "--symbol",
            "AuthFlow",
            "--intent",
            "refactor",
            "--limit",
            "10",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"paths\":[\"src/auth.rs\"]"))
        .stdout(predicate::str::contains("\"symbols\":[\"AuthFlow\"]"))
        .stdout(predicate::str::contains("\"intent\":\"refactor\""))
        .stdout(predicate::str::contains("AuthFlow refactor notes"))
        .stdout(predicate::str::contains("BillingFlow refactor notes").not());
}
