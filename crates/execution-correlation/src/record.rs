//! Ingested canonical execution records.
//!
//! A record is a claim made by its owning subsystem, copied here for correlation. The monitor
//! never authors execution state: [`execution_records_from_json`] accepts the monitor's own
//! ingestion envelope, and [`execution_records_from_test_result`] reads the outcome section of
//! a `mncs.test-result/1` envelope without reinterpreting it.

use serde_json::Value;

/// Versioned ingestion envelope for explicitly authored execution records.
pub const EXECUTION_RECORD_SCHEMA: &str = "mncs.system-monitor.execution-record.v1";

/// The schema this crate reads test outcomes from. Anything else is rejected, never widened.
const TEST_RESULT_SCHEMA: &str = "mncs.test-result/1";

/// An opaque canonical execution identity: a Forge run/record identity, a test `run_id`, or an
/// explicitly assigned token. The monitor never parses inside it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExecutionIdentity(pub String);

/// Canonical lifecycle state as reported by the owning subsystem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionStatus {
    Active,
    Completed,
    Cancelled,
    Unknown,
}

/// Verification outcome as reported by the owning subsystem. `Unknown` preserves operational
/// incompleteness (interruption, missing evidence, infrastructure failure) and must never be
/// converted into [`VerificationOutcome::Fail`] by correlation evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerificationOutcome {
    Pass,
    Fail,
    NotFinished,
    Unknown { reason: Option<String> },
}

/// An admitted resource envelope, consumed from canonical policy. Every bound is optional: an
/// absent bound means the policy did not state one, and the monitor must not invent a default.
/// `declared` distinguishes "policy stated an (unenveloped) execution" from "no policy seen".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceEnvelope {
    pub memory_max_bytes: Option<u64>,
    pub cpu_quota_cores: Option<f64>,
    pub disk_min_free_bytes: Option<u64>,
    pub declared: bool,
}

/// Where a record came from. Provenance travels with the record so consumers can tell a Forge
/// receipt from a hand-authored file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordSource {
    ForgeReceipt,
    ForgeObservation,
    TestResult,
    Manual,
    Unknown,
}

/// One ingested execution record. `host_pid`/`host_start_marker` are linkage hints recorded by
/// the launcher, not monitor discoveries; correlation matches them exactly or not at all.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionRecord {
    pub identity: ExecutionIdentity,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub argv: Vec<String>,
    pub executable: Option<String>,
    pub host_pid: Option<u32>,
    pub host_start_marker: Option<u64>,
    pub envelope: ResourceEnvelope,
    pub status: ExecutionStatus,
    pub outcome: VerificationOutcome,
    pub verification_identity: Option<String>,
    pub evidence_path: Option<String>,
    pub source: RecordSource,
}

/// Why ingestion refused a document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IngestError {
    MalformedJson(String),
    UnexpectedSchema {
        expected: &'static str,
        found: String,
    },
    MissingField(&'static str),
    InvalidValue {
        field: &'static str,
        detail: String,
    },
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedJson(detail) => write!(formatter, "malformed JSON: {detail}"),
            Self::UnexpectedSchema { expected, found } => {
                write!(
                    formatter,
                    "unexpected schema '{found}', expected '{expected}"
                )
            }
            Self::MissingField(field) => write!(formatter, "missing required field '{field}'"),
            Self::InvalidValue { field, detail } => {
                write!(formatter, "invalid value for '{field}': {detail}")
            }
        }
    }
}

impl std::error::Error for IngestError {}

fn check_ingestion_schema(value: &Value) -> Result<(), IngestError> {
    match value.get("schema").and_then(Value::as_str) {
        None | Some(EXECUTION_RECORD_SCHEMA) => Ok(()),
        Some(found) => Err(IngestError::UnexpectedSchema {
            expected: EXECUTION_RECORD_SCHEMA,
            found: found.to_string(),
        }),
    }
}

