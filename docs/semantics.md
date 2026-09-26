# Semantic vocabulary

This document defines the currently exercised vocabulary owned by the monitor. It is a Rust
semantic boundary, not a promise that these structs are the final MNCS representation.

## Evidence statuses

Every source may establish a value, or establish why it did not. observed, unknown, unavailable,
unsupported, permission_denied, malformed, disappeared, stale, counter_regression, and
invalid_interval are distinct. A missing /proc/<pid>/io file is not zero I/O; a first sample is
not a zero rate; and a PID absent from the next enumeration is not evidence of a clean exit.

## Subjects

| Subject | Meaning | Required identity |
| --- | --- | --- |
| `Process` | A process visible to the selected host view | PID plus optional start marker |
| `Service` | A service identity supplied by a service manager or host source | source-native stable name |
| `Cgroup` | A resource-control grouping | source-native path or identifier |
| `Interface` | A host network interface | source-native interface name |
| `Host` | The observed host scope | collector scope |

Relationships between these subjects are facts from a source. They are not inferred merely because
two display names look related.

## Observations

An observation has a subject, observation time, source, evidence status, and an optional value:

```text
Observation<T> {
    subject: Process | Service | Cgroup | Interface | Host,
    observed_at: timestamp,
    source: ProcStat | ProcStatus | Sysfs | Cgroup | Systemd | HostApi | ...,
    status: Observed | Unknown | Unavailable | Unsupported | PermissionDenied | Malformed | ...,
    value: Option<T>,
}
```

The timestamp says when the source was observed, not when the event necessarily occurred. If a
collector cannot establish timing or provenance, it should retain `None` or `Unknown` rather than
manufacture precision.

## Samples and units

- CPU utilization is a ratio when the collector can establish the interval and counter delta.
- Memory values use bytes at the monitor boundary; display units are a projection concern.
- I/O values retain counters or interval-qualified throughput according to what the source
  establishes; a counter is not silently relabeled as a rate.
- Network samples retain interface identity when available.
- Pressure records retain a kind, qualitative level, PSI `some`/`full` averages when Linux exposes
  them, source, and an optional time window.

The sampler retains at most a configured number of HistorySample values (120 by default). Raw
counters stay alongside derived values. CPU, disk, network, and process I/O rates require a valid
monotonic interval and continuity; counter regressions clear the derived rate and retain the
regression status.

The monitor does not claim that similarly named fields from different operating systems have
identical semantics. Adapters must document source-specific assumptions and preserve `Unknown`
where normalization would be misleading.

## Identity and lifecycle

PIDs are reusable. `ProcessIdentity` therefore contains the PID and an optional host start marker.
If a source does not expose a start marker, the identity remains weaker and reconciliation must not
claim continuity across a reuse boundary.

The sampler emits bounded lifecycle transitions and evidence-scoped events for observed starts,
identity changes, and processes no longer present in the next accepted snapshot. A process
disappearing from one snapshot is not, by itself, proof of a clean exit. Missing start markers
produce weaker reconciliation and do not justify a continuity claim.

## Uncertainty vocabulary

The project follows the MNCS-family distinction between known outcomes and missing evidence:

- `PASS` — the bounded check established its stated property;
- `FAIL` — the bounded check found a violation;
- `UNKNOWN` — the check or source was unavailable, unsupported, stale, malformed, or incomplete.

At the data boundary, `Unknown` enum values and `Option<T>` fields carry local uncertainty. They
must not be collapsed into normal values merely to make a table or API easier to consume.

## Canonical execution correlation

Host telemetry answers "what is the machine doing". Records ingested from canonical
subsystems answer "what MNCS believes it started, admitted, and concluded". Correlation
relates the two without merging them:

- **Execution identity** (`forge:run:…`, `mncs-test:<run_id>`, …) is opaque. The monitor
  carries it, links against it, and reports it; it never parses inside it.
- **Linkage tiers**: `linked` (PID plus start marker match exactly), `process_absent`
  (declared PID gone), `pid_reused` (PID alive under a different marker — linkage withheld,
  never guessed), `no_pid_declared` (record carries no host hint), `linkage_unknown`.
  Command-line substrings, paths, names, and timestamps are never linkage evidence.
- **Anomalies** (`stale_active_execution`, `lingering_process`, `pid_reused`,
  `envelope_exceeded`, `saturation_with_unknown`, `watched_orphan`) each explain why they
  matter in `detail`. `envelope_exceeded` compares observed use against the envelope from
  the record — the monitor holds no thresholds of its own. Saturation context comes from
  observed PSI `Full` pressure levels, never from invented limits.
- **UNKNOWN preservation**: `saturation_with_unknown` explains an existing UNKNOWN; it never
  converts operational incompleteness into a semantic failure. Restart reconciliation
  classifies (`active_linked`, `stale_active`, `lingering`, `pid_ambiguous`,
  `completed_observed`, `linkage_unknown`) without resurrecting terminal records or resolving
  UNKNOWN.
- **Restart/recovery**: `--reconcile FILE` classifies retained records against one fresh
  snapshot. Stale-active means host loss or stale canonical state; the outcome stays UNKNOWN
  either way.

## Ownership test

Before adding a type, ask:

1. Is this a fact about a host resource or monitor observation? It may belong here.
2. Is it about rectangles, focus, frames, terminal commands, or ANSI/VT meaning? It belongs in
   `mncs-tui` or its upstream authorities.
3. Is it a generic type, effect, identity, evidence, time, or bounded-computation primitive? Record
   language pressure and consider `mncs-language`.
4. Is it about source diagnostics, navigation, or agent/editor context? Consider
   `mncs-language-service`.
5. Is it about execution admission, envelopes, cancellation, or verification outcomes? Consume
   Forge/Store records; do not model policy here.
6. Is it about diagnosing WHY something failed? Link evidence for Debug; do not diagnose here.

The goal is a clear vertical slice, not a second shared platform hidden inside the monitor.
