//! Reusable composition seams owned by the Packet28 daemon.
//!
//! The executable remains the lifecycle owner. Feature-gated modules here
//! expose only the small, independently testable orchestration boundaries that
//! need production parity or benchmark coverage.

#[cfg(feature = "shared-repository-scan")]
pub mod shared_repository_scan;
