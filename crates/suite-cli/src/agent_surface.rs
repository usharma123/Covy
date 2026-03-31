use std::path::{Path, PathBuf};

use clap::ValueEnum;
pub const LATEST_BOOTSTRAP_RELATIVE_PATH: &str = ".packet28/agent/latest-bootstrap.json";
pub const LATEST_HANDOFF_RELATIVE_PATH: &str = ".packet28/agent/latest-handoff.json";
const ROOT_PLACEHOLDER: &str = "<path>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentPromptFormat {
    Claude,
    Agents,
    Cursor,
    CursorRule,
    WindsurfRule,
}

pub fn latest_bootstrap_path(root: &Path) -> PathBuf {
    root.join(LATEST_BOOTSTRAP_RELATIVE_PATH)
}

pub fn latest_handoff_path(root: &Path) -> PathBuf {
    root.join(LATEST_HANDOFF_RELATIVE_PATH)
}

pub fn mcp_command_example(root: Option<&str>) -> String {
    format!("Packet28 mcp serve{}", command_root_fragment(root),)
}

pub fn mcp_proxy_command_example(root: Option<&str>) -> String {
    format!(
        "Packet28 mcp proxy{} --upstream-config .mcp.proxy.json",
        command_root_fragment(root),
    )
}

pub fn wrapper_command_example() -> &'static str {
    "packet28-agent --task-id <task-id> --wait-for-handoff -- <agent command...>"
}

