use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use crate::{
    EventKind, HistorySample, MonitorEvent, ObservationStatus, ObservationTime, Process,
    ProcessHistoryPoint, ProcessId, ProcessIdentity, ProcessLifecycle, ProcessRelationship,
    ProcessTransition, RelationshipStatus, SystemSnapshot,
};

pub const DEFAULT_HISTORY_CAPACITY: usize = 120;
const HISTORY_PROCESS_CAPACITY: usize = 512;

/// The result of accepting a raw collector snapshot. Both projections consume the same snapshot;
/// reconciliation is returned separately so a view never has to infer lifecycle from text.
#[derive(Clone, Debug)]
pub struct AcceptedSnapshot {
    pub snapshot: SystemSnapshot,
}

/// A bounded stateful sampler. The collector provides facts and counters; this layer owns temporal
/// validity, counter continuity, lifecycle reconciliation, rates, events, and retention.
#[derive(Clone, Debug)]
pub struct Sampler {
    history_capacity: usize,
    previous: Option<SystemSnapshot>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_CAPACITY)
    }
}

impl Sampler {
    pub fn new(history_capacity: usize) -> Self {
        Self {
            history_capacity: history_capacity.clamp(1, 1_000),
            previous: None,
        }
    }

    pub fn history_capacity(&self) -> usize {
        self.history_capacity
    }

    pub fn accept(&mut self, mut current: SystemSnapshot) -> AcceptedSnapshot {
        let previous = self.previous.as_ref();
        let interval =
            previous.and_then(|old| sample_interval(old.sample_time, current.sample_time));
        let transitions = reconcile_processes(
            previous.map(|snapshot| &snapshot.processes),
            &current.processes,
        );

        derive_host_cpu(previous, &mut current, interval);
        derive_process_metrics(previous, &mut current, interval);
        derive_network(previous, &mut current, interval);
        derive_disk(previous, &mut current, interval);

        current.relationships = build_relationships(&current.processes);
        current.transitions = transitions.clone();
        current.events = build_events(previous, &current, &transitions, interval);

        let mut history = previous.map_or_else(VecDeque::new, |snapshot| snapshot.history.clone());
        history.push_back(history_sample(&current));
        while history.len() > self.history_capacity {
            history.pop_front();
        }
        current.history = history;

        self.previous = Some(current.clone());
        AcceptedSnapshot { snapshot: current }
    }

    pub fn previous(&self) -> Option<&SystemSnapshot> {
        self.previous.as_ref()
    }
}

fn sample_interval(old: Option<ObservationTime>, new: Option<ObservationTime>) -> Option<Duration> {
    let old = old?;
    let new = new?;
    new.monotonic_ns
        .checked_sub(old.monotonic_ns)
        .map(Duration::from_nanos)
        .filter(|duration| !duration.is_zero())
}

fn counter_delta(old: Option<u64>, new: Option<u64>) -> Result<Option<u64>, ObservationStatus> {
    match (old, new) {
        (Some(old), Some(new)) if new >= old => Ok(Some(new - old)),
        (Some(_), Some(_)) => Err(ObservationStatus::CounterRegression),
        _ => Ok(None),
    }
}

fn rate(delta: Option<u64>, interval: Option<Duration>) -> Result<Option<f64>, ObservationStatus> {
    let Some(delta) = delta else {
        return Ok(None);
    };
    let Some(interval) = interval else {
        return Err(ObservationStatus::InvalidInterval);
    };
    let seconds = interval.as_secs_f64();
    if seconds <= 0.0 || !seconds.is_finite() {
        return Err(ObservationStatus::InvalidInterval);
    }
    Ok(Some(delta as f64 / seconds))
}

fn same_identity(old: &ProcessIdentity, new: &ProcessIdentity) -> Option<bool> {
    if old.pid != new.pid {
        return Some(false);
    }
    match (old.start_marker, new.start_marker) {
        (Some(old), Some(new)) => Some(old == new),
        (None, None) | (Some(_), None) | (None, Some(_)) => None,
    }
}

