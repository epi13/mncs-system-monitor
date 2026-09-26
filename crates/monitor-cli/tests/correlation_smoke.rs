//! Deterministic CLI correlation tests. Fixture PIDs (near u32::MAX) can never
//! exist on a live host, so every expectation holds regardless of machine state.

use std::io::Write;
use std::process::Command;

const ABSENT_PID: u32 = 4_000_000_000;

fn fixture_records() -> String {
    format!(
        r#"[{{
            "schema": "mncs.system-monitor.execution-record.v1",
            "execution_identity": "smoke:stale",
            "repository": "example",
            "host_pid": {ABSENT_PID},
            "status": "active",
            "outcome": "not_finished",
            "source": "manual"
        }}]"#
    )
}

fn write_fixture(name: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("mncs-monitor-{name}-{}", std::process::id()));
    let mut file = std::fs::File::create(&path).expect("fixture file");
    file.write_all(contents.as_bytes()).expect("fixture write");
    path
}

fn monitor_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mncs-system-monitor"))
}

#[test]
fn json_correlation_flags_stale_active() {
    let fixture = write_fixture("records", &fixture_records());
    let output = Command::new(monitor_bin())
        .args([
            "--json",
            "--once",
            "--history",
            "1",
            "--executions",
            fixture.to_str().expect("path"),
        ])
        .output()
        .expect("monitor runs");
    std::fs::remove_file(&fixture).ok();
    assert!(output.status.success(), "exit zero on valid records");
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["schema"], "mncs.system-monitor.snapshot.v1");
    let correlation = &envelope["correlation"];
    assert_eq!(correlation["schema"], "mncs.system-monitor.correlation.v1");
    assert_eq!(correlation["executions"], serde_json::json!(1));
    assert_eq!(correlation["links"][0]["linkage"], "process_absent");
    let kinds: Vec<&str> = correlation["anomaly_details"]
        .as_array()
        .expect("anomalies")
        .iter()
        .map(|anomaly| anomaly["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, vec!["stale_active_execution"]);
}

#[test]
fn reconcile_reports_stale_active() {
    let fixture = write_fixture("reconcile", &fixture_records());
    let output = Command::new(monitor_bin())
        .args(["--reconcile", fixture.to_str().expect("path")])
        .output()
        .expect("monitor runs");
    std::fs::remove_file(&fixture).ok();
    assert!(output.status.success(), "exit zero when report is produced");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["schema"], "mncs.system-monitor.restart.v1");
    assert_eq!(report["items"][0]["execution"], "smoke:stale");
    assert_eq!(report["items"][0]["class"], "stale_active");
}

#[test]
fn malformed_records_are_a_hard_error() {
    let fixture = write_fixture("malformed", "{not json");
    let output = Command::new(monitor_bin())
        .args([
            "--json",
            "--once",
            "--executions",
            fixture.to_str().expect("path"),
        ])
        .output()
        .expect("monitor runs");
    std::fs::remove_file(&fixture).ok();
    assert!(!output.status.success(), "malformed records fail loudly");
}
