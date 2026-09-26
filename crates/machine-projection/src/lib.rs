//! Machine-facing projection over the shared monitor semantic state.
//!
//! JSON is only a transport encoding. The semantic source remains `SystemSnapshot`, and unknown
//! or permission-limited values are emitted as structured status-bearing fields rather than being
//! replaced by zeroes or strings scraped from the terminal.

use mncs_monitor_core::{
    CpuSample, DiskSample, IoSample, MemorySample, NetworkSample, ObservationSource,
    ObservationStatus, ObservationTime, Process, ProcessHistoryPoint, ProcessIdentity,
    ProcessState, SystemSnapshot,
};
use serde_json::{json, Value};

pub const SNAPSHOT_SCHEMA: &str = "mncs.system-monitor.snapshot.v1";

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

    pub fn top_processes_by_cpu(&self, limit: usize) -> Vec<&Process> {
        let mut processes: Vec<&Process> = self.snapshot.processes.iter().collect();
        processes.sort_by(|left, right| {
            right
                .cpu
                .as_ref()
                .and_then(|cpu| cpu.utilization)
                .partial_cmp(&left.cpu.as_ref().and_then(|cpu| cpu.utilization))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        processes.truncate(limit);
        processes
    }

    pub fn top_processes_by_rss(&self, limit: usize) -> Vec<&Process> {
        let mut processes: Vec<&Process> = self.snapshot.processes.iter().collect();
        processes.sort_by_key(|process| {
            std::cmp::Reverse(
                process
                    .memory
                    .as_ref()
                    .and_then(|memory| memory.resident_bytes)
                    .unwrap_or(0),
            )
        });
        processes.truncate(limit);
        processes
    }

    pub fn history_for(&self, identity: &ProcessIdentity) -> Vec<&ProcessHistoryPoint> {
        self.snapshot
            .history
            .iter()
            .flat_map(|sample| sample.processes.iter())
            .filter(|point| &point.identity == identity)
            .collect()
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&snapshot_value(self.schema, self.snapshot))
    }

    /// Emit the snapshot envelope with extra top-level sections merged in. Sections are
    /// caller-supplied projections (for example execution correlation); the snapshot fields
    /// themselves are never altered by a section.
    pub fn to_json_with(&self, sections: &[(&str, Value)]) -> Result<String, serde_json::Error> {
        let mut value = snapshot_value(self.schema, self.snapshot);
        if let Value::Object(ref mut map) = value {
            for (name, section) in sections {
                map.insert((*name).to_string(), section.clone());
            }
        }
        serde_json::to_string_pretty(&value)
    }
}

/// Projection contract used by a local API, agent protocol, or export format.
pub trait MachineProjection {
    fn project<'a>(&self, snapshot: &'a SystemSnapshot) -> MachineView<'a>;
}

#[derive(Debug, Default)]
pub struct StructuredProjection;

impl StructuredProjection {
    pub fn json(&self, snapshot: &SystemSnapshot) -> Result<String, serde_json::Error> {
        self.project(snapshot).to_json()
    }

    pub fn json_with(
        &self,
        snapshot: &SystemSnapshot,
        sections: &[(&str, Value)],
    ) -> Result<String, serde_json::Error> {
        self.project(snapshot).to_json_with(sections)
    }
}

impl MachineProjection for StructuredProjection {
    fn project<'a>(&self, snapshot: &'a SystemSnapshot) -> MachineView<'a> {
        MachineView {
            schema: SNAPSHOT_SCHEMA,
            snapshot,
        }
    }
}

fn status_label(status: ObservationStatus) -> &'static str {
    match status {
        ObservationStatus::Observed => "observed",
        ObservationStatus::Unknown => "unknown",
        ObservationStatus::Unavailable => "unavailable",
        ObservationStatus::Unsupported => "unsupported",
        ObservationStatus::PermissionDenied => "permission_denied",
        ObservationStatus::Malformed => "malformed",
        ObservationStatus::Disappeared => "disappeared",
        ObservationStatus::Stale => "stale",
        ObservationStatus::CounterRegression => "counter_regression",
        ObservationStatus::InvalidInterval => "invalid_interval",
    }
}

fn source_label(source: ObservationSource) -> &'static str {
    match source {
        ObservationSource::ProcStat => "proc_stat",
        ObservationSource::ProcStatus => "proc_status",
        ObservationSource::ProcIo => "proc_io",
        ObservationSource::ProcPressure => "proc_pressure",
        ObservationSource::Sysfs => "sysfs",
        ObservationSource::Cgroup => "cgroup",
        ObservationSource::Systemd => "systemd",
        ObservationSource::HostApi => "host_api",
        ObservationSource::Synthetic => "synthetic",
        ObservationSource::Unavailable => "unavailable",
    }
}