pub fn reconcile_processes(
    previous: Option<&Vec<Process>>,
    current: &[Process],
) -> Vec<ProcessTransition> {
    let old_by_pid: BTreeMap<ProcessId, ProcessIdentity> = previous
        .into_iter()
        .flatten()
        .map(|process| (process.identity.pid, process.identity.clone()))
        .collect();
    let new_by_pid: BTreeMap<ProcessId, ProcessIdentity> = current
        .iter()
        .map(|process| (process.identity.pid, process.identity.clone()))
        .collect();

    let mut pids: Vec<ProcessId> = old_by_pid
        .keys()
        .chain(new_by_pid.keys())
        .copied()
        .collect();
    pids.sort_unstable();
    pids.dedup();

    pids.into_iter()
        .map(|pid| {
            let old = old_by_pid.get(&pid).cloned();
            let new = new_by_pid.get(&pid).cloned();
            let lifecycle = match (&old, &new) {
                (None, Some(_)) => ProcessLifecycle::New,
                (Some(_), None) => ProcessLifecycle::NoLongerObserved,
                (Some(old), Some(new)) => match same_identity(old, new) {
                    Some(true) => ProcessLifecycle::Same,
                    Some(false) => ProcessLifecycle::IdentityChanged,
                    None => ProcessLifecycle::IdentityAmbiguous,
                },
                (None, None) => unreachable!("PID set is built from both maps"),
            };
            ProcessTransition {
                pid,
                previous: old,
                current: new,
                lifecycle,
            }
        })
        .collect()
}

fn previous_process<'a>(
    previous: Option<&'a SystemSnapshot>,
    identity: &ProcessIdentity,
) -> Option<&'a Process> {
    let previous = previous?;
    previous
        .processes
        .iter()
        .find(|process| same_identity(&process.identity, identity) == Some(true))
}

fn derive_host_cpu(
    previous: Option<&SystemSnapshot>,
    current: &mut SystemSnapshot,
    interval: Option<Duration>,
) {
    let Some(cpu) = current.host_cpu.as_mut() else {
        return;
    };
    let Some(old) = previous.and_then(|snapshot| snapshot.host_cpu.as_ref()) else {
        return;
    };
    let total = match counter_delta(old.total_ticks, cpu.total_ticks) {
        Ok(Some(delta)) if delta > 0 => delta,
        Ok(Some(_)) => {
            cpu.status = ObservationStatus::Stale;
            return;
        }
        Ok(None) => return,
        Err(status) => {
            cpu.status = status;
            return;
        }
    };
    let busy = match counter_delta(old.busy_ticks, cpu.busy_ticks) {
        Ok(value) => value,
        Err(status) => {
            cpu.status = status;
            return;
        }
    };
    cpu.interval = interval;
    cpu.utilization = busy.map(|busy| (busy as f64 / total as f64).clamp(0.0, 1.0));
    cpu.status = if interval.is_some() && cpu.utilization.is_some() {
        ObservationStatus::Observed
    } else {
        ObservationStatus::InvalidInterval
    };
}

