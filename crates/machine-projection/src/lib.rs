//! Machine-facing projection over the shared monitor semantic state.
//!
//! The projection intentionally borrows `SystemSnapshot` instead of defining a second process or
//! resource ontology. Serialization and transport are future adapters around this stable seam.

use mncs_monitor_core::SystemSnapshot;

pub const SNAPSHOT_SCHEMA: &str = "mncs.system-monitor.snapshot.v0";

/// A machine-facing envelope that preserves the monitor model and its uncertainty.
#[derive(Debug)]
pub struct MachineView<'a> {
    pub schema: &'static str,
    pub snapshot: &'a SystemSnapshot,
}

impl MachineView<'_> {
    pub fn process_count(&self) -> usize {
        self.snapshot.process_count()
    }
}

/// Projection contract used by a future local API, agent protocol, or export format.
pub trait MachineProjection {
    fn project<'a>(&self, snapshot: &'a SystemSnapshot) -> MachineView<'a>;
}

#[derive(Debug, Default)]
pub struct StructuredProjection;

impl MachineProjection for StructuredProjection {
    fn project<'a>(&self, snapshot: &'a SystemSnapshot) -> MachineView<'a> {
        MachineView {
            schema: SNAPSHOT_SCHEMA,
            snapshot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_the_shared_snapshot_identity() {
        let snapshot = SystemSnapshot::default();
        let view = StructuredProjection.project(&snapshot);

        assert_eq!(view.schema, SNAPSHOT_SCHEMA);
        assert_eq!(view.process_count(), 0);
        assert!(std::ptr::eq(view.snapshot, &snapshot));
    }
}
