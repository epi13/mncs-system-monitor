//! Monitor-owned semantic subjects and observation vocabulary.
//!
//! This crate deliberately does not model terminal layout, generic language semantics, or raw
//! operating-system files. Those responsibilities belong to `mncs-tui`, `mncs-language`, and the
//! host-collector boundary respectively.

mod model;
mod sampling;

pub use model::{
    CollectionIssue, CpuCounters, CpuSample, DiskSample, EventKind, HistorySample, IoSample,
    MemorySample, MonitorEvent, NetworkSample, Observation, ObservationSource, ObservationStatus,
    ObservationSubject, ObservationTime, PressureKind, PressureLevel, Process, ProcessHistoryPoint,
    ProcessId, ProcessIdentity, ProcessLifecycle, ProcessRelationship, ProcessState,
    ProcessTransition, RelationshipStatus, ResourcePressure, SystemSnapshot,
};
pub use sampling::{reconcile_processes, AcceptedSnapshot, Sampler, DEFAULT_HISTORY_CAPACITY};
