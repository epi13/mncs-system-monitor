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

/// The monitor's normalized process subject. OS-specific fields are translated by a collector
/// before they enter this model.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub identity: ProcessIdentity,
    pub parent: Option<ProcessId>,
    pub executable: String,
    pub user: Option<String>,
    pub state: ProcessState,
    pub threads: Option<u32>,
    pub cpu: Option<CpuSample>,
    pub memory: Option<MemorySample>,
    pub io: Option<IoSample>,
}

/// A CPU observation over an explicit interval. Utilization is a ratio in the inclusive range
/// `0.0..=1.0` when the source can establish it.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuSample {
    pub interval: Duration,
    pub utilization: Option<f64>,
    pub observed_at: SystemTime,
    pub source: ObservationSource,
}

/// A memory observation using bytes as the canonical unit at the monitor boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemorySample {
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    pub observed_at: SystemTime,
    pub source: ObservationSource,
}

/// Process or host I/O observations. A missing field means the source did not establish it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IoSample {
    pub read_bytes: Option<u64>,
    pub written_bytes: Option<u64>,
    pub interval: Option<Duration>,
    pub observed_at: SystemTime,
    pub source: ObservationSource,
}

/// Host network throughput observation. Interface identity is retained when available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkSample {
    pub interface: Option<String>,
    pub received_bytes: Option<u64>,
    pub transmitted_bytes: Option<u64>,
    pub interval: Option<Duration>,
    pub observed_at: SystemTime,
    pub source: ObservationSource,
}

/// Provenance for facts entering the semantic model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationSource {
    ProcStat,
    ProcStatus,
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
    pub observed_at: SystemTime,
    pub source: ObservationSource,
    pub value: T,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePressure {
    pub kind: PressureKind,
    pub level: PressureLevel,
    pub window: Option<Duration>,
    pub observed_at: SystemTime,
    pub source: ObservationSource,
}

/// A point-in-time semantic view shared by human and machine projections.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemSnapshot {
    pub observed_at: Option<SystemTime>,
    pub processes: Vec<Process>,
    pub pressure: Vec<ResourcePressure>,
    pub network: Vec<NetworkSample>,
    pub cpu_count: Option<u32>,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub load_average: Option<[f64; 3]>,
    pub uptime: Option<Duration>,
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
                user: None,
                state: ProcessState::Unknown,
                threads: None,
                cpu: None,
                memory: None,
                io: None,
            }],
            ..SystemSnapshot::default()
        };

        assert_eq!(snapshot.process_count(), 1);
    }
}
