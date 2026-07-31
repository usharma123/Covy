use context_kernel_mechanism::{ExecutionBudget, KernelRequest};
use serde_json::json;
use suite_packet_core::kernel;

#[test]
fn legacy_kernel_paths_are_the_shared_wire_types() {
    let shared = kernel::KernelRequest {
        target: "context.assemble".to_string(),
        budget: kernel::ExecutionBudget {
            token_cap: Some(64),
            byte_cap: None,
            runtime_ms_cap: Some(20),
        },
        ..kernel::KernelRequest::default()
    };
    let legacy: KernelRequest = shared;
    let legacy_budget: ExecutionBudget = legacy.budget;

    assert_eq!(
        serde_json::to_value((legacy.target, legacy_budget)).unwrap(),
        json!(["context.assemble", {
            "token_cap": 64,
            "byte_cap": null,
            "runtime_ms_cap": 20
        }])
    );
}
