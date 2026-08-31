use mncs_monitor_core::{ObservationSource, SystemSnapshot};

use crate::{CollectError, Collector};

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
        Err(CollectError::Unsupported {
            platform: std::env::consts::OS.into(),
        })
    }
}
