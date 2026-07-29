pub mod mcp;
#[expect(
    dead_code,
    reason = "each integration binary exercises a focused subset of the shared harness"
)]
pub mod process_harness;
