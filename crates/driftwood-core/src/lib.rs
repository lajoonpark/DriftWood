//! DriftWood core — the product.
//!
//! ALL scan/reasoning/report logic lives here. The Tauri app and the MCP
//! server are thin wrappers that call only [`engine::run_scan`] and
//! [`engine::apply_correction`] and consume serialized [`report::Report`]
//! models. This crate must never depend on UI, Tauri, or MCP crates.

pub mod config;
pub mod engine;
pub mod events;
pub mod paths;
pub mod reason;
pub mod report;
pub mod rules;
pub mod scan;
pub mod score;
pub mod types;

pub use config::{default_model, DriftTuning, ScanConfig};
pub use engine::{apply_correction, load_rules, redistill, run_scan, CorrectionRequest, ScanHandle};
pub use events::{EventSink, Phase, ScanEvent};
pub use report::{GroupSummary, Report, ReportEntry, SCHEMA_VERSION};
pub use rules::Rule;
pub use types::{Band, Candidate, Kind, OrphanStatus, PrivacyTier, ScopeCategory, Tier};
pub use types::TierSource;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DriftError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("scan error: {0}")]
    Scan(String),
    #[error("reason error: {0}")]
    Reason(String),
    #[error("another DriftWood scan is already running (lock: {0})")]
    SingleFlight(String),
    #[error("scan cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, DriftError>;
