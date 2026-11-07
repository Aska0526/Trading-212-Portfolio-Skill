//! Core contracts for T212 Portfolio.
//!
//! This crate holds the normalized domain model, typed errors, redaction, the
//! provider port and deterministic analytics. It has no I/O and no knowledge of
//! MCP, the CLI, credential stores or the Trading 212 wire format.

pub mod analytics;
pub mod domain;
pub mod error;
pub mod performance;
pub mod provider;
pub mod redact;
pub mod timeline;

/// Version of the normalized contract. Bump on any incompatible change to
/// `domain` and update the fixtures.
pub const CONTRACT_VERSION: &str = "0.3.0";
