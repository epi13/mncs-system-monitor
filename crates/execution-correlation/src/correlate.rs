//! Pure correlation between ingested execution records and a host snapshot.
//!
//! All functions are total over their inputs and allocate only in proportion to the record and
//! process counts. Linkage is exact (PID plus start marker) or explicitly declared; there is no
//! fuzzy tier, so an unlinkable record yields [`Linkage::LinkageUnknown`], never a guess.

use std::collections::BTreeMap;

use mncs_monitor_core::{PressureLevel, ProcessIdentity, ProcessState, SystemSnapshot};

use crate::record::{ExecutionIdentity, ExecutionRecord, ExecutionStatus, VerificationOutcome};

/// How a record relates to the live process table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Linkage {
    /// Declared PID (and start marker, when the record carries one) matches a live process.
    Linked { confidence: LinkConfidence },
    /// The declared PID exists but its start marker differs: probable PID reuse. The record is
    /// not linked to that process.
    PidReused,
    /// The declared PID is absent from the snapshot.
    ProcessAbsent,
    /// The record declares no host PID, so no host linkage is possible.
    NoPidDeclared,
    /// The canonical source itself is unavailable: correlation cannot be evaluated.
    LinkageUnknown,
}

/// Strength of an established link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkConfidence {
    /// PID and start marker both match.
    ExactPid,
    /// PID matches and the record carries no start marker to check against.
    DeclaredPidNoMarker,
}

/// One record's linkage result. `process` is `Some` only for [`Linkage::Linked`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionLink {
    pub execution: ExecutionIdentity,
    pub process: Option<ProcessIdentity>,
    pub linkage: Linkage,
}

/// The anomaly vocabulary. Each variant explains why it matters in `detail`; variants that only
/// explain an existing UNKNOWN carry `explains_unknown`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnomalyKind {
    /// An Active record whose declared process is absent: host loss, or stale canonical state.
    StaleActiveExecution,
    /// A Completed/Cancelled record whose declared process is still alive and consuming.
    LingeringProcess,
    /// A live PID collides with a record's declared PID under a different start marker.
    PidReused,
    /// Observed resident memory exceeds the admitted envelope bound.
    EnvelopeExceeded,
    /// Host pressure at Full level coincides with an Unknown outcome: operational context for
    /// the UNKNOWN, never a verdict change.
    SaturationWithUnknown,
    /// An operator-watched executable runs with no linked Active execution.
    WatchedOrphan,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anomaly {
    pub kind: AnomalyKind,
    pub execution: Option<ExecutionIdentity>,
    pub process: Option<ProcessIdentity>,
    pub detail: String,
    pub explains_unknown: bool,
}

/// Observed host saturation, derived from pressure observations (PSI Full levels), never from
/// monitor-invented thresholds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostSaturation {
    pub cpu_full: bool,
    pub memory_full: bool,
    pub io_full: bool,
}

impl HostSaturation {
    pub fn any(self) -> bool {
        self.cpu_full || self.memory_full || self.io_full
    }

    /// Derive saturation from a snapshot's pressure observations.
    pub fn from_snapshot(snapshot: &SystemSnapshot) -> Self {
        let mut saturation = Self::default();
        for pressure in &snapshot.pressure {
            if pressure.level != PressureLevel::Full {
                continue;
            }
            match pressure.kind {
                mncs_monitor_core::PressureKind::Cpu => saturation.cpu_full = true,
                mncs_monitor_core::PressureKind::Memory => saturation.memory_full = true,
                mncs_monitor_core::PressureKind::Io => saturation.io_full = true,
                _ => {}
            }
        }
        saturation
    }
}

/// The full correlation result for one records-plus-snapshot pair.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CorrelationReport {
    pub links: Vec<ExecutionLink>,
    pub anomalies: Vec<Anomaly>,
}