fn derive_process_metrics(
    previous: Option<&SystemSnapshot>,
    current: &mut SystemSnapshot,
    interval: Option<Duration>,
) {
    let host_total = previous
        .and_then(|snapshot| snapshot.host_cpu.as_ref())
        .and_then(|cpu| {
            current.host_cpu.as_ref().and_then(|now| {
                counter_delta(cpu.total_ticks, now.total_ticks)
                    .ok()
                    .flatten()
            })
        });
    let per_core_total =
        host_total.map(|total| total as f64 / current.cpu_count.unwrap_or(1).max(1) as f64);

    for process in &mut current.processes {
        let Some(old) = previous_process(previous, &process.identity) else {
            if let Some(cpu) = process.cpu.as_mut() {
                cpu.interval = None;
                cpu.utilization = None;
                cpu.status = ObservationStatus::Unknown;
            }
            if let Some(io) = process.io.as_mut() {
                io.interval = None;
                io.read_rate_bytes_per_sec = None;
                io.written_rate_bytes_per_sec = None;
                io.status = ObservationStatus::Unknown;
            }
            continue;
        };

        if let Some(cpu) = process.cpu.as_mut() {
            let old_ticks = old.cpu.as_ref().and_then(|sample| sample.process_ticks);
            match counter_delta(old_ticks, cpu.process_ticks) {
                Ok(Some(delta)) => {
                    cpu.interval = interval;
                    cpu.utilization = per_core_total.and_then(|denominator| {
                        (denominator > 0.0).then_some((delta as f64 / denominator).max(0.0))
                    });
                    cpu.status = if cpu.utilization.is_some() {
                        ObservationStatus::Observed
                    } else {
                        ObservationStatus::InvalidInterval
                    };
                }
                Ok(None) => cpu.status = ObservationStatus::Unknown,
                Err(status) => {
                    cpu.status = status;
                    cpu.utilization = None;
                }
            }
        }

        if let Some(io) = process.io.as_mut() {
            let old_io = old.io.as_ref();
            let read = counter_delta(old_io.and_then(|sample| sample.read_bytes), io.read_bytes);
            let written = counter_delta(
                old_io.and_then(|sample| sample.written_bytes),
                io.written_bytes,
            );
            let read_rate = read.and_then(|delta| rate(delta, interval));
            let written_rate = written.and_then(|delta| rate(delta, interval));
            io.interval = interval;
            io.read_rate_bytes_per_sec = read_rate.as_ref().ok().and_then(|value| *value);
            io.written_rate_bytes_per_sec = written_rate.as_ref().ok().and_then(|value| *value);
            io.status = first_error([
                read.err(),
                written.err(),
                read_rate.err(),
                written_rate.err(),
            ])
            .unwrap_or_else(|| {
                if io.read_rate_bytes_per_sec.is_some() || io.written_rate_bytes_per_sec.is_some() {
                    ObservationStatus::Observed
                } else {
                    ObservationStatus::Unknown
                }
            });
        }
    }
}

fn derive_network(
    previous: Option<&SystemSnapshot>,
    current: &mut SystemSnapshot,
    interval: Option<Duration>,
) {
    for network in &mut current.network {
        let old = previous.and_then(|snapshot| {
            snapshot
                .network
                .iter()
                .find(|sample| sample.interface == network.interface)
        });
        let Some(old) = old else {
            network.interval = None;
            network.received_rate_bytes_per_sec = None;
            network.transmitted_rate_bytes_per_sec = None;
            network.status = ObservationStatus::Unknown;
            continue;
        };
        let received = counter_delta(old.received_bytes, network.received_bytes);
        let transmitted = counter_delta(old.transmitted_bytes, network.transmitted_bytes);
        let received_rate = received.and_then(|delta| rate(delta, interval));
        let transmitted_rate = transmitted.and_then(|delta| rate(delta, interval));
        network.interval = interval;
        network.received_rate_bytes_per_sec = received_rate.as_ref().ok().and_then(|value| *value);
        network.transmitted_rate_bytes_per_sec =
            transmitted_rate.as_ref().ok().and_then(|value| *value);
        network.status = first_error([
            received.err(),
            transmitted.err(),
            received_rate.err(),
            transmitted_rate.err(),
        ])
        .unwrap_or_else(|| {
            if network.received_rate_bytes_per_sec.is_some()
                || network.transmitted_rate_bytes_per_sec.is_some()
            {
                ObservationStatus::Observed
            } else {
                ObservationStatus::Unknown
            }
        });
    }
}

