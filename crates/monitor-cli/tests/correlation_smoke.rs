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

fn forge_receipt() -> String {
    format!(
        r#"{{
            "record_type": "mncs-execution-receipt",
            "schema_version": "0.1-experimental",
            "receipt_identity": "forge:smoke:resource-1",
            "lifecycle": {{"termination_category": "signal"}},
            "process": {{"harness_status": "PASS", "exit_code": null, "signal": 9}},
            "extensions": {{"forge:local-process": {{
                "termination_error_code": null,
                "resource_envelope": {{"memory_max_bytes": 8192, "tasks_max": 8}},
                "resource_observations": {{"resource_observations": {{
                    "host_pid": {ABSENT_PID},
                    "host_start_marker": 991,
                    "cgroup_memory_peak_bytes": 7000,
                    "cpu_time_microseconds": 250
                }}}}
            }}}}
        }}"#
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

fn private_artifact_dir(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mncs-monitor-artifacts-{name}-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&path).ok();
    std::fs::create_dir(&path).expect("artifact directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("private artifact permissions");
    }
    path
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
fn json_ingests_forge_receipt_and_keeps_operational_facts_separate() {
    let fixture = write_fixture("forge-receipt", &forge_receipt());
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
    assert!(
        output.status.success(),
        "valid Forge receipt is ingested: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(
        envelope["correlation"]["links"][0]["linkage"],
        "process_absent"
    );
    let record = &envelope["execution_records"]["records"][0];
    assert_eq!(record["execution"], "forge:smoke:resource-1");
    assert_eq!(record["source"], "forge_receipt");
    assert_eq!(record["harness_status"], "PASS");
    assert_eq!(record["verification_outcome"]["status"], "unknown");
    assert_eq!(record["termination"]["category"], "signal");
    assert_eq!(record["termination"]["process_signal"], 9);
    assert_eq!(record["process_identity"]["host_start_marker"], 991);
    assert_eq!(record["resource_envelope"]["process_count_max"], 8);
    assert_eq!(
        record["resource_observations"]["cgroup_memory_peak_bytes"],
        7000
    );
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

#[test]
fn environment_invocation_publishes_a_minimized_resource_sample() {
    let receipt = write_fixture("resource-sample-receipt", &forge_receipt());
    let artifact_dir = private_artifact_dir("resource-sample");
    let output = Command::new(monitor_bin())
        .args([
            "--resource-sample",
            "--executions",
            receipt.to_str().expect("receipt path"),
        ])
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(
        output.status.success(),
        "monitor succeeds: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact = std::fs::read(artifact_dir.join("system-resource-sample.json"))
        .expect("Environment resource sample artifact");
    assert_eq!(
        artifact, output.stdout,
        "retained bytes match provider output"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&artifact).expect("JSON envelope");
    assert_eq!(envelope["schema"], "mncs.system-monitor.resource-sample.v1");
    assert!(envelope.get("hostname").is_none());
    assert!(envelope.get("processes").is_none());
    assert_eq!(
        envelope["execution_records"]["records"][0]["source"],
        "forge_receipt"
    );
    assert_eq!(
        envelope["execution_records"]["records"][0]["resource_observations"]
            ["cgroup_memory_peak_bytes"],
        7000
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_file(receipt).ok();
}

#[cfg(unix)]
#[test]
fn environment_invocation_does_not_follow_resource_artifact_symlinks() {
    use std::os::unix::fs::symlink;

    let artifact_dir = private_artifact_dir("symlink");
    let outside = artifact_dir.with_extension("outside");
    std::fs::write(&outside, b"preserve").expect("outside fixture");
    symlink(&outside, artifact_dir.join("system-resource-sample.json")).expect("artifact symlink");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "symlink destination is rejected");
    assert_eq!(
        std::fs::read(&outside).expect("outside fixture remains"),
        b"preserve"
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_file(&outside).ok();
}
