//! Correlation between canonical MNCS execution records and live host observations.
//!
//! This crate is an observer, never an authority. Execution records are ingested from the
//! subsystems that own them (Forge receipts and observations, `mncs.test-result/1` envelopes,
//! or explicitly authored files); host processes come from the monitor collector through
//! [`mncs_monitor_core::SystemSnapshot`]. Every link, anomaly, and restart classification in
//! here is a projection over those two inputs, and every uncertainty is explicit:
//!
//! - a host PID is a transient observation, matched only together with a start marker;
//! - no command-line substring, path, name-string, or timestamp heuristic is used for linkage;
//! - a verification outcome of [`VerificationOutcome::Unknown`] is never converted into a
//!   semantic failure by correlation evidence;
//! - an absent canonical source yields [`Linkage::LinkageUnknown`], never an invented identity.

mod correlate;
mod projection;
mod record;

pub use correlate::{
    correlate, reconcile_after_restart, Anomaly, AnomalyKind, CorrelationReport, HostSaturation,
    LinkConfidence, Linkage, RestartClass, RestartItem, RestartReport,
};
pub use projection::{
    correlation_report_value, execution_records_value, restart_report_value, CORRELATION_SCHEMA,
    EXECUTION_RECORDS_SCHEMA,
};
pub use record::{
    execution_records_from_json, execution_records_from_test_result, ExecutionIdentity,
    ExecutionRecord, ExecutionStatus, IngestError, RecordSource, RecordWindow, ResourceEnvelope,
    VerificationOutcome, EXECUTION_RECORD_SCHEMA,
};
