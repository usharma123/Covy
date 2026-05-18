# Persistent FFF MCP Search Smoke

This artifact records the follow-up inspection requested for `dmtrKovalenko/fff` and the Packet28 MCP integration change that keeps the upstream `fff-mcp` process warm across searches.

## Source Snapshot

- Repository: `https://github.com/dmtrKovalenko/fff`
- Revision inspected: `6645a68ebc1f1a0dc98c1669a7e3f6d91625f79f`
- Local inspection path: `/private/tmp/packet28-fff-upstream-20260518`
- Build command: `cargo build -p fff-mcp --no-default-features --release`
- Built binary: `/private/tmp/packet28-fff-upstream-20260518/target/release/fff-mcp`

## Integration Decision

Upstream exposes a real Rust crate, `fff-search`, plus the `fff-mcp` binary. Direct linking is not safe for Packet28 yet because Packet28 declares `rust-version = "1.75"` and edition 2021, while current FFF crates use edition 2024. The integration therefore uses the upstream Rust implementation through a persistent `fff-mcp` subprocess in the long-lived Packet28 MCP server.

Packet28 now accepts `search_strategy: "fff"` on `packet28.search` and `packet28.search_fast`. The MCP session caches one `fff-mcp` process per root, preserving FFF's warm index and typed output path while retaining Packet28's slim/full payload reduction and artifact storage.

## Smoke Command

The smoke used `target/debug/Packet28 mcp serve --root /Users/utsavsharma/Documents/GitHub/Coverage` with `P28_FFF_MCP_BIN` pointed at the built upstream binary, then called `packet28.search_fast` four times in the same MCP session.

## Results

| query | Packet28 persistent FFF ms | backend | matches |
|---|---:|---|---:|
| `SearchResult` | 1172.2 | fff_mcp | 19 |
| `SearchResult` | 8.0 | fff_mcp | 19 |
| `SearchResult` | 1.0 | fff_mcp | 19 |
| `Packet28SearchStrategy` | 0.8 | fff_mcp | 10 |

Native `rg -n --max-count 20` comparison in the same repo:

| query | rg ms | status |
|---|---:|---:|
| `SearchResult` | 161.3 | 0 |
| `SearchResult` | 18.4 | 0 |
| `SearchResult` | 17.3 | 0 |
| `Packet28SearchStrategy` | 17.1 | 0 |

## Outcome

FFF is worthwhile for Packet28 when it is used as a warm, long-lived backend. The previous per-query `p28 --engine fff` adapter proved correctness but paid process startup every call. The persistent MCP strategy shows the intended FFF shape: after initial scan, repeated indexed searches are faster than repeated `rg` spawns while Packet28 still returns reduced MCP payloads.
