use crate::daemon_task_mcp::{
    initialize_mcp_session, run_claude_hook, start_mcp_server, write_intention_via_mcp,
};
use serde_json::json;
use std::path::Path;

pub fn seed_checkpointed_handoff_task(dir: &Path, task_id: &str, intention_text: &str) {
    let (mut child, mut stdin, mut stdout) = start_mcp_server(dir);
    initialize_mcp_session(&mut stdin, &mut stdout);
    let _ = write_intention_via_mcp(
        &mut stdin,
        &mut stdout,
        2,
        task_id,
        intention_text,
        "investigating",
        &["src/alpha.rs"],
    );
    let _ = child.kill();
    let _ = child.wait();
    let (status, _) = run_claude_hook(
        dir,
        &json!({
            "hook_event_name":"Stop",
            "task_id":task_id,
            "session_id": format!("session-{task_id}"),
        }),
    );
    assert_eq!(status, 0);
}
