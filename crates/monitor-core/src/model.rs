use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// A process identifier paired with a host-provided start marker to avoid PID-reuse ambiguity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProcessIdentity {
    pub pid: ProcessId,
    pub start_marker: Option<u64>,
}

/// The host's process identifier namespace.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProcessId(pub u32);

/// Monitor-level process state. `Unknown` is intentional when the adapter cannot establish a
/// stronger state from its source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Running,
    Sleeping,
    Stopped,
    Zombie,
    Dead,
    Unknown,
}

/// Why a value is absent or should not be treated as a normal measurement.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum ObservationStatus {
    Observed,
    #[default]
    Unknown,
    Unavailable,
    Unsupported,
    PermissionDenied,
    Malformed,
    Disappeared,
    Stale,
    CounterRegression,
    InvalidInterval,
}

impl ObservationStatus {
    pub fn is_observed(self) -> bool {
        self == Self::Observed
    }
}

/// A reading's clock values. The monotonic value is relative to the collector's clock origin;
/// it is suitable for interval validation but is not a wall-clock timestamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationTime {
    pub wall_time: SystemTime,
    pub monotonic_ns: u64,
}

/// Linux CPU counters normalized before they enter the monitor model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpuCounters {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuCounters {
    pub fn total(self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    pub fn busy(self) -> u64 {
        self.total()
            .saturating_sub(self.idle.saturating_add(self.iowait))
    }
}

/// A CPU observation. Counters are retained so consumers can distinguish a measured rate from a
/// display-only percentage. Process utilization can exceed 1.0 when it is normalized to one core.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuSample {
    pub interval: Option<Duration>,
    pub utilization: Option<f64>,
    pub total_ticks: Option<u64>,
    pub busy_ticks: Option<u64>,
    pub process_ticks: Option<u64>,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
}

/// A memory observation using bytes as the canonical unit at the monitor boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemorySample {
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
}

/// Process or host I/O observations. Missing fields remain missing; rates are only populated by
/// the sampling layer after a valid, identity-continuous pair exists.
#[derive(Clone, Debug, PartialEq)]
pub struct IoSample {
    pub read_bytes: Option<u64>,
    pub written_bytes: Option<u64>,
    pub read_rate_bytes_per_sec: Option<f64>,
    pub written_rate_bytes_per_sec: Option<f64>,
    pub interval: Option<Duration>,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
}

/// A block device counter and any interval-qualified rates derived from it.
#[derive(Clone, Debug, PartialEq)]
pub struct DiskSample {
    pub device: String,
    pub read_bytes: Option<u64>,
    pub written_bytes: Option<u64>,
    pub read_rate_bytes_per_sec: Option<f64>,
    pub written_rate_bytes_per_sec: Option<f64>,
    pub interval: Option<Duration>,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
}

/// Host network counters and interval-qualified throughput.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkSample {
    pub interface: Option<String>,
    pub received_bytes: Option<u64>,
    pub transmitted_bytes: Option<u64>,
    pub received_rate_bytes_per_sec: Option<f64>,
    pub transmitted_rate_bytes_per_sec: Option<f64>,
    pub interval: Option<Duration>,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
}

/// Provenance for facts entering the semantic model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationSource {
    ProcStat,
    ProcStatus,
    ProcIo,
    ProcPressure,
    Sysfs,
    Cgroup,
    Systemd,
    HostApi,
    Synthetic,
    Unavailable,
}

/// The subject to which an observation applies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationSubject {
    Host,
    Process(ProcessIdentity),
    Interface(String),
    Service(String),
    Cgroup(String),
}

/// A value with timing, subject, and provenance retained as first-class data.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation<T> {
    pub subject: ObservationSubject,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
    pub value: Option<T>,
}

/// Resource pressure category reported by a host adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressureKind {
    Cpu,
    Memory,
    Io,
    Network,
    FileDescriptors,
    Unknown,
}

/// Qualitative pressure level. It is not a universal OS severity scale.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PressureLevel {
    None,
    Some,
    Full,
    Unknown,
}

/// A pressure signal with an explicit time window and source.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourcePressure {
    pub kind: PressureKind,
    pub level: PressureLevel,
    pub some_avg10: Option<f64>,
    pub some_avg60: Option<f64>,
    pub some_avg300: Option<f64>,
    pub full_avg10: Option<f64>,
    pub full_avg60: Option<f64>,
    pub full_avg300: Option<f64>,
    pub window: Option<Duration>,
    pub observed_at: ObservationTime,
    pub source: ObservationSource,
    pub status: ObservationStatus,
}

