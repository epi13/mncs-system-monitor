//! Monitor-owned semantic subjects and observation vocabulary.
//!
//! This crate deliberately does not model terminal layout, generic language semantics, or raw
//! operating-system files. Those responsibilities belong to `mncs-tui`, `mncs-language`, and the
//! host-collector boundary respectively.

mod model;

pub use model::{
    CpuSample, IoSample, MemorySample, NetworkSample, Observation, ObservationSource,
    ObservationSubject, PressureKind, PressureLevel, Process, ProcessId, ProcessIdentity,
    ProcessState, ResourcePressure, SystemSnapshot,
};
