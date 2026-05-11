# Windsurf Diagnostics

## Support Statement

Packet28 Windsurf support is currently **MCP and rules first**. Command rewrite through Windsurf hooks is **not guaranteed** until a test proves Windsurf invokes Packet28 around shell command execution.

Do not describe Windsurf as full hook/rewrite parity. The honest tier is:

- MCP config: supported and must be smoke-tested.
- Rules file: supported and must be generated.
- Command rewrite: unproven/guidance-only unless an interception test is added later.

## Expected Config

Windsurf setup writes global config at:

```text
~/.codeium/windsurf/mcp_config.json
```

Expected shape:

```json
{
  "mcpServers": {
    "packet28": {
      "command": "packet28-mcp",
      "args": ["--root", "/absolute/repo/path"]
    }
  }
}
```

The command may be an absolute executable path or a command resolvable on `PATH`. Existing sibling MCP servers must be preserved.

## Current Gaps

1. `Packet28 doctor` does not accept `--agent windsurf`.
2. `Packet28 doctor` does not inspect `~/.codeium/windsurf/mcp_config.json`.
3. Doctor currently starts the current Packet28 binary directly for MCP checks instead of spawning the command from the generated Windsurf config.
4. `Packet28 mcp` has no `smoke-test --from-config windsurf` command.
5. Existing Windsurf E2E setup coverage checks file existence, not config shape, path preservation, command resolution, or MCP initialize/tools-list.
6. Generated Windsurf rules must explicitly avoid claiming guaranteed command rewrite.

## Required Doctor Checks

`Packet28 doctor --agent windsurf --root <path>` must verify:

| Check | Expected result |
|---|---|
| Config exists | `~/.codeium/windsurf/mcp_config.json` is present. |
| JSON parses | Invalid JSON is reported without overwriting or repairing implicitly. |
| Other servers preserved | Existing `mcpServers` entries survive setup. |
| Packet28 entry exists | `mcpServers.packet28` exists. |
| Command resolves | `command` is absolute executable or found on `PATH`. |
| Args preserve repo root | `args` includes `--root` followed by the exact repo path, including spaces. |
| MCP initialize | Stdio initialize succeeds from the configured command and args. |
| MCP tools/list | Stdio tools/list succeeds and includes Packet28 tools. |
| Daemon | Daemon can start or reports an actionable failure. |
| Index | Repo index is healthy or reports an actionable rebuild command. |
| Rules | `.windsurf/rules/packet28.md` exists and states support honestly. |

## Smoke Test Command

`Packet28 mcp smoke-test --from-config windsurf` should:

1. Load the Windsurf config.
2. Resolve `mcpServers.packet28.command`.
3. Spawn the configured command with configured args.
4. Send MCP `initialize`.
5. Send MCP `tools/list`.
6. Exit non-zero with a specific diagnostic if any step fails.

## Test Plan

Add tests for:

- Fresh home directory.
- Existing valid Windsurf config with another MCP server.
- Invalid Windsurf JSON refusal.
- Repo path with spaces.
- Generated MCP command handshake.
- Doctor success.
- Doctor failure with missing binary.
- Idempotent setup.
- No regression to Claude/Cursor/Codex setup tests.

## Files To Touch First

- `crates/suite-cli/src/cmd_mcp.rs`
- `crates/suite-cli/src/cmd_doctor.rs`
- `crates/suite-cli/src/cmd_setup.rs`
- `crates/suite-cli/tests/setup_e2e.rs`
- `crates/suite-cli/src/agent_surface.rs`