fn derive_disk(
    previous: Option<&SystemSnapshot>,
    current: &mut SystemSnapshot,
    interval: Option<Duration>,
) {
    for disk in &mut current.disk {
        let old = previous.and_then(|snapshot| {
            snapshot
                .disk
                .iter()
                .find(|sample| sample.device == disk.device)
        });
        let Some(old) = old else {
            disk.interval = None;
            disk.read_rate_bytes_per_sec = None;
            disk.written_rate_bytes_per_sec = None;
            disk.status = ObservationStatus::Unknown;
            continue;
        };
        let read = counter_delta(old.read_bytes, disk.read_bytes);
        let written = counter_delta(old.written_bytes, disk.written_bytes);
        let read_rate = read.and_then(|delta| rate(delta, interval));
        let written_rate = written.and_then(|delta| rate(delta, interval));
        disk.interval = interval;
        disk.read_rate_bytes_per_sec = read_rate.as_ref().ok().and_then(|value| *value);
        disk.written_rate_bytes_per_sec = written_rate.as_ref().ok().and_then(|value| *value);
        disk.status = first_error([
            read.err(),
            written.err(),
            read_rate.err(),
            written_rate.err(),
        ])
        .unwrap_or_else(|| {
            if disk.read_rate_bytes_per_sec.is_some() || disk.written_rate_bytes_per_sec.is_some() {
                ObservationStatus::Observed
            } else {
                ObservationStatus::Unknown
            }
        });
    }
}

fn first_error<const N: usize>(
    statuses: [Option<ObservationStatus>; N],
) -> Option<ObservationStatus> {
    statuses.into_iter().flatten().next()
}

fn build_relationships(processes: &[Process]) -> Vec<ProcessRelationship> {
    let by_pid: BTreeMap<ProcessId, ProcessIdentity> = processes
        .iter()
        .map(|process| (process.identity.pid, process.identity.clone()))
        .collect();
    processes
        .iter()
        .map(|process| match process.parent {
            Some(pid) => match by_pid.get(&pid) {
                Some(parent) => ProcessRelationship {
                    child: process.identity.clone(),
                    parent: Some(parent.clone()),
                    status: RelationshipStatus::Resolved,
                },
                None => ProcessRelationship {
                    child: process.identity.clone(),
                    parent: None,
                    status: RelationshipStatus::ParentNotObserved,
                },
            },
            None => ProcessRelationship {
                child: process.identity.clone(),
                parent: None,
                status: RelationshipStatus::Resolved,
            },
        })
        .collect()
}