fn optional_string(value: &Value, field: &'static str) -> Result<Option<String>, IngestError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(IngestError::InvalidValue {
            field,
            detail: "expected a string".to_string(),
        }),
    }
}

fn optional_u64(value: &Value, field: &'static str) -> Result<Option<u64>, IngestError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let bytes = number.as_u64().ok_or(IngestError::InvalidValue {
                field,
                detail: "expected an unsigned integer".to_string(),
            })?;
            Ok(Some(bytes))
        }
        Some(_) => Err(IngestError::InvalidValue {
            field,
            detail: "expected an unsigned integer".to_string(),
        }),
    }
}

fn parse_status(text: &str) -> Result<ExecutionStatus, IngestError> {
    match text {
        "active" => Ok(ExecutionStatus::Active),
        "completed" => Ok(ExecutionStatus::Completed),
        "cancelled" => Ok(ExecutionStatus::Cancelled),
        "unknown" => Ok(ExecutionStatus::Unknown),
        _ => Err(IngestError::InvalidValue {
            field: "status",
            detail: format!("expected active|completed|cancelled|unknown, found '{text}'"),
        }),
    }
}

fn parse_outcome(value: &Value) -> Result<VerificationOutcome, IngestError> {
    match value.get("outcome").and_then(Value::as_str) {
        None | Some("not_finished") => Ok(VerificationOutcome::NotFinished),
        Some("pass") => Ok(VerificationOutcome::Pass),
        Some("fail") => Ok(VerificationOutcome::Fail),
        Some("unknown") => Ok(VerificationOutcome::Unknown {
            reason: optional_string(value, "unknown_reason")?,
        }),
        Some(found) => Err(IngestError::InvalidValue {
            field: "outcome",
            detail: format!("expected pass|fail|unknown|not_finished, found '{found}'"),
        }),
    }
}

fn parse_envelope(value: &Value) -> Result<ResourceEnvelope, IngestError> {
    let Some(envelope) = value.get("envelope") else {
        return Ok(ResourceEnvelope::default());
    };
    if envelope.is_null() {
        return Ok(ResourceEnvelope::default());
    }
    let cpu_quota_cores = match envelope.get("cpu_quota_cores") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => number.as_f64().filter(|quota| *quota > 0.0),
        Some(_) => {
            return Err(IngestError::InvalidValue {
                field: "envelope.cpu_quota_cores",
                detail: "expected a positive number".to_string(),
            });
        }
    };
    Ok(ResourceEnvelope {
        memory_max_bytes: optional_u64(envelope, "memory_max_bytes")?,
        cpu_quota_cores,
        disk_min_free_bytes: optional_u64(envelope, "disk_min_free_bytes")?,
        declared: true,
    })
}

fn parse_source(value: &Value) -> RecordSource {
    match value.get("source").and_then(Value::as_str) {
        Some("forge_receipt") => RecordSource::ForgeReceipt,
        Some("forge_observation") => RecordSource::ForgeObservation,
        Some("test_result") => RecordSource::TestResult,
        Some("manual") => RecordSource::Manual,
        _ => RecordSource::Unknown,
    }
}

