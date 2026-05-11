# Windsurf Diagnostics

Packet28’s Windsurf support tier is MCP/rules verified and command rewrite guidance-only.

## Generated Files

- MCP config: `~/.codeium/windsurf/mcp_config.json`
- Rules: `.windsurf/rules/packet28.md`
- Repo hook config: `.windsurf/hooks.json`

## Verification Commands

```bash
Packet28 init --agent windsurf --yes --root .
Packet28 setup --runtime windsurf --yes --root .
Packet28 doctor --agent windsurf --root .
Packet28 mcp smoke-test --from-config windsurf
```

## Doctor Checks

`Packet28 doctor --agent windsurf --root .` validates:

- generated MCP config exists and contains `mcpServers.packet28`,
- command resolves,
- args preserve the requested workspace root,
- rules are honest about the support tier,
- daemon/index health is available,
- MCP initialize and `tools/list` succeed through the generated config.

## Non-Claim

Packet28 does not claim full RTK-style transparent command interception for Windsurf. That remains deferred until a real Windsurf runtime test proves that Windsurf invokes Packet28 around shell commands.