fn build_events(
    previous: Option<&SystemSnapshot>,
    current: &SystemSnapshot,
    transitions: &[ProcessTransition],
    interval: Option<Duration>,
) -> Vec<MonitorEvent> {
    let at = current.sample_time.unwrap_or(ObservationTime {
        wall_time: current.observed_at.unwrap_or(std::time::UNIX_EPOCH),
        monotonic_ns: 0,
    });
    let mut events = Vec::new();
    for transition in transitions {
        match transition.lifecycle {
            ProcessLifecycle::New => events.push(MonitorEvent {
                kind: EventKind::ProcessObserved,
                observed_at: at,
                subject: transition.current.clone(),
                detail: "process identity observed for the first time in this sampler".into(),
                source: crate::ObservationSource::ProcStat,
            }),
            ProcessLifecycle::NoLongerObserved => events.push(MonitorEvent {
                kind: EventKind::ProcessNoLongerObserved,
                observed_at: at,
                subject: transition.previous.clone(),
                detail: "PID was not observed in the current snapshot; clean exit is not asserted"
                    .into(),
                source: crate::ObservationSource::ProcStat,
            }),
            ProcessLifecycle::IdentityChanged => events.push(MonitorEvent {
                kind: EventKind::ProcessIdentityChanged,
                observed_at: at,
                subject: transition.current.clone(),
                detail: "PID persisted but its start marker changed".into(),
                source: crate::ObservationSource::ProcStat,
            }),
            ProcessLifecycle::Same | ProcessLifecycle::IdentityAmbiguous => {}
        }
    }
    if previous.is_some() && interval.is_none() {
        events.push(MonitorEvent {
            kind: EventKind::SamplingGap,
            observed_at: at,
            subject: None,
            detail: "monotonic sample interval was missing, zero, or regressed".into(),
            source: crate::ObservationSource::HostApi,
        });
    }
    for issue in &current.issues {
        if issue.status == ObservationStatus::PermissionDenied {
            events.push(MonitorEvent {
                kind: EventKind::PermissionLimited,
                observed_at: at,
                subject: issue.subject.map(|pid| ProcessIdentity::new(pid.0, None)),
                detail: issue.detail.clone(),
                source: issue.source,
            });
        }
    }
    if let Some(cpu) = current
        .host_cpu
        .as_ref()
        .and_then(|sample| sample.utilization)
    {
        if cpu >= 0.90 {
            events.push(MonitorEvent {
                kind: EventKind::CpuSpike,
                observed_at: at,
                subject: None,
                detail: format!("host CPU utilization reached {:.1}%", cpu * 100.0),
                source: cpu_source(current),
            });
        }
    }
    for process in &current.processes {
        if let Some(cpu) = process.cpu.as_ref().and_then(|sample| sample.utilization) {
            if cpu >= 1.0 {
                events.push(MonitorEvent {
                    kind: EventKind::CpuSpike,
                    observed_at: at,
                    subject: Some(process.identity.clone()),
                    detail: format!(
                        "process CPU utilization reached {:.1}% of one core",
                        cpu * 100.0
                    ),
                    source: cpu_source(current),
                });
            }
        }
        if let Some(old) = previous_process(previous, &process.identity) {
            let old_rss = old.memory.as_ref().and_then(|memory| memory.resident_bytes);
            let rss = process
                .memory
                .as_ref()
                .and_then(|memory| memory.resident_bytes);
            if let (Some(old_rss), Some(rss)) = (old_rss, rss) {
                let growth = rss.saturating_sub(old_rss);
                if growth >= 16 * 1024 * 1024 && growth.saturating_mul(4) >= old_rss {
                    events.push(MonitorEvent {
                        kind: EventKind::MemoryGrowth,
                        observed_at: at,
                        subject: Some(process.identity.clone()),
                        detail: format!("resident memory grew by {}", format_bytes(growth)),
                        source: crate::ObservationSource::ProcStatus,
                    });
                }
            }
        }
        let io_rate = process.io.as_ref().and_then(|io| {
            match (io.read_rate_bytes_per_sec, io.written_rate_bytes_per_sec) {
                (Some(read), Some(write)) => Some(read.max(write)),
                (Some(read), None) | (None, Some(read)) => Some(read),
                (None, None) => None,
            }
        });
        if io_rate.is_some_and(|value| value >= 10.0 * 1024.0 * 1024.0) {
            events.push(MonitorEvent {
                kind: EventKind::IoBurst,
                observed_at: at,
                subject: Some(process.identity.clone()),
                detail: "process I/O exceeded 10 MiB/s".into(),
                source: crate::ObservationSource::ProcIo,
            });
        }
    }
    events
}

fn cpu_source(snapshot: &SystemSnapshot) -> crate::ObservationSource {
    snapshot
        .host_cpu
        .as_ref()
        .map_or(crate::ObservationSource::ProcStat, |cpu| cpu.source)
}

fn history_sample(snapshot: &SystemSnapshot) -> HistorySample {
    let at = snapshot.sample_time.unwrap_or(ObservationTime {
        wall_time: snapshot.observed_at.unwrap_or(std::time::UNIX_EPOCH),
        monotonic_ns: 0,
    });
    HistorySample {
        observed_at: at,
        host_cpu_utilization: snapshot.host_cpu.as_ref().and_then(|cpu| cpu.utilization),
        memory_used_bytes: snapshot.memory_used_bytes,
        processes: snapshot
            .processes
            .iter()
            .take(HISTORY_PROCESS_CAPACITY)
            .map(|process| ProcessHistoryPoint {
                identity: process.identity.clone(),
                observed_at: at,
                cpu_utilization: process.cpu.as_ref().and_then(|cpu| cpu.utilization),
                resident_bytes: process
                    .memory
                    .as_ref()
                    .and_then(|memory| memory.resident_bytes),
                read_rate_bytes_per_sec: process
                    .io
                    .as_ref()
                    .and_then(|io| io.read_rate_bytes_per_sec),
                written_rate_bytes_per_sec: process
                    .io
                    .as_ref()
                    .and_then(|io| io.written_rate_bytes_per_sec),
            })
            .collect(),
    }
}