pub fn render_prompt_fragment(format: AgentPromptFormat, root: Option<&str>) -> String {
    let mcp = mcp_command_example(root);
    let proxy = mcp_proxy_command_example(root);
    let wrapper = wrapper_command_example();
    let root_note = if root.is_some() {
        format!(
            "Use `--root {}` only when the agent is operating outside the repository root.",
            ROOT_PLACEHOLDER
        )
    } else {
        "Use `--root <path>` only when the agent is operating outside the repository root."
            .to_string()
    };

    match format {
        AgentPromptFormat::Claude => format!(
            "## Packet28\n\
Use Packet28 as a hooks-first reducer-plus-handoff runtime for non-trivial coding, debugging, test, review, refactor, or design tasks.\n\
\n\
- Start with `{mcp}` and install Claude hooks with `Packet28 setup`.\n\
- Let Claude hooks rewrite supported shell reads/searches and auto-capture routine tool activity; keep reducer traffic out of the visible MCP loop.\n\
- Prefer `packet28.search`, `packet28.read_regions`, and `packet28.glob` for compact in-turn exploration, then use `packet28.fetch_tool_result` only when you need the stored full artifact.\n\
- Use `packet28.write_intention` only when the task objective, current decision, or next step changes materially.\n\
- Use `packet28.task_status` only when you need to inspect handoff readiness or the latest artifact IDs for a task.\n\
- Use `packet28.prepare_handoff` and `packet28.fetch_context` only at checkpoint, resume, or explicit artifact-inspection boundaries.\n\
- If Packet28 is fronting upstream MCP tools via proxy, prefer `{proxy}` so upstream activity is captured into the next brief automatically.\n\
- For delegated relaunch flows, prefer `{wrapper}` or daemon-managed fresh-worker resume instead of stretching one worker session indefinitely.\n\
- Treat the latest Packet28 brief as the only canonical Packet28 context block; replace older Packet28 blocks instead of appending them.\n\
- Respect the supersession header in each brief and use it to ignore older Packet28 context.\n\
- Use `packet28://task/<task_id>/brief` or `.packet28/task/<task_id>/brief.md` only as a fallback bridge when MCP is unavailable.\n\
- If Packet28 is unavailable or returns insufficient context, fall back to direct file reads and commands.\n\
- Do not force handoff/bootstrap orchestration for trivial conversational requests or narrow single-file edits.\n\
- {root_note}\n"
        ),
        AgentPromptFormat::Agents => format!(
            "## Packet28 Guidance\n\
When the task is substantial, use Packet28 as a hooks-first reducer-plus-handoff runtime.\n\
\n\
- MCP command: `{mcp}`\n\
- Preferred MCP endpoint when available: `{proxy}`\n\
- Use runtime hooks installed by `Packet28 setup`, not visible reducer MCP calls, to rewrite supported shell commands and capture routine tool activity into Packet28.\n\
- Prefer `packet28.search`, `packet28.read_regions`, and `packet28.glob` for compact exploration; use `packet28.fetch_tool_result` only when you need the stored full artifact.\n\
- Use `packet28.write_intention` only when the task objective, current decision, or next step changes materially.\n\
- Use `packet28.task_status` only when you need to inspect handoff readiness or the latest artifact IDs.\n\
- Use `packet28.prepare_handoff` only at checkpoint or handoff boundaries, not as a normal exploration step.\n\
- Replace the prior Packet28 context block each turn instead of appending historical Packet28 briefs.\n\
- Keep thick context assembly out of the active worker loop; use `packet28.fetch_context` only for explicit handoff/bootstrap or artifact inspection.\n\
- Let the daemon or `{wrapper}` own fresh-worker relaunch from the prepared handoff packet.\n\
- Respect the supersession header in each brief and keep one mutable Packet28 block in the runtime prompt.\n\
- Use the task brief file/resource only as a compatibility fallback when MCP is unavailable.\n\
- Fall back to direct file reads if Packet28 is unavailable, errors, or does not provide enough context.\n\
- Skip handoff/bootstrap ceremony for trivial chat or isolated single-file edits.\n\
- {root_note}\n"
        ),
        AgentPromptFormat::Cursor => format!(
            "Packet28 integration:\n\
- Start `{mcp}` and use Packet28 as a control-plane plus handoff broker.\n\
- Prefer `{proxy}` when you want Packet28 to auto-capture upstream tool activity.\n\
- Prefer `packet28.search`, `packet28.read_regions`, and `packet28.glob` when compact native search/read output matters in-turn; use `packet28.fetch_tool_result` for stored full artifacts.\n\
- Use `packet28.write_intention` for semantic objective updates and keep rewrite/capture out of the visible MCP loop.\n\
- Use `packet28.task_status` only when you need handoff readiness or artifact IDs.\n\
- For checkpointed relaunch flows, use `packet28.prepare_handoff` to seed the next worker.\n\
- Keep one mutable Packet28 context block and replace it whenever a newer brief supersedes the old one.\n\
- Use `packet28.fetch_context` only when you explicitly need to inspect a stored handoff or context artifact.\n\
- Prefer `{wrapper}` or daemon-managed relaunch after checkpointed handoff assembly instead of keeping one session hot.\n\
- Respect the supersession header in each brief and use it to discard older Packet28 reasoning context.\n\
- Use `.packet28/task/<task_id>/brief.md` only as a fallback bridge when MCP is unavailable.\n\
- If Packet28 is unavailable, fails, or returns insufficient context, fall back to direct file reads and commands.\n\
- Do not force handoff/bootstrap orchestration for trivial chat or isolated single-file edits.\n\
- {root_note}\n"
        ),
        AgentPromptFormat::CursorRule => format!(
            "---\n\
description: Packet28 runtime guidance\n\
globs:\n\
alwaysApply: true\n\
---\n\
\n\
# Packet28 Integration\n\
\n\
- Start `{mcp}` and use Packet28 as a control-plane plus handoff broker.\n\
- Prefer `{proxy}` when you want Packet28 to auto-capture upstream tool activity.\n\
- Prefer `packet28.search`, `packet28.read_regions`, and `packet28.glob` when compact native search/read output matters in-turn; use `packet28.fetch_tool_result` for stored full artifacts.\n\
- Use `packet28.write_intention` for semantic objective updates and keep rewrite/capture out of the visible MCP loop.\n\
- Use `packet28.task_status` only when you need handoff readiness or artifact IDs.\n\
- For checkpointed relaunch flows, use `packet28.prepare_handoff` to seed the next worker.\n\
- Keep one mutable Packet28 context block and replace it whenever a newer brief supersedes the old one.\n\
- Use `packet28.fetch_context` only when you explicitly need to inspect a stored handoff or context artifact.\n\
- Prefer `{wrapper}` or daemon-managed relaunch after checkpointed handoff assembly instead of keeping one session hot.\n\
- Respect the supersession header in each brief and use it to discard older Packet28 reasoning context.\n\
- Use `.packet28/task/<task_id>/brief.md` only as a fallback bridge when MCP is unavailable.\n\
- If Packet28 is unavailable, fails, or returns insufficient context, fall back to direct file reads and commands.\n\
- Do not force handoff/bootstrap orchestration for trivial chat or isolated single-file edits.\n\
- {root_note}\n"
        ),
        AgentPromptFormat::WindsurfRule => format!(
            "---\n\
description: Packet28 runtime guidance\n\
trigger: always_on\n\
---\n\
\n\
# Packet28 Integration\n\
\n\
- Start `{mcp}` and use Packet28 as a control-plane plus handoff broker.\n\
- Prefer `{proxy}` when you want Packet28 to auto-capture upstream tool activity.\n\
- Prefer `packet28.search`, `packet28.read_regions`, and `packet28.glob` when compact native search/read output matters in-turn; use `packet28.fetch_tool_result` for stored full artifacts.\n\
- Use `packet28.write_intention` for semantic objective updates and keep rewrite/capture out of the visible MCP loop.\n\
- Use `packet28.task_status` only when you need handoff readiness or artifact IDs.\n\
- For checkpointed relaunch flows, use `packet28.prepare_handoff` to seed the next worker.\n\
- Keep one mutable Packet28 context block and replace it whenever a newer brief supersedes the old one.\n\
- Use `packet28.fetch_context` only when you explicitly need to inspect a stored handoff or context artifact.\n\
- Prefer `{wrapper}` or daemon-managed relaunch after checkpointed handoff assembly instead of keeping one session hot.\n\
- Respect the supersession header in each brief and use it to discard older Packet28 reasoning context.\n\
- Use `.packet28/task/<task_id>/brief.md` only as a fallback bridge when MCP is unavailable.\n\
- If Packet28 is unavailable, fails, or returns insufficient context, fall back to direct file reads and commands.\n\
- Do not force handoff/bootstrap orchestration for trivial chat or isolated single-file edits.\n\
- {root_note}\n"
        ),
    }
}