/// Link every record against the live process table. Records that cannot be evaluated (no
/// canonical input is a separate concern) always produce a link entry; absence of evidence is
/// [`Linkage::LinkageUnknown`] only when the caller marks records unavailable, which this pure
/// function cannot see, so every record here is evaluated on its merits.
pub fn correlate(
    records: &[ExecutionRecord],
    snapshot: &SystemSnapshot,
    watched_executables: &[String],
    saturation: HostSaturation,
) -> CorrelationReport {
    let live: BTreeMap<u32, &mncs_monitor_core::Process> = snapshot
        .processes
        .iter()
        .map(|process| (process.identity.pid.0, process))
        .collect();

    let mut links = Vec::with_capacity(records.len());
    let mut anomalies = Vec::new();

    for record in records {
        let (linkage, linked_process) = link_record(record, &live);
        if linkage == Linkage::PidReused {
            anomalies.push(Anomaly {
                kind: AnomalyKind::PidReused,
                execution: Some(record.identity.clone()),
                process: live
                    .get(&record.host_pid.unwrap_or(u32::MAX))
                    .map(|process| process.identity.clone()),
                detail: format!(
                    "declared PID {} is alive under a different start marker; \
                     linkage withheld (probable PID reuse)",
                    record.host_pid.unwrap_or(0)
                ),
                explains_unknown: false,
            });
        }
        links.push(ExecutionLink {
            execution: record.identity.clone(),
            process: linked_process.clone(),
            linkage,
        });

        match record.status {
            ExecutionStatus::Active => {
                if matches!(
                    links.last().map(|link| &link.linkage),
                    Some(Linkage::ProcessAbsent)
                ) {
                    anomalies.push(Anomaly {
                        kind: AnomalyKind::StaleActiveExecution,
                        execution: Some(record.identity.clone()),
                        process: None,
                        detail: format!(
                            "execution '{}' is Active but declared PID {} is absent; \
                             host loss or stale canonical state",
                            record.identity.0,
                            record.host_pid.unwrap_or(0)
                        ),
                        explains_unknown: matches!(
                            record.outcome,
                            VerificationOutcome::Unknown { .. }
                        ),
                    });
                }
            }
            ExecutionStatus::Completed | ExecutionStatus::Cancelled => {
                if let Some(process) = linked_process.as_ref() {
                    let rss = live
                        .get(&process.pid.0)
                        .and_then(|live_process| live_process.memory.as_ref())
                        .and_then(|memory| memory.resident_bytes)
                        .unwrap_or(0);
                    anomalies.push(Anomaly {
                        kind: AnomalyKind::LingeringProcess,
                        execution: Some(record.identity.clone()),
                        process: Some(process.clone()),
                        detail: format!(
                            "execution '{}' is {:?} but its process is still alive (rss {} bytes)",
                            record.identity.0, record.status, rss
                        ),
                        explains_unknown: false,
                    });
                }
            }
            ExecutionStatus::Unknown => {}
        }

        if let (Some(cap), Some(process)) =
            (record.envelope.memory_max_bytes, linked_process.as_ref())
        {
            let rss = live
                .get(&process.pid.0)
                .and_then(|live_process| live_process.memory.as_ref())
                .and_then(|memory| memory.resident_bytes);
            if rss.is_some_and(|bytes| bytes > cap) {
                anomalies.push(Anomaly {
                    kind: AnomalyKind::EnvelopeExceeded,
                    execution: Some(record.identity.clone()),
                    process: Some(process.clone()),
                    detail: format!(
                        "resident {} bytes exceeds admitted envelope {} bytes",
                        rss.unwrap_or(0),
                        cap
                    ),
                    explains_unknown: false,
                });
            }
        }

        if saturation.any() && matches!(record.outcome, VerificationOutcome::Unknown { .. }) {
            anomalies.push(Anomaly {
                kind: AnomalyKind::SaturationWithUnknown,
                execution: Some(record.identity.clone()),
                process: linked_process,
                detail: format!(
                    "host saturation (cpu={} mem={} io={}) coincides with UNKNOWN outcome of '{}'; \
                     operational context only, not a verdict",
                    saturation.cpu_full, saturation.memory_full, saturation.io_full,
                    record.identity.0
                ),
                explains_unknown: true,
            });
        }
    }

    let linked_pids: Vec<u32> = links
        .iter()
        .filter_map(|link| link.process.as_ref().map(|identity| identity.pid.0))
        .collect();
    for process in &snapshot.processes {
        if linked_pids.contains(&process.identity.pid.0) {
            continue;
        }
        if watched_executables
            .iter()
            .any(|watched| watched == &process.executable)
        {
            anomalies.push(Anomaly {
                kind: AnomalyKind::WatchedOrphan,
                execution: None,
                process: Some(process.identity.clone()),
                detail: format!(
                    "watched executable '{}' (pid {}) runs with no linked Active execution",
                    process.executable, process.identity.pid.0
                ),
                explains_unknown: false,
            });
        }
    }

    CorrelationReport { links, anomalies }
}

