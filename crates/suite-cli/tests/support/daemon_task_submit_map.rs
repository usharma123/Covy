use serde_json::{json, Value};
use std::path::Path;

pub fn map_repo_step(root: &Path, task_id: &str, id: Option<&str>, depends_on: &[&str]) -> Value {
    let mut step = json!({
        "target": "mapy.repo",
        "depends_on": depends_on,
        "input_packets": [],
        "policy_context": {
            "task_id": task_id
        },
        "reducer_input": {
            "repo_root": root,
            "focus_paths": [],
            "focus_symbols": [],
            "max_files": 10,
            "max_symbols": 20,
            "include_tests": false
        },
        "budget": {}
    });
    if let Some(id) = id {
        step["id"] = json!(id);
    }
    step
}