/// An issue recorded while a partial host snapshot was acquired. A process disappearing here is
/// deliberately not promoted to a clean-exit claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionIssue {
    pub subject: Option<ProcessId>,
    pub status: ObservationStatus,
    pub source: ObservationSource,
    pub detail: String,
}

/// The monitor-owned relationship status between a process and its observed parent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationshipStatus {
    Resolved,
    ParentNotObserved,
    Ambiguous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessRelationship {
    pub child: ProcessIdentity,
    pub parent: Option<ProcessIdentity>,
    pub status: RelationshipStatus,
}

/// Bounded lifecycle classification between two accepted snapshots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLifecycle {
    Same,
    New,
    NoLongerObserved,
    IdentityChanged,
    IdentityAmbiguous,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessTransition {
    pub pid: ProcessId,
    pub previous: Option<ProcessIdentity>,
    pub current: Option<ProcessIdentity>,
    pub lifecycle: ProcessLifecycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    ProcessObserved,
    ProcessNoLongerObserved,
    ProcessIdentityChanged,
    CpuSpike,
    MemoryGrowth,
    IoBurst,
    SamplingGap,
    CollectorFailure,
    PermissionLimited,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorEvent {
    pub kind: EventKind,
    pub observed_at: ObservationTime,
    pub subject: Option<ProcessIdentity>,
    pub detail: String,
    pub source: ObservationSource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessHistoryPoint {
    pub identity: ProcessIdentity,
    pub observed_at: ObservationTime,
    pub cpu_utilization: Option<f64>,
    pub resident_bytes: Option<u64>,
    pub read_rate_bytes_per_sec: Option<f64>,
    pub written_rate_bytes_per_sec: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistorySample {
    pub observed_at: ObservationTime,
    pub host_cpu_utilization: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub processes: Vec<ProcessHistoryPoint>,
}

/// A process subject. OS-specific fields are translated by a collector before they enter this
/// model. `identity` is always the PID plus whatever start evidence was available.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub identity: ProcessIdentity,
    pub parent: Option<ProcessId>,
    pub executable: String,
    pub command_line: Option<String>,
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub state: ProcessState,
    pub threads: Option<u32>,
    pub cpu: Option<CpuSample>,
    pub memory: Option<MemorySample>,
    pub memory_status: ObservationStatus,
    pub io: Option<IoSample>,
    pub io_status: ObservationStatus,
}

/// A point-in-time semantic view shared by human and machine projections. `history` and `events`
/// are bounded by the sampler, never by the collector's process count.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemSnapshot {
    pub observed_at: Option<SystemTime>,
    pub sample_time: Option<ObservationTime>,
    pub hostname: Option<String>,
    pub processes: Vec<Process>,
    pub relationships: Vec<ProcessRelationship>,
    pub pressure: Vec<ResourcePressure>,
    pub network: Vec<NetworkSample>,
    pub disk: Vec<DiskSample>,
    pub host_cpu: Option<CpuSample>,
    pub cpu_count: Option<u32>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub memory_free_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub swap_free_bytes: Option<u64>,
    pub swap_used_bytes: Option<u64>,
    pub load_average: Option<[f64; 3]>,
    pub uptime: Option<Duration>,
    pub issues: Vec<CollectionIssue>,
    pub transitions: Vec<ProcessTransition>,
    pub events: Vec<MonitorEvent>,
    pub history: VecDeque<HistorySample>,
}

impl ProcessIdentity {
    pub fn new(pid: u32, start_marker: Option<u64>) -> Self {
        Self {
            pid: ProcessId(pid),
            start_marker,
        }
    }
}

impl SystemSnapshot {
    /// Number of process subjects present in this observation.
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    pub fn process(&self, identity: &ProcessIdentity) -> Option<&Process> {
        self.processes
            .iter()
            .find(|process| &process.identity == identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_identity_retains_pid_reuse_marker() {
        let first = ProcessIdentity::new(42, Some(100));
        let second = ProcessIdentity::new(42, Some(200));

        assert_ne!(first, second);
    }

    #[test]
    fn snapshot_count_is_projection_neutral() {
        let snapshot = SystemSnapshot {
            processes: vec![Process {
                identity: ProcessIdentity::new(7, None),
                parent: None,
                executable: "example".into(),
                command_line: None,
                uid: None,
                user: None,
                state: ProcessState::Unknown,
                threads: None,
                cpu: None,
                memory: None,
                memory_status: ObservationStatus::Unknown,
                io: None,
                io_status: ObservationStatus::Unknown,
            }],
            ..SystemSnapshot::default()
        };

        assert_eq!(snapshot.process_count(), 1);
    }
}