fn record_from_value(value: &Value) -> Result<ExecutionRecord, IngestError> {
    check_ingestion_schema(value)?;
    let identity = value
        .get("execution_identity")
        .and_then(Value::as_str)
        .ok_or(IngestError::MissingField("execution_identity"))?;
    if identity.is_empty() {
        return Err(IngestError::InvalidValue {
            field: "execution_identity",
            detail: "identity must be non-empty".to_string(),
        });
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .ok_or(IngestError::MissingField("status"))
        .and_then(parse_status)?;
    let argv = match value.get("argv") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut argv = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(text) => argv.push(text.to_string()),
                    None => {
                        return Err(IngestError::InvalidValue {
                            field: "argv",
                            detail: "argv entries must be strings".to_string(),
                        });
                    }
                }
            }
            argv
        }
        Some(_) => {
            return Err(IngestError::InvalidValue {
                field: "argv",
                detail: "expected an array of strings".to_string(),
            });
        }
    };
    let host_pid = optional_u64(value, "host_pid")?.and_then(|pid| u32::try_from(pid).ok());
    Ok(ExecutionRecord {
        identity: ExecutionIdentity(identity.to_string()),
        repository: optional_string(value, "repository")?,
        revision: optional_string(value, "revision")?,
        argv,
        executable: optional_string(value, "executable")?,
        host_pid,
        host_start_marker: optional_u64(value, "host_start_marker")?,
        envelope: parse_envelope(value)?,
        status,
        outcome: parse_outcome(value)?,
        verification_identity: optional_string(value, "verification_identity")?,
        evidence_path: optional_string(value, "evidence_path")?,
        source: parse_source(value),
    })
}

/// Parse one record or an array of records from JSON text. Malformed input is a hard
/// [`IngestError`], never a partially accepted set: the caller retries with a fixed document.
pub fn execution_records_from_json(text: &str) -> Result<Vec<ExecutionRecord>, IngestError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| IngestError::MalformedJson(error.to_string()))?;
    match &value {
        Value::Array(items) => items.iter().map(record_from_value).collect(),
        Value::Object(_) => record_from_value(&value).map(|record| vec![record]),
        _ => Err(IngestError::InvalidValue {
            field: "document",
            detail: "expected an execution record object or an array of them".to_string(),
        }),
    }
}

/// Read the outcome section of a `mncs.test-result/1` envelope as an execution record.
///
/// A returned envelope describes a finished run, so the record status is always
/// [`ExecutionStatus::Completed`]; interruption without an envelope is the stale-active path,
/// not something this function can observe. Fields the envelope does not carry (repository,
/// revision, host PID, envelope) stay absent rather than inferred.
pub fn execution_records_from_test_result(text: &str) -> Result<Vec<ExecutionRecord>, IngestError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| IngestError::MalformedJson(error.to_string()))?;
    match value.get("schema_version").and_then(Value::as_str) {
        Some(TEST_RESULT_SCHEMA) => {}
        other => {
            return Err(IngestError::UnexpectedSchema {
                expected: TEST_RESULT_SCHEMA,
                found: other.unwrap_or("(absent)").to_string(),
            });
        }
    }
    let run_id = value
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or(IngestError::MissingField("run_id"))?;
    let verdict = value
        .get("summary")
        .and_then(|summary| summary.get("verdict"))
        .and_then(Value::as_str);
    let outcome = match verdict {
        Some("PASS") => VerificationOutcome::Pass,
        Some("FAIL") => VerificationOutcome::Fail,
        _ => VerificationOutcome::Unknown {
            reason: Some(format!(
                "test verdict '{}' carries no PASS/FAIL claim",
                verdict.unwrap_or("(absent)")
            )),
        },
    };
    Ok(vec![ExecutionRecord {
        identity: ExecutionIdentity(format!("mncs-test:{run_id}")),
        repository: None,
        revision: None,
        argv: Vec::new(),
        executable: None,
        host_pid: None,
        host_start_marker: None,
        envelope: ResourceEnvelope::default(),
        status: ExecutionStatus::Completed,
        outcome,
        verification_identity: value
            .get("scope")
            .and_then(|scope| scope.get("module"))
            .and_then(Value::as_str)
            .map(str::to_string),
        evidence_path: None,
        source: RecordSource::TestResult,
    }])
}

/// A bounded window of recent records. The monitor keeps linkage inputs small and explicit;
/// long-term execution history belongs in the owning subsystem's record store, not here.
#[derive(Clone, Debug)]
pub struct RecordWindow {
    capacity: usize,
    records: std::collections::VecDeque<ExecutionRecord>,
}

