use mncs_monitor_core::{ObservationSource, SystemSnapshot};

use crate::{CollectError, Collector};

/// Linux host adapter seam.
///
/// The collector intentionally has no `/proc` implementation in the bootstrap. Adding one is a
/// separate, testable boundary change with explicit parsing, privilege, and freshness behavior.
#[derive(Debug, Default)]
pub struct PlatformCollector;

impl PlatformCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for PlatformCollector {
    fn source(&self) -> ObservationSource {
        ObservationSource::Unavailable
    }

    fn collect(&mut self) -> Result<SystemSnapshot, CollectError> {
        Err(CollectError::Unavailable {
            resource: "Linux host collector (/proc, /sys, cgroups, and systemd)".into(),
        })
    }
}
