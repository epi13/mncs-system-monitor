//! Machine-readable projection of correlation results.
//!
//! JSON is transport only. Every section carries its schema, every link names its linkage
//! (including explicit unknown states), and anomalies carry their explanatory detail.

use serde_json::{json, Value};

use crate::correlate::{AnomalyKind, CorrelationReport, Linkage, RestartClass, RestartReport};

/// Versioned correlation section schema.
pub const CORRELATION_SCHEMA: &str = "mncs.system-monitor.correlation.v1";
/// Versioned restart-reconciliation section schema.
pub const RESTART_SCHEMA: &str = "mncs.system-monitor.restart.v1";

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
}
