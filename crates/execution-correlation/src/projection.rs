//! Machine-readable projection of correlation results.
//!
//! JSON is transport only. Every section carries its schema, every link names its linkage
//! (including explicit unknown states), and anomalies carry their explanatory detail.

use serde_json::{json, Value};

use crate::correlate::{AnomalyKind, CorrelationReport, Linkage, RestartClass, RestartReport};
use crate::record::{ExecutionRecord, ExecutionStatus, RecordSource, VerificationOutcome};

/// Versioned correlation section schema.
pub const CORRELATION_SCHEMA: &str = "mncs.system-monitor.correlation.v1";
/// Versioned restart-reconciliation section schema.
pub const RESTART_SCHEMA: &str = "mncs.system-monitor.restart.v1";
/// Versioned projection of ingested execution identities, declared limits and observations.
pub const EXECUTION_RECORDS_SCHEMA: &str = "mncs.system-monitor.execution-records.v1";

fn linkage_name(linkage: &Linkage) -> &'static str {
    match linkage {
        Linkage::Linked { .. } => "linked",
        Linkage::PidReused => "pid_reused",
        Linkage::ProcessAbsent => "process_absent",
        Linkage::NoPidDeclared => "no_pid_declared",
        Linkage::LinkageUnknown => "linkage_unknown",
    }
}

fn anomaly_name(kind: AnomalyKind) -> &'static str {
    match kind {
        AnomalyKind::StaleActiveExecution => "stale_active_execution",
        AnomalyKind::LingeringProcess => "lingering_process",
        AnomalyKind::PidReused => "pid_reused",
        AnomalyKind::EnvelopeExceeded => "envelope_exceeded",
        AnomalyKind::SaturationWithUnknown => "saturation_with_unknown",
        AnomalyKind::WatchedOrphan => "watched_orphan",
    }
}

fn restart_name(class: RestartClass) -> &'static str {
    match class {
        RestartClass::ActiveLinked => "active_linked",
        RestartClass::StaleActive => "stale_active",
        RestartClass::CompletedObserved => "completed_observed",
        RestartClass::Lingering => "lingering",
        RestartClass::PidAmbiguous => "pid_ambiguous",
        RestartClass::LinkageUnknown => "linkage_unknown",
    }
}

/// Project a correlation report as a versioned JSON section for merging into the snapshot
/// envelope. TUI and agent consumers read the same section; neither scrapes the other.
pub fn correlation_report_value(report: &CorrelationReport) -> Value {
    let linked = report
        .links
        .iter()
        .filter(|link| matches!(link.linkage, Linkage::Linked { .. }))
        .count();
    json!({
        "schema": CORRELATION_SCHEMA,
        "executions": report.links.len(),
        "linked": linked,
        "anomalies": report.anomalies.len(),
        "links": report.links.iter().map(|link| {
            json!({
                "execution": link.execution.0,
                "linkage": linkage_name(&link.linkage),
                "pid": link.process.as_ref().map(|identity| identity.pid.0),
                "start_marker": link.process.as_ref().and_then(|identity| identity.start_marker),
            })
        }).collect::<Vec<_>>(),
        "anomaly_details": report.anomalies.iter().map(|anomaly| {
            json!({
                "kind": anomaly_name(anomaly.kind),
                "execution": anomaly.execution.as_ref().map(|identity| &identity.0),
                "pid": anomaly.process.as_ref().map(|identity| identity.pid.0),
                "detail": anomaly.detail,
                "explains_unknown": anomaly.explains_unknown,
            })
        }).collect::<Vec<_>>(),
    })
}

/// Project a restart-reconciliation report as a versioned JSON section.
pub fn restart_report_value(report: &RestartReport) -> Value {
    json!({
        "schema": RESTART_SCHEMA,
        "items": report.items.iter().map(|item| {
            json!({
                "execution": item.execution.0,
                "class": restart_name(item.class),
                "process_state": item.process_state.map(|state| format!("{state:?}")),
            })
        }).collect::<Vec<_>>(),
    })
}

fn status_name(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Active => "active",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Cancelled => "cancelled",
        ExecutionStatus::Unknown => "unknown",
    }
}

fn source_name(source: RecordSource) -> &'static str {
    match source {
        RecordSource::ForgeReceipt => "forge_receipt",
        RecordSource::ForgeObservation => "forge_observation",
        RecordSource::TestResult => "test_result",
        RecordSource::Manual => "manual",
        RecordSource::Unknown => "unknown",
    }
}

fn outcome_value(outcome: &VerificationOutcome) -> Value {
    match outcome {
        VerificationOutcome::Pass => json!({"status": "pass"}),
        VerificationOutcome::Fail => json!({"status": "fail"}),
        VerificationOutcome::NotFinished => json!({"status": "not_finished"}),
        VerificationOutcome::Unknown { reason } => {
            json!({"status": "unknown", "reason": reason})
        }
    }
}

