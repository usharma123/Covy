# p28 Search Backend Comparison

This artifact compares native `rg`, default Packet28 indexed `p28`, and the opt-in `p28 --engine fff` MCP adapter. It is search-backend evidence only, not a full parity maturity claim.

- Run id: FFF_COMPARE_20260511
- Repeats requested: 3
- fff-mcp binary: `/tmp/fff-eval/target/release/fff-mcp`
- Total rows: 81
- Failed rows excluding rg no-match status 1: 0
- Fallback rows: 12
- Average native rg duration ms: 34.3
- Average p28 indexed duration ms: 2615.4
- Average p28 fff duration ms: 143.6

## Backend Counts

| backend | rows |
|---|---:|
| (native) | 27 |
| fff_mcp | 27 |
| indexed_regex | 15 |
| legacy_rg | 12 |

## Coverage

| repo | query | native rg | p28 indexed | p28 fff |
|---|---|---:|---:|---:|
| fd | `Result` | 3 | 3 | 3 |
| fd | `TODO` | 3 | 3 | 3 |
| fd | `fn` | 3 | 3 | 3 |
| packet28 | `Result` | 3 | 3 | 3 |
| packet28 | `TODO` | 3 | 3 | 3 |
| packet28 | `fn` | 3 | 3 | 3 |
| ripgrep | `Result` | 3 | 3 | 3 |
| ripgrep | `TODO` | 3 | 3 | 3 |
| ripgrep | `fn` | 3 | 3 | 3 |

## Rows

