use guardy_core::{AuditFinding, AuditResult, AuditTotals};
use suite_packet_core::governance;

#[test]
fn legacy_governance_paths_are_the_shared_wire_types() {
    let shared = governance::AuditResult {
        passed: true,
        policy_version: 1,
        checked_at_unix: 7,
        totals: governance::AuditTotals::default(),
        findings: vec![governance::AuditFinding {
            rule: "allowed".to_string(),
            subject: "packet".to_string(),
            message: "accepted".to_string(),
        }],
    };
    let legacy: AuditResult = shared;
    let _: AuditTotals = legacy.totals;
    let finding: &AuditFinding = &legacy.findings[0];

    assert!(legacy.passed);
    assert_eq!(finding.rule, "allowed");
}