/// Project the ingested records alongside the live correlation section. Operational facts and
/// measurements remain separate from the verification outcome carried by the source record.
pub fn execution_records_value(records: &[ExecutionRecord]) -> Value {
    json!({
        "schema": EXECUTION_RECORDS_SCHEMA,
        "records": records.iter().map(|record| {
            json!({
                "execution": record.identity.0,
                "source": source_name(record.source),
                "status": status_name(record.status),
                "verification_outcome": outcome_value(&record.outcome),
                "harness_status": record.harness_status,
                "termination": {
                    "category": record.termination_category,
                    "error_code": record.termination_error_code,
                    "process_exit_code": record.process_exit_code,
                    "process_signal": record.process_signal,
                },
                "process_identity": {
                    "host_pid": record.host_pid,
                    "host_start_marker": record.host_start_marker,
                },
                "resource_envelope": {
                    "declared": record.envelope.declared,
                    "memory_max_bytes": record.envelope.memory_max_bytes,
                    "process_count_max": record.envelope.process_count_max,
                    "cpu_quota_cores": record.envelope.cpu_quota_cores,
                    "disk_min_free_bytes": record.envelope.disk_min_free_bytes,
                },
                "resource_observations": record.resource_observations,
            })
        }).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correlate::{Anomaly, CorrelationReport, ExecutionLink};
    use crate::record::ExecutionIdentity;
    use mncs_monitor_core::ProcessIdentity;

    #[test]
    fn correlation_section_carries_schema_and_counts() {
        let report = CorrelationReport {
            links: vec![ExecutionLink {
                execution: ExecutionIdentity("e1".to_string()),
                process: Some(ProcessIdentity::new(10, Some(3))),
                linkage: Linkage::Linked {
                    confidence: crate::correlate::LinkConfidence::ExactPid,
                },
            }],
            anomalies: vec![Anomaly {
                kind: AnomalyKind::WatchedOrphan,
                execution: None,
                process: Some(ProcessIdentity::new(11, None)),
                detail: "watched".to_string(),
                explains_unknown: false,
            }],
        };
        let value = correlation_report_value(&report);
        assert_eq!(value["schema"], Value::from(CORRELATION_SCHEMA));
        assert_eq!(value["executions"], Value::from(1));
        assert_eq!(value["linked"], Value::from(1));
        assert_eq!(value["anomalies"], Value::from(1));
        assert_eq!(value["links"][0]["linkage"], Value::from("linked"));
        assert_eq!(
            value["anomaly_details"][0]["kind"],
            Value::from("watched_orphan")
        );
    }

    #[test]
    fn restart_section_names_classes() {
        let report = RestartReport {
            items: vec![crate::correlate::RestartItem {
                execution: ExecutionIdentity("e1".to_string()),
                class: RestartClass::StaleActive,
                process_state: None,
            }],
        };
        let value = restart_report_value(&report);
        assert_eq!(value["schema"], Value::from(RESTART_SCHEMA));
        assert_eq!(value["items"][0]["class"], Value::from("stale_active"));
    }

    #[test]
    fn execution_records_keep_measurements_and_unknown_semantics_separate() {
        let records = crate::record::execution_records_from_json(
            r#"{"record_type":"mncs-execution-receipt","schema_version":"0.1-experimental",
                "receipt_identity":"receipt-1","lifecycle":{"termination_category":"signal"},
                "process":{"harness_status":"UNKNOWN","exit_code":null,"signal":9},
                "extensions":{"forge:local-process":{"termination_error_code":null,
                    "resource_envelope":{"memory_max_bytes":4096,"tasks_max":8},
                    "resource_observations":{"resource_observations":{
                        "cgroup_memory_peak_bytes":3000,"host_pid":4,
                        "host_start_marker":99}}}}}"#,
        )
        .expect("Forge receipt parses");
        let value = execution_records_value(&records);
        assert_eq!(value["schema"], Value::from(EXECUTION_RECORDS_SCHEMA));
        assert_eq!(
            value["records"][0]["harness_status"],
            Value::from("UNKNOWN")
        );
        assert_eq!(
            value["records"][0]["termination"]["process_signal"],
            Value::from(9)
        );
        assert_eq!(
            value["records"][0]["resource_envelope"]["process_count_max"],
            Value::from(8)
        );
        assert_eq!(
            value["records"][0]["resource_observations"]["cgroup_memory_peak_bytes"],
            Value::from(3000)
        );
        assert_eq!(
            value["records"][0]["verification_outcome"]["status"],
            Value::from("unknown")
        );
    }
}