| kind | repo | query | status | duration ms | hit lines | tokens | backend | fallback |
|---|---|---|---:|---:|---:|---:|---|---|
| native_rg | packet28 | `fn` | 0 | 81 | 3321 | 83616 |  |  |
| p28_indexed | packet28 | `fn` | 0 | 41196 | 2561 | 27300 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | packet28 | `fn` | 0 | 154 | 20 | 191 | fff_mcp |  |
| native_rg | packet28 | `Result` | 0 | 34 | 1292 | 39087 |  |  |
| p28_indexed | packet28 | `Result` | 0 | 54 | 1137 | 11969 | legacy_rg | candidate set remained too broad for indexed verification (276/491 files) |
| p28_fff | packet28 | `Result` | 0 | 143 | 26 | 342 | fff_mcp |  |
| native_rg | packet28 | `TODO` | 0 | 32 | 34 | 2321 |  |  |
| p28_indexed | packet28 | `TODO` | 0 | 28 | 19 | 244 | indexed_regex |  |
| p28_fff | packet28 | `TODO` | 0 | 142 | 17 | 218 | fff_mcp |  |
| native_rg | ripgrep | `fn` | 0 | 34 | 1636 | 32360 |  |  |
| p28_indexed | ripgrep | `fn` | 0 | 23971 | 1024 | 8123 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | ripgrep | `fn` | 0 | 140 | 50 | 311 | fff_mcp |  |
| native_rg | ripgrep | `Result` | 0 | 28 | 539 | 11776 |  |  |
| p28_indexed | ripgrep | `Result` | 0 | 50 | 355 | 2785 | indexed_regex |  |
| p28_fff | ripgrep | `Result` | 0 | 146 | 19 | 135 | fff_mcp |  |
| native_rg | ripgrep | `TODO` | 0 | 29 | 6 | 164 |  |  |
| p28_indexed | ripgrep | `TODO` | 0 | 28 | 6 | 45 | indexed_regex |  |
| p28_fff | ripgrep | `TODO` | 0 | 145 | 6 | 45 | fff_mcp |  |
| native_rg | fd | `fn` | 0 | 29 | 274 | 5527 |  |  |
| p28_indexed | fd | `fn` | 0 | 4520 | 226 | 1113 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | fd | `fn` | 0 | 145 | 34 | 159 | fff_mcp |  |
| native_rg | fd | `Result` | 0 | 26 | 94 | 1803 |  |  |
| p28_indexed | fd | `Result` | 0 | 28 | 76 | 349 | indexed_regex |  |
| p28_fff | fd | `Result` | 0 | 142 | 17 | 70 | fff_mcp |  |
| native_rg | fd | `TODO` | 0 | 25 | 13 | 1659 |  |  |
| p28_indexed | fd | `TODO` | 0 | 24 | 13 | 59 | indexed_regex |  |
| p28_fff | fd | `TODO` | 0 | 145 | 13 | 59 | fff_mcp |  |
| native_rg | packet28 | `fn` | 0 | 60 | 3321 | 83616 |  |  |
| p28_indexed | packet28 | `fn` | 0 | 70 | 2561 | 27300 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | packet28 | `fn` | 0 | 142 | 20 | 191 | fff_mcp |  |
| native_rg | packet28 | `Result` | 0 | 37 | 1292 | 39087 |  |  |
| p28_indexed | packet28 | `Result` | 0 | 55 | 1137 | 11969 | legacy_rg | candidate set remained too broad for indexed verification (276/491 files) |
| p28_fff | packet28 | `Result` | 0 | 143 | 26 | 342 | fff_mcp |  |
| native_rg | packet28 | `TODO` | 0 | 32 | 34 | 2321 |  |  |
| p28_indexed | packet28 | `TODO` | 0 | 26 | 19 | 244 | indexed_regex |  |
| p28_fff | packet28 | `TODO` | 0 | 144 | 17 | 218 | fff_mcp |  |
| native_rg | ripgrep | `fn` | 0 | 34 | 1636 | 32360 |  |  |
| p28_indexed | ripgrep | `fn` | 0 | 43 | 1024 | 8123 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | ripgrep | `fn` | 0 | 139 | 50 | 311 | fff_mcp |  |
| native_rg | ripgrep | `Result` | 0 | 32 | 539 | 11776 |  |  |
| p28_indexed | ripgrep | `Result` | 0 | 53 | 355 | 2785 | indexed_regex |  |
| p28_fff | ripgrep | `Result` | 0 | 152 | 19 | 135 | fff_mcp |  |
| native_rg | ripgrep | `TODO` | 0 | 40 | 6 | 164 |  |  |
| p28_indexed | ripgrep | `TODO` | 0 | 27 | 6 | 45 | indexed_regex |  |
| p28_fff | ripgrep | `TODO` | 0 | 143 | 6 | 45 | fff_mcp |  |
| native_rg | fd | `fn` | 0 | 26 | 274 | 5527 |  |  |
| p28_indexed | fd | `fn` | 0 | 32 | 226 | 1113 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | fd | `fn` | 0 | 146 | 34 | 159 | fff_mcp |  |
| native_rg | fd | `Result` | 0 | 24 | 94 | 1803 |  |  |
| p28_indexed | fd | `Result` | 0 | 28 | 76 | 349 | indexed_regex |  |
| p28_fff | fd | `Result` | 0 | 141 | 17 | 70 | fff_mcp |  |
| native_rg | fd | `TODO` | 0 | 28 | 13 | 1659 |  |  |
| p28_indexed | fd | `TODO` | 0 | 25 | 13 | 59 | indexed_regex |  |
| p28_fff | fd | `TODO` | 0 | 144 | 13 | 59 | fff_mcp |  |
| native_rg | packet28 | `fn` | 0 | 61 | 3321 | 83616 |  |  |
| p28_indexed | packet28 | `fn` | 0 | 66 | 2561 | 27300 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | packet28 | `fn` | 0 | 139 | 20 | 191 | fff_mcp |  |
| native_rg | packet28 | `Result` | 0 | 33 | 1292 | 39087 |  |  |
| p28_indexed | packet28 | `Result` | 0 | 55 | 1137 | 11969 | legacy_rg | candidate set remained too broad for indexed verification (276/491 files) |
| p28_fff | packet28 | `Result` | 0 | 143 | 26 | 342 | fff_mcp |  |
| native_rg | packet28 | `TODO` | 0 | 31 | 34 | 2321 |  |  |
| p28_indexed | packet28 | `TODO` | 0 | 28 | 19 | 244 | indexed_regex |  |
| p28_fff | packet28 | `TODO` | 0 | 139 | 17 | 218 | fff_mcp |  |
| native_rg | ripgrep | `fn` | 0 | 31 | 1636 | 32360 |  |  |
| p28_indexed | ripgrep | `fn` | 0 | 46 | 1024 | 8123 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | ripgrep | `fn` | 0 | 142 | 50 | 311 | fff_mcp |  |
| native_rg | ripgrep | `Result` | 0 | 31 | 539 | 11776 |  |  |
| p28_indexed | ripgrep | `Result` | 0 | 48 | 355 | 2785 | indexed_regex |  |
| p28_fff | ripgrep | `Result` | 0 | 140 | 19 | 135 | fff_mcp |  |
| native_rg | ripgrep | `TODO` | 0 | 29 | 6 | 164 |  |  |
| p28_indexed | ripgrep | `TODO` | 0 | 27 | 6 | 45 | indexed_regex |  |
| p28_fff | ripgrep | `TODO` | 0 | 145 | 6 | 45 | fff_mcp |  |
| native_rg | fd | `fn` | 0 | 27 | 274 | 5527 |  |  |
| p28_indexed | fd | `fn` | 0 | 31 | 226 | 1113 | legacy_rg | planner derived only weak/common literals; routing broad regex to legacy_rg |
| p28_fff | fd | `fn` | 0 | 146 | 34 | 159 | fff_mcp |  |
| native_rg | fd | `Result` | 0 | 28 | 94 | 1803 |  |  |
| p28_indexed | fd | `Result` | 0 | 29 | 76 | 349 | indexed_regex |  |
| p28_fff | fd | `Result` | 0 | 142 | 17 | 70 | fff_mcp |  |
| native_rg | fd | `TODO` | 0 | 25 | 13 | 1659 |  |  |
| p28_indexed | fd | `TODO` | 0 | 27 | 13 | 59 | indexed_regex |  |
| p28_fff | fd | `TODO` | 0 | 145 | 13 | 59 | fff_mcp |  |