fn time_value(time: ObservationTime) -> Value {
    let wall_ms = time
        .wall_time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .ok();
    json!({ "wall_time_unix_ms": wall_ms, "monotonic_ns": time.monotonic_ns })
}

fn status_value(status: ObservationStatus, source: ObservationSource) -> Value {
    json!({ "status": status_label(status), "source": source_label(source) })
}

fn interval_ms(interval: Option<std::time::Duration>) -> Option<u64> {
    interval.map(|value| value.as_millis() as u64)
}

fn cpu_value(cpu: &CpuSample) -> Value {
    let mut value = status_value(cpu.status, cpu.source);
    value["interval_ms"] = json!(interval_ms(cpu.interval));
    value["utilization"] = json!(cpu.utilization);
    value["total_ticks"] = json!(cpu.total_ticks);
    value["busy_ticks"] = json!(cpu.busy_ticks);
    value["process_ticks"] = json!(cpu.process_ticks);
    value["observed_at"] = time_value(cpu.observed_at);
    value
}

fn memory_value(memory: &MemorySample) -> Value {
    let mut value = status_value(memory.status, memory.source);
    value["resident_bytes"] = json!(memory.resident_bytes);
    value["virtual_bytes"] = json!(memory.virtual_bytes);
    value["observed_at"] = time_value(memory.observed_at);
    value
}

fn io_value(io: &IoSample) -> Value {
    let mut value = status_value(io.status, io.source);
    value["read_bytes"] = json!(io.read_bytes);
    value["written_bytes"] = json!(io.written_bytes);
    value["read_rate_bytes_per_sec"] = json!(io.read_rate_bytes_per_sec);
    value["written_rate_bytes_per_sec"] = json!(io.written_rate_bytes_per_sec);
    value["interval_ms"] = json!(interval_ms(io.interval));
    value["observed_at"] = time_value(io.observed_at);
    value
}

fn disk_value(disk: &DiskSample) -> Value {
    let mut value = status_value(disk.status, disk.source);
    value["device"] = json!(disk.device);
    value["read_bytes"] = json!(disk.read_bytes);
    value["written_bytes"] = json!(disk.written_bytes);
    value["read_rate_bytes_per_sec"] = json!(disk.read_rate_bytes_per_sec);
    value["written_rate_bytes_per_sec"] = json!(disk.written_rate_bytes_per_sec);
    value["interval_ms"] = json!(interval_ms(disk.interval));
    value["observed_at"] = time_value(disk.observed_at);
    value
}

fn network_value(network: &NetworkSample) -> Value {
    let mut value = status_value(network.status, network.source);
    value["interface"] = json!(network.interface);
    value["received_bytes"] = json!(network.received_bytes);
    value["transmitted_bytes"] = json!(network.transmitted_bytes);
    value["received_rate_bytes_per_sec"] = json!(network.received_rate_bytes_per_sec);
    value["transmitted_rate_bytes_per_sec"] = json!(network.transmitted_rate_bytes_per_sec);
    value["interval_ms"] = json!(interval_ms(network.interval));
    value["observed_at"] = time_value(network.observed_at);
    value
}

fn identity_value(identity: &ProcessIdentity) -> Value {
    json!({ "pid": identity.pid.0, "start_marker": identity.start_marker })
}

fn state_label(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Running => "running",
        ProcessState::Sleeping => "sleeping",
        ProcessState::Stopped => "stopped",
        ProcessState::Zombie => "zombie",
        ProcessState::Dead => "dead",
        ProcessState::Unknown => "unknown",
    }
}

fn process_value(process: &Process) -> Value {
    json!({
        "identity": identity_value(&process.identity),
        "parent_pid": process.parent.map(|pid| pid.0),
        "executable": process.executable,
        "command_line": process.command_line,
        "uid": process.uid,
        "user": process.user,
        "state": state_label(process.state),
        "threads": process.threads,
        "cpu": process.cpu.as_ref().map(cpu_value),
        "memory": process.memory.as_ref().map(memory_value),
        "memory_status": status_label(process.memory_status),
        "io": process.io.as_ref().map(io_value),
        "io_status": status_label(process.io_status),
    })
}

