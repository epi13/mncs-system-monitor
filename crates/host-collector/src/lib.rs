//! Host-bound collection contracts.
//!
//! This crate is the only workspace member that should acquire OS facts. Platform modules may
//! read `/proc`, `/sys`, cgroups, systemd, or another host API in later phases, then translate
//! those facts into `mncs-monitor-core` subjects. They must not leak raw host parsing into the
//! semantic model or UI crates.

use std::fmt::{Display, Formatter};

use mncs_monitor_core::{ObservationSource, SystemSnapshot};

#[cfg(target_os = "linux")]
mod platform;

#[cfg(target_os = "linux")]
pub use platform::{
    parse_proc_stat_cpu, parse_process_stat, LinuxPaths, ParsedProcessStat, PlatformCollector,
};

#[cfg(not(target_os = "linux"))]
mod unsupported;

#[cfg(not(target_os = "linux"))]
pub use unsupported::PlatformCollector;

/// The smallest host-boundary interface needed by the monitor loop.
pub trait Collector {
    fn source(&self) -> ObservationSource;
    fn collect(&mut self) -> Result<SystemSnapshot, CollectError>;
}

/// Collection failure or uncertainty that should remain visible to callers and projections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CollectError {
    Unsupported { platform: String },
    PermissionDenied { resource: String },
    Unavailable { resource: String },
    Malformed { resource: String },
    Io { resource: String, detail: String },
}

impl Display for CollectError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { platform } => {
                write!(formatter, "collection is unsupported on {platform}")
            }
            Self::PermissionDenied { resource } => {
                write!(formatter, "permission denied while reading {resource}")
            }
            Self::Unavailable { resource } => {
                write!(formatter, "host resource unavailable: {resource}")
            }
            Self::Malformed { resource } => {
                write!(formatter, "host resource is malformed: {resource}")
            }
            Self::Io { resource, detail } => {
                write!(formatter, "I/O error while reading {resource}: {detail}")
            }
        }
    }
}

impl std::error::Error for CollectError {}
