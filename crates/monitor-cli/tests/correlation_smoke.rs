//! Deterministic CLI correlation tests. Fixture PIDs (near u32::MAX) can never
//! exist on a live host, so every expectation holds regardless of machine state.

use sha2::{Digest, Sha256};
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

fn environment_receipt_input(
    name: &str,
    receipt: &str,
    capability: &str,
    digest: Option<&str>,
) -> (std::path::PathBuf, String) {
    let input_dir = private_artifact_dir(name);
    let local_path = input_dir.join("0000.artifact");
    std::fs::write(&local_path, receipt.as_bytes()).expect("Environment input file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&local_path, std::fs::Permissions::from_mode(0o400))
            .expect("read-only Environment input file");
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(&local_path)
            .expect("Environment input metadata")
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&local_path, permissions)
            .expect("read-only Environment input file");
    }
    let actual_digest = Sha256::digest(receipt.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let manifest = serde_json::json!({
        "schema_version": "mncs.environment.provider-input-artifacts/1",
        "session_id": "ses_fixture",
        "artifacts": [{
            "name": "forge-receipt",
            "capability": capability,
            "binding_id": "cap_0123456789abcdef",
            "store_reference": format!(
                "mncs-store:ses_fixture:capability-invocation-artifacts:{}",
                "b".repeat(64),
            ),
            "path": "execution-receipt.json",
            "byte_count": receipt.len(),
            "sha256": digest.unwrap_or(&actual_digest),
            "local_path": "0000.artifact",
        }],
    });
    (input_dir, manifest.to_string())
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

#[test]
fn environment_invocation_reads_verified_forge_receipt_input_and_retains_provenance() {
    let receipt = forge_receipt();
    let (input_dir, manifest) = environment_receipt_input(
        "forge-input",
        &receipt,
        "mncs-forge:provider-artifact-execution",
        None,
    );
    let artifact_dir = private_artifact_dir("resource-sample-with-input");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", &manifest)
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs with Environment receipt input");
    assert!(
        output.status.success(),
        "verified receipt input is accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sample: serde_json::Value = serde_json::from_slice(&output.stdout).expect("sample JSON");
    assert_eq!(
        sample["execution_records"]["records"][0]["source"],
        "forge_receipt"
    );
    assert_eq!(
        sample["execution_records"]["records"][0]["resource_observations"]
            ["cgroup_memory_peak_bytes"],
        7000
    );
    assert_eq!(
        sample["execution_receipt_input"]["capability"],
        "mncs-forge:provider-artifact-execution"
    );
    assert_eq!(
        sample["execution_receipt_input"]["store_reference"],
        format!(
            "mncs-store:ses_fixture:capability-invocation-artifacts:{}",
            "b".repeat(64),
        )
    );
    let retained =
        std::fs::read(artifact_dir.join("system-resource-sample.json")).expect("retained sample");
    assert_eq!(retained, output.stdout);
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();
}

#[test]
fn environment_invocation_rejects_forged_receipt_input_binding_digest_or_writable_file() {
    let receipt = forge_receipt();
    let (input_dir, manifest) = environment_receipt_input(
        "wrong-capability",
        &receipt,
        "mncs-system-monitor:system-resource-sample",
        None,
    );
    let artifact_dir = private_artifact_dir("wrong-capability-output");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", &manifest)
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "non-Forge binding is rejected");
    assert!(
        !artifact_dir.join("system-resource-sample.json").exists(),
        "rejected input must not produce a sample"
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();

    let (input_dir, manifest) = environment_receipt_input(
        "wrong-digest",
        &receipt,
        "mncs-forge:provider-artifact-execution",
        Some(&"0".repeat(64)),
    );
    let artifact_dir = private_artifact_dir("wrong-digest-output");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", &manifest)
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "digest mismatch is rejected");
    assert!(
        !artifact_dir.join("system-resource-sample.json").exists(),
        "rejected input must not produce a sample"
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();

    let (input_dir, manifest) = environment_receipt_input(
        "writable-input",
        &receipt,
        "mncs-forge:provider-artifact-execution",
        None,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            input_dir.join("0000.artifact"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("make input fixture writable");
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(input_dir.join("0000.artifact"))
            .expect("input fixture metadata")
            .permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(input_dir.join("0000.artifact"), permissions)
            .expect("make input fixture writable");
    }
    let artifact_dir = private_artifact_dir("writable-input-output");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", &manifest)
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "writable input is rejected");
    assert!(
        !artifact_dir.join("system-resource-sample.json").exists(),
        "rejected input must not produce a sample"
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();
}

#[cfg(unix)]
#[test]
fn environment_invocation_rejects_receipt_input_symlinks_and_path_traversal() {
    use std::os::unix::fs::symlink;

    let receipt = forge_receipt();
    let (input_dir, manifest) = environment_receipt_input(
        "symlink-input",
        &receipt,
        "mncs-forge:provider-artifact-execution",
        None,
    );
    let outside = input_dir.with_extension("outside");
    std::fs::write(&outside, receipt.as_bytes()).expect("outside receipt fixture");
    std::fs::remove_file(input_dir.join("0000.artifact")).expect("remove input fixture");
    symlink(&outside, input_dir.join("0000.artifact")).expect("receipt symlink");
    let artifact_dir = private_artifact_dir("symlink-input-output");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", &manifest)
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "symlink inputs are rejected");
    assert!(
        !artifact_dir.join("system-resource-sample.json").exists(),
        "rejected input must not produce a sample"
    );
    assert_eq!(
        std::fs::read(&outside).expect("outside fixture preserved"),
        receipt.as_bytes()
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();
    std::fs::remove_file(&outside).ok();

    let (input_dir, manifest) = environment_receipt_input(
        "nested-symlink-input",
        &receipt,
        "mncs-forge:provider-artifact-execution",
        None,
    );
    let mut manifest: serde_json::Value = serde_json::from_str(&manifest).expect("manifest JSON");
    manifest["artifacts"][0]["local_path"] = serde_json::json!("linked/0000.artifact");
    let outside_dir = input_dir.with_extension("outside-dir");
    std::fs::create_dir(&outside_dir).expect("outside input directory");
    std::fs::write(outside_dir.join("0000.artifact"), receipt.as_bytes())
        .expect("outside nested receipt fixture");
    symlink(&outside_dir, input_dir.join("linked")).expect("input directory symlink");
    let artifact_dir = private_artifact_dir("nested-symlink-output");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", manifest.to_string())
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "symlink directories are rejected");
    assert!(
        !artifact_dir.join("system-resource-sample.json").exists(),
        "rejected input must not produce a sample"
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();
    std::fs::remove_dir_all(&outside_dir).ok();

    let (input_dir, manifest) = environment_receipt_input(
        "traversal-input",
        &receipt,
        "mncs-forge:provider-artifact-execution",
        None,
    );
    let mut manifest: serde_json::Value = serde_json::from_str(&manifest).expect("manifest JSON");
    manifest["artifacts"][0]["local_path"] = serde_json::json!("../outside.json");
    let artifact_dir = private_artifact_dir("traversal-input-output");
    let output = Command::new(monitor_bin())
        .arg("--resource-sample")
        .env("MNCS_ENV_INPUT_ARTIFACT_DIR", &input_dir)
        .env("MNCS_ENV_INPUT_ARTIFACTS_JSON", manifest.to_string())
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifact_dir)
        .output()
        .expect("monitor runs");
    assert!(!output.status.success(), "path traversal is rejected");
    assert!(
        !artifact_dir.join("system-resource-sample.json").exists(),
        "rejected input must not produce a sample"
    );
    std::fs::remove_dir_all(&artifact_dir).ok();
    std::fs::remove_dir_all(&input_dir).ok();
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