fn snapshot_value(schema: &str, snapshot: &SystemSnapshot) -> Value {
    let sample_time = snapshot.sample_time.map(time_value);
    json!({
        "schema": schema,
        "observed_at": snapshot.observed_at.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|time| time.as_millis() as u64),
        "sample_time": sample_time,
        "hostname": snapshot.hostname,
        "host": {
            "cpu_count": snapshot.cpu_count,
            "cpu": snapshot.host_cpu.as_ref().map(cpu_value),
            "memory_total_bytes": snapshot.memory_total_bytes,
            "memory_available_bytes": snapshot.memory_available_bytes,
            "memory_free_bytes": snapshot.memory_free_bytes,
            "memory_used_bytes": snapshot.memory_used_bytes,
            "swap_total_bytes": snapshot.swap_total_bytes,
            "swap_free_bytes": snapshot.swap_free_bytes,
            "swap_used_bytes": snapshot.swap_used_bytes,
            "load_average": snapshot.load_average,
            "uptime_seconds": snapshot.uptime.map(|value| value.as_secs_f64()),
        },
        "process_count": snapshot.process_count(),
        "processes": snapshot.processes.iter().map(process_value).collect::<Vec<_>>(),
        "relationships": snapshot.relationships.iter().map(|relationship| json!({
            "child": identity_value(&relationship.child),
            "parent": relationship.parent.as_ref().map(identity_value),
            "status": format!("{:?}", relationship.status).to_lowercase(),
        })).collect::<Vec<_>>(),
        "network": snapshot.network.iter().map(network_value).collect::<Vec<_>>(),
        "disk": snapshot.disk.iter().map(disk_value).collect::<Vec<_>>(),
        "pressure": snapshot.pressure.iter().map(|pressure| json!({
            "kind": format!("{:?}", pressure.kind).to_lowercase(),
            "level": format!("{:?}", pressure.level).to_lowercase(),
            "some_avg10": pressure.some_avg10,
            "some_avg60": pressure.some_avg60,
            "some_avg300": pressure.some_avg300,
            "full_avg10": pressure.full_avg10,
            "full_avg60": pressure.full_avg60,
            "full_avg300": pressure.full_avg300,
            "window_ms": interval_ms(pressure.window),
            "observed_at": time_value(pressure.observed_at),
            "status": status_label(pressure.status),
            "source": source_label(pressure.source),
        })).collect::<Vec<_>>(),
        "issues": snapshot.issues.iter().map(|issue| json!({
            "pid": issue.subject.map(|pid| pid.0),
            "status": status_label(issue.status),
            "source": source_label(issue.source),
            "detail": issue.detail,
        })).collect::<Vec<_>>(),
        "transitions": snapshot.transitions.iter().map(|transition| json!({
            "pid": transition.pid.0,
            "previous": transition.previous.as_ref().map(identity_value),
            "current": transition.current.as_ref().map(identity_value),
            "lifecycle": format!("{:?}", transition.lifecycle).to_lowercase(),
        })).collect::<Vec<_>>(),
        "events": snapshot.events.iter().map(|event| json!({
            "kind": format!("{:?}", event.kind).to_lowercase(),
            "observed_at": time_value(event.observed_at),
            "subject": event.subject.as_ref().map(identity_value),
            "detail": event.detail,
            "source": source_label(event.source),
        })).collect::<Vec<_>>(),
        "history": snapshot.history.iter().map(|sample| json!({
            "observed_at": time_value(sample.observed_at),
            "host_cpu_utilization": sample.host_cpu_utilization,
            "memory_used_bytes": sample.memory_used_bytes,
            "processes": sample.processes.iter().map(history_point_value).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn history_point_value(point: &ProcessHistoryPoint) -> Value {
    json!({
        "identity": identity_value(&point.identity),
        "observed_at": time_value(point.observed_at),
        "cpu_utilization": point.cpu_utilization,
        "resident_bytes": point.resident_bytes,
        "read_rate_bytes_per_sec": point.read_rate_bytes_per_sec,
        "written_rate_bytes_per_sec": point.written_rate_bytes_per_sec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mncs_monitor_core::SystemSnapshot;

    #[test]
    fn projection_keeps_the_shared_snapshot_identity() {
        let snapshot = SystemSnapshot::default();
        let view = StructuredProjection.project(&snapshot);

        assert_eq!(view.schema, SNAPSHOT_SCHEMA);
        assert_eq!(view.process_count(), 0);
        assert!(std::ptr::eq(view.snapshot, &snapshot));
    }

    #[test]
    fn json_keeps_unknowns_as_structured_null_and_status() {
        let snapshot = SystemSnapshot::default();
        let json = StructuredProjection.json(&snapshot).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema"], SNAPSHOT_SCHEMA);
        assert!(value["host"]["cpu"].is_null());
        assert_eq!(value["process_count"], 0);
    }
}