impl RecordWindow {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, 4096),
            records: std::collections::VecDeque::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn push(&mut self, record: ExecutionRecord) {
        if self.records.len() >= self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    pub fn records(&self) -> Vec<&ExecutionRecord> {
        self.records.iter().collect()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_json(identity: &str, status: &str) -> String {
        format!(r#"{{"execution_identity": "{identity}", "status": "{status}"}}"#)
    }

    #[test]
    fn rejects_unknown_ingestion_schema() {
        let error = execution_records_from_json(
            r#"{"schema": "nope", "execution_identity": "a",
                "status": "active"}"#,
        )
        .expect_err("unknown schema must be rejected");
        assert!(matches!(error, IngestError::UnexpectedSchema { .. }));
    }

    #[test]
    fn rejects_missing_identity() {
        let error = execution_records_from_json(r#"{"status": "active"}"#)
            .expect_err("missing identity must be rejected");
        assert_eq!(error, IngestError::MissingField("execution_identity"));
    }

    #[test]
    fn parses_full_record() {
        let records = execution_records_from_json(
            r#"{
                "schema": "mncs.system-monitor.execution-record.v1",
                "execution_identity": "forge:run:abc",
                "repository": "mncs-numerics",
                "revision": "2a76bb9",
                "argv": ["python3", "scripts/run_tests.py"],
                "executable": "python3",
                "host_pid": 4242,
                "host_start_marker": 99,
                "envelope": {"memory_max_bytes": 1073741824, "cpu_quota_cores": 2.0},
                "status": "active",
                "outcome": "not_finished",
                "source": "forge_observation"
            }"#,
        )
        .expect("full record must parse");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.identity.0, "forge:run:abc");
        assert_eq!(record.host_pid, Some(4242));
        assert_eq!(record.envelope.memory_max_bytes, Some(1_073_741_824));
        assert!(record.envelope.declared);
        assert_eq!(record.status, ExecutionStatus::Active);
        assert_eq!(record.source, RecordSource::ForgeObservation);
    }

    #[test]
    fn absent_envelope_means_no_policy_seen() {
        let records =
            execution_records_from_json(&record_json("a", "completed")).expect("minimal parses");
        assert!(!records[0].envelope.declared);
        assert_eq!(records[0].envelope.memory_max_bytes, None);
    }

    #[test]
    fn rejects_wrong_test_result_schema() {
        let error = execution_records_from_test_result(r#"{"schema_version": "other/1"}"#)
            .expect_err("wrong schema must be rejected, never widened");
        assert!(matches!(error, IngestError::UnexpectedSchema { .. }));
    }

    #[test]
    fn maps_test_result_verdicts() {
        let passed = execution_records_from_test_result(
            r#"{"schema_version": "mncs.test-result/1", "run_id": "r1",
                "summary": {"verdict": "PASS"}}"#,
        )
        .expect("PASS envelope parses");
        assert_eq!(passed[0].outcome, VerificationOutcome::Pass);
        assert_eq!(passed[0].status, ExecutionStatus::Completed);

        let unknown = execution_records_from_test_result(
            r#"{"schema_version": "mncs.test-result/1", "run_id": "r2",
                "summary": {"verdict": "UNKNOWN"}}"#,
        )
        .expect("UNKNOWN envelope parses");
        assert!(matches!(
            unknown[0].outcome,
            VerificationOutcome::Unknown { .. }
        ));
    }

    #[test]
    fn record_window_evicts_oldest() {
        let mut window = RecordWindow::new(2);
        for identity in ["a", "b", "c"] {
            window.push(
                execution_records_from_json(&record_json(identity, "completed"))
                    .expect("parses")
                    .remove(0),
            );
        }
        assert_eq!(window.len(), 2);
        let identities: Vec<&str> = window
            .records()
            .iter()
            .map(|record| record.identity.0.as_str())
            .collect();
        assert_eq!(identities, vec!["b", "c"]);
    }
}
