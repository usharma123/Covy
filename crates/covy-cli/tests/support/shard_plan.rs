use serde_json::json;
use std::path::{Path, PathBuf};

pub fn write_tests_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let tests_file = dir.join(name);
    std::fs::write(&tests_file, content).unwrap();
    tests_file
}

pub fn write_basic_tasks_file(dir: &Path) -> PathBuf {
    let tasks_file = dir.join("tasks.json");
    let payload = json!({
        "schema_version": 1,
        "tasks": [
            {"id": "com.foo.BarTest", "selector": "com.foo.BarTest", "est_ms": 1000},
            {"id": "tests/test_mod.py::test_one", "selector": "tests/test_mod.py::test_one", "est_ms": 800}
        ]
    });
    std::fs::write(&tasks_file, serde_json::to_string(&payload).unwrap()).unwrap();
    tasks_file
}

pub fn write_tier_tasks_file(dir: &Path) -> PathBuf {
    let tasks_file = dir.join("tasks.json");
    let payload = json!({
        "schema_version": 1,
        "tasks": [
            {"id": "fast-test", "selector": "fast-test", "est_ms": 1000, "tags": ["unit"]},
            {"id": "slow-test", "selector": "slow-test", "est_ms": 2000, "tags": ["slow"]}
        ]
    });
    std::fs::write(&tasks_file, serde_json::to_string(&payload).unwrap()).unwrap();
    tasks_file
}