fn command_root_fragment(root: Option<&str>) -> String {
    match root {
        Some(root) if !root.trim().is_empty() && root.trim() != "." => {
            format!(" --root \"{}\"", root.trim())
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_example_uses_requested_root() {
        let rendered = mcp_command_example(Some("repo"));
        assert!(rendered.contains("--root \"repo\""));
    }

    #[test]
    fn claude_fragment_contains_required_guidance() {
        let rendered = render_prompt_fragment(AgentPromptFormat::Claude, None);
        assert!(rendered.contains("hooks-first reducer-plus-handoff runtime"));
        assert!(rendered.contains("packet28.search"));
        assert!(rendered.contains("packet28.read_regions"));
        assert!(rendered.contains("packet28.write_intention"));
        assert!(rendered.contains("packet28.prepare_handoff"));
        assert!(rendered.contains("packet28.task_status"));
        assert!(rendered.contains("packet28.fetch_tool_result"));
        assert!(rendered.contains("fall back to direct file reads and commands"));
        assert!(rendered.contains("brief.md"));
    }

    #[test]
    fn agents_fragment_tracks_current_workflow() {
        let rendered = render_prompt_fragment(AgentPromptFormat::Agents, None);
        assert!(rendered.contains("hooks-first reducer-plus-handoff runtime"));
        assert!(rendered.contains("packet28.fetch_tool_result"));
        assert!(rendered.contains("packet28.task_status"));
        assert!(rendered.contains("packet28.prepare_handoff"));
        assert!(rendered.contains("write_intention"));
        assert!(rendered.contains("packet28-agent --task-id <task-id>"));
        assert!(!rendered.contains("write_state"));
        assert!(!rendered.contains("get_context"));
    }

    #[test]
    fn cursor_fragment_mentions_non_trivial_scope() {
        let rendered = render_prompt_fragment(AgentPromptFormat::Cursor, None);
        assert!(rendered.contains("packet28.prepare_handoff"));
        assert!(rendered.contains("packet28.fetch_context"));
        assert!(rendered.contains("packet28.glob"));
        assert!(rendered.contains("packet28.fetch_tool_result"));
        assert!(rendered.contains("packet28.task_status"));
        assert!(rendered.contains("Packet28 mcp serve"));
        assert!(rendered.contains("single-file edits"));
    }

    #[test]
    fn cursor_rule_fragment_has_frontmatter() {
        let rendered = render_prompt_fragment(AgentPromptFormat::CursorRule, None);
        assert!(rendered.starts_with("---\n"));
        assert!(rendered.contains("alwaysApply: true"));
        assert!(rendered.contains("# Packet28 Integration"));
    }

    #[test]
    fn windsurf_rule_fragment_has_trigger_frontmatter() {
        let rendered = render_prompt_fragment(AgentPromptFormat::WindsurfRule, None);
        assert!(rendered.starts_with("---\n"));
        assert!(rendered.contains("trigger: always_on"));
        assert!(rendered.contains("# Packet28 Integration"));
    }
}