fn link_record(
    record: &ExecutionRecord,
    live: &BTreeMap<u32, &mncs_monitor_core::Process>,
) -> (Linkage, Option<ProcessIdentity>) {
    let live_identity = record
        .host_pid
        .and_then(|pid| live.get(&pid))
        .map(|process| process.identity.clone());
    link_record_inner(record.host_pid, record.host_start_marker, &live_identity)
}

/// Shared linkage rule used by both live correlation and restart reconciliation: a PID match
/// alone never links when both sides carry start markers that disagree.
fn link_record_inner(
    host_pid: Option<u32>,
    host_start_marker: Option<u64>,
    live_identity: &Option<ProcessIdentity>,
) -> (Linkage, Option<ProcessIdentity>) {
    let Some(_pid) = host_pid else {
        return (Linkage::NoPidDeclared, None);
    };
    let Some(identity) = live_identity else {
        return (Linkage::ProcessAbsent, None);
    };
    match (host_start_marker, identity.start_marker) {
        (Some(declared), Some(observed)) if declared == observed => (
            Linkage::Linked {
                confidence: LinkConfidence::ExactPid,
            },
            Some(identity.clone()),
        ),
        (Some(_), Some(_)) => (Linkage::PidReused, None),
        _ => (
            Linkage::Linked {
                confidence: LinkConfidence::DeclaredPidNoMarker,
            },
            Some(identity.clone()),
        ),
    }
}

/// Classification of one record when the monitor restarts and reconciles retained records
/// against the fresh host snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartClass {
    /// Active record with its process alive: resume observation.
    ActiveLinked,
    /// Active record with its process absent: host loss or stale state; outcome stays UNKNOWN.
    StaleActive,
    /// Finished record with its process gone: nothing to do.
    CompletedObserved,
    /// Finished record with its process alive: lingering; report, do not kill.
    Lingering,
    /// Declared PID is alive under a different start marker: neither resume nor declare
    /// staleness; the live process belongs to someone else.
    PidAmbiguous,
    /// No PID declared or status Unknown: cannot classify; report as such.
    LinkageUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestartItem {
    pub execution: ExecutionIdentity,
    pub class: RestartClass,
    pub process_state: Option<ProcessState>,
}

/// Reconcile retained records against a fresh snapshot after monitor restart or host
/// interruption. Terminal records are never resurrected and UNKNOWN is never resolved here:
/// classification only, no state mutation anywhere.
pub fn reconcile_after_restart(
    records: &[ExecutionRecord],
    snapshot: &SystemSnapshot,
) -> RestartReport {
    let live: BTreeMap<u32, &mncs_monitor_core::Process> = snapshot
        .processes
        .iter()
        .map(|process| (process.identity.pid.0, process))
        .collect();
    let report = RestartReport {
        items: records
            .iter()
            .map(|record| {
                let (linkage, _) = link_record(record, &live);
                let process_state = record
                    .host_pid
                    .and_then(|pid| live.get(&pid).map(|process| process.state));
                let class = match (record.status, &linkage) {
                    (ExecutionStatus::Active, Linkage::Linked { .. }) => RestartClass::ActiveLinked,
                    (ExecutionStatus::Active, Linkage::ProcessAbsent) => RestartClass::StaleActive,
                    (ExecutionStatus::Active, Linkage::PidReused) => RestartClass::PidAmbiguous,
                    (
                        ExecutionStatus::Completed | ExecutionStatus::Cancelled,
                        Linkage::Linked { .. },
                    ) => RestartClass::Lingering,
                    (ExecutionStatus::Completed | ExecutionStatus::Cancelled, _) => {
                        RestartClass::CompletedObserved
                    }
                    _ => RestartClass::LinkageUnknown,
                };
                RestartItem {
                    execution: record.identity.clone(),
                    class,
                    process_state,
                }
            })
            .collect(),
    };
    report
}

