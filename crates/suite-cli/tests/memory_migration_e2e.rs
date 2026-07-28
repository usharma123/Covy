#[path = "support/memory_migration.rs"]
mod memory_migration;
#[path = "support/memory_migration_assertions.rs"]
mod memory_migration_assertions;

use assert_cmd::Command;
use memory_migration::write_legacy_memory_db;
use memory_migration_assertions::{
    assert_migrated_fts, assert_migrated_memory_defaults, assert_migrated_schema,
};
use predicates::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[test]
fn test_memory_migration_store_migrates_legacy_sqlite_schema() {
    let home = TempDir::new().unwrap();
    let db_path = write_legacy_memory_db(home.path());

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "stats", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"memory_count\":1"));

    let conn = Connection::open(&db_path).unwrap();
    assert_migrated_schema(&conn);
    assert_migrated_fts(&conn);
    assert_migrated_memory_defaults(&conn);

    suite_cmd()
        .env("HOME", home.path())
        .args(["memory", "recall", "legacy durable", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy Packet28 durable context"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["feedback", "search", "legacy correction", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy correction body"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["graph", "search", "LegacyConcept", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("LegacyConcept"));
    suite_cmd()
        .env("HOME", home.path())
        .args(["transcript", "search", "legacy transcript", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy transcript context"));
}