fn format_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = value as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CpuSample, ObservationSource, ProcessState};

    fn time(ns: u64) -> ObservationTime {
        ObservationTime {
            wall_time: std::time::UNIX_EPOCH + Duration::from_nanos(ns),
            monotonic_ns: ns,
        }
    }

    fn process(pid: u32, start: Option<u64>, ticks: u64) -> Process {
        Process {
            identity: ProcessIdentity::new(pid, start),
            parent: None,
            executable: "fixture".into(),
            command_line: None,
            uid: None,
            user: None,
            state: ProcessState::Running,
            threads: Some(1),
            cpu: Some(CpuSample {
                interval: None,
                utilization: None,
                total_ticks: None,
                busy_ticks: None,
                process_ticks: Some(ticks),
                observed_at: time(0),
                source: ObservationSource::ProcStat,
                status: ObservationStatus::Observed,
            }),
            memory: None,
            memory_status: ObservationStatus::Unavailable,
            io: None,
            io_status: ObservationStatus::Unavailable,
        }
    }

    #[test]
    fn pid_reuse_is_identity_changed_not_same() {
        let old = vec![process(7, Some(10), 100)];
        let new = vec![process(7, Some(20), 1)];
        let transitions = reconcile_processes(Some(&old), &new);
        assert_eq!(transitions[0].lifecycle, ProcessLifecycle::IdentityChanged);
    }

    #[test]
    fn missing_start_markers_are_ambiguous() {
        let old = vec![process(7, None, 100)];
        let new = vec![process(7, None, 101)];
        let transitions = reconcile_processes(Some(&old), &new);
        assert_eq!(
            transitions[0].lifecycle,
            ProcessLifecycle::IdentityAmbiguous
        );
    }

    #[test]
    fn history_and_rates_are_bounded_and_need_two_samples() {
        let mut sampler = Sampler::new(2);
        let mut first = SystemSnapshot {
            sample_time: Some(time(1_000_000_000)),
            host_cpu: Some(CpuSample {
                interval: None,
                utilization: None,
                total_ticks: Some(100),
                busy_ticks: Some(50),
                process_ticks: None,
                observed_at: time(1_000_000_000),
                source: ObservationSource::ProcStat,
                status: ObservationStatus::Observed,
            }),
            processes: vec![process(7, Some(1), 10)],
            cpu_count: Some(1),
            ..SystemSnapshot::default()
        };
        first.observed_at = Some(time(1_000_000_000).wall_time);
        let first = sampler.accept(first).snapshot;
        assert_eq!(first.history.len(), 1);
        assert!(first.processes[0]
            .cpu
            .as_ref()
            .unwrap()
            .utilization
            .is_none());

        let mut second = SystemSnapshot {
            sample_time: Some(time(2_000_000_000)),
            host_cpu: Some(CpuSample {
                interval: None,
                utilization: None,
                total_ticks: Some(200),
                busy_ticks: Some(100),
                process_ticks: None,
                observed_at: time(2_000_000_000),
                source: ObservationSource::ProcStat,
                status: ObservationStatus::Observed,
            }),
            processes: vec![process(7, Some(1), 30)],
            cpu_count: Some(1),
            ..SystemSnapshot::default()
        };
        second.observed_at = Some(time(2_000_000_000).wall_time);
        let second = sampler.accept(second).snapshot;
        assert_eq!(second.history.len(), 2);
        assert!(
            second.processes[0]
                .cpu
                .as_ref()
                .unwrap()
                .utilization
                .unwrap()
                > 0.0
        );

        let mut third = second.clone();
        third.sample_time = Some(time(3_000_000_000));
        let third = sampler.accept(third).snapshot;
        assert_eq!(third.history.len(), 2);
    }
}