/// The restart reconciliation result: one classification per retained record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestartReport {
    pub items: Vec<RestartItem>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{RecordSource, ResourceEnvelope};
    use mncs_monitor_core::{
        MemorySample, ObservationSource, ObservationStatus, ObservationTime, Process,
    };
    use std::time::SystemTime;

    fn time() -> ObservationTime {
        ObservationTime {
            wall_time: SystemTime::UNIX_EPOCH,
            monotonic_ns: 1,
        }
    }

    fn live_process(pid: u32, marker: Option<u64>, executable: &str, rss: u64) -> Process {
        Process {
            identity: ProcessIdentity::new(pid, marker),
            parent: None,
            executable: executable.to_string(),
            command_line: None,
            uid: None,
            user: None,
            state: ProcessState::Running,
            threads: None,
            cpu: None,
            memory: Some(MemorySample {
                resident_bytes: Some(rss),
                virtual_bytes: None,
                observed_at: time(),
                source: ObservationSource::Synthetic,
                status: ObservationStatus::Observed,
            }),
            memory_status: ObservationStatus::Observed,
            io: None,
            io_status: ObservationStatus::Unknown,
        }
    }

    fn record(
        identity: &str,
        status: ExecutionStatus,
        pid: Option<u32>,
        marker: Option<u64>,
    ) -> ExecutionRecord {
        ExecutionRecord {
            identity: ExecutionIdentity(identity.to_string()),
            repository: None,
            revision: None,
            argv: Vec::new(),
            executable: None,
            host_pid: pid,
            host_start_marker: marker,
            envelope: ResourceEnvelope::default(),
            resource_observations: Default::default(),
            harness_status: None,
            termination_category: None,
            termination_error_code: None,
            process_exit_code: None,
            process_signal: None,
            status,
            outcome: VerificationOutcome::NotFinished,
            verification_identity: None,
            evidence_path: None,
            source: RecordSource::Manual,
        }
    }

    fn snapshot_with(processes: Vec<Process>) -> SystemSnapshot {
        SystemSnapshot {
            processes,
            ..SystemSnapshot::default()
        }
    }

    #[test]
    fn exact_pid_and_marker_links() {
        let snapshot = snapshot_with(vec![live_process(100, Some(7), "worker", 10)]);
        let report = correlate(
            &[record("a", ExecutionStatus::Active, Some(100), Some(7))],
            &snapshot,
            &[],
            HostSaturation::default(),
        );
        assert_eq!(report.links.len(), 1);
        assert_eq!(
            report.links[0].linkage,
            Linkage::Linked {
                confidence: LinkConfidence::ExactPid
            }
        );
        assert!(report.anomalies.is_empty());
    }

    #[test]
    fn pid_reuse_withholds_linkage() {
        let snapshot = snapshot_with(vec![live_process(100, Some(9), "other", 10)]);
        let report = correlate(
            &[record("a", ExecutionStatus::Active, Some(100), Some(7))],
            &snapshot,
            &[],
            HostSaturation::default(),
        );
        assert_eq!(report.links[0].linkage, Linkage::PidReused);
        assert!(report.links[0].process.is_none());
        assert!(report
            .anomalies
            .iter()
            .any(|anomaly| anomaly.kind == AnomalyKind::PidReused));
    }

    #[test]
    fn stale_active_preserves_unknown() {
        let snapshot = snapshot_with(vec![]);
        let mut rec = record("a", ExecutionStatus::Active, Some(100), Some(7));
        rec.outcome = VerificationOutcome::Unknown {
            reason: Some("interrupted".to_string()),
        };
        let report = correlate(&[rec], &snapshot, &[], HostSaturation::default());
        assert_eq!(report.links[0].linkage, Linkage::ProcessAbsent);
        let anomaly = report
            .anomalies
            .iter()
            .find(|anomaly| anomaly.kind == AnomalyKind::StaleActiveExecution)
            .expect("stale-active anomaly");
        assert!(anomaly.explains_unknown);
    }

    #[test]
    fn completed_but_alive_lingers() {
        let snapshot = snapshot_with(vec![live_process(100, Some(7), "worker", 50)]);
        let report = correlate(
            &[record("a", ExecutionStatus::Completed, Some(100), Some(7))],
            &snapshot,
            &[],
            HostSaturation::default(),
        );
        assert!(report
            .anomalies
            .iter()
            .any(|anomaly| anomaly.kind == AnomalyKind::LingeringProcess));
    }

    #[test]
    fn envelope_exceeded_only_with_declared_bound() {
        let snapshot = snapshot_with(vec![live_process(100, Some(7), "worker", 2_000)]);
        let mut rec = record("a", ExecutionStatus::Active, Some(100), Some(7));
        rec.envelope = ResourceEnvelope {
            memory_max_bytes: Some(1_000),
            process_count_max: None,
            cpu_quota_cores: None,
            disk_min_free_bytes: None,
            declared: true,
        };
        let report = correlate(&[rec.clone()], &snapshot, &[], HostSaturation::default());
        assert!(report
            .anomalies
            .iter()
            .any(|anomaly| anomaly.kind == AnomalyKind::EnvelopeExceeded));

        rec.envelope = ResourceEnvelope::default();
        let report = correlate(&[rec], &snapshot, &[], HostSaturation::default());
        assert!(report
            .anomalies
            .iter()
            .all(|anomaly| anomaly.kind != AnomalyKind::EnvelopeExceeded));
    }

    #[test]
    fn saturation_explains_but_never_converts_unknown() {
        let snapshot = snapshot_with(vec![]);
        let mut rec = record("a", ExecutionStatus::Completed, None, None);
        rec.outcome = VerificationOutcome::Unknown {
            reason: Some("power loss".to_string()),
        };
        let saturation = HostSaturation {
            memory_full: true,
            ..HostSaturation::default()
        };
        let report = correlate(&[rec], &snapshot, &[], saturation);
        let anomaly = report
            .anomalies
            .iter()
            .find(|anomaly| anomaly.kind == AnomalyKind::SaturationWithUnknown)
            .expect("saturation context");
        assert!(anomaly.explains_unknown);
        assert!(matches!(report.links[0].linkage, Linkage::NoPidDeclared));
    }

    #[test]
    fn watched_orphan_matches_exact_executable_only() {
        let snapshot = snapshot_with(vec![
            live_process(100, Some(7), "watched-worker", 10),
            live_process(101, Some(8), "watched-worker-extra", 10),
        ]);
        let report = correlate(
            &[],
            &snapshot,
            &["watched-worker".to_string()],
            HostSaturation::default(),
        );
        let orphans: Vec<&Anomaly> = report
            .anomalies
            .iter()
            .filter(|anomaly| anomaly.kind == AnomalyKind::WatchedOrphan)
            .collect();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].process.as_ref().expect("pid").pid.0, 100);
    }

    #[test]
    fn restart_reconciliation_classifies() {
        let snapshot = snapshot_with(vec![live_process(100, Some(7), "worker", 10)]);
        let report = reconcile_after_restart(
            &[
                record("active-alive", ExecutionStatus::Active, Some(100), Some(7)),
                record("active-gone", ExecutionStatus::Active, Some(200), Some(7)),
                record("active-reused", ExecutionStatus::Active, Some(100), Some(8)),
                record("done-alive", ExecutionStatus::Completed, Some(100), Some(7)),
                record(
                    "done-reused",
                    ExecutionStatus::Completed,
                    Some(100),
                    Some(8),
                ),
                record("done-gone", ExecutionStatus::Completed, Some(300), None),
                record("no-pid", ExecutionStatus::Active, None, None),
            ],
            &snapshot,
        );
        let class = |identity: &str| {
            report
                .items
                .iter()
                .find(|item| item.execution.0 == identity)
                .expect("item")
                .class
        };
        assert_eq!(class("active-alive"), RestartClass::ActiveLinked);
        assert_eq!(class("active-gone"), RestartClass::StaleActive);
        assert_eq!(class("active-reused"), RestartClass::PidAmbiguous);
        assert_eq!(class("done-alive"), RestartClass::Lingering);
        assert_eq!(class("done-reused"), RestartClass::CompletedObserved);
        assert_eq!(class("done-gone"), RestartClass::CompletedObserved);
        assert_eq!(class("no-pid"), RestartClass::LinkageUnknown);
    }
}
