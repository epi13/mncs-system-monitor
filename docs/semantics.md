# Semantic vocabulary

This document defines the bootstrap vocabulary owned by the monitor. It is a design boundary, not
a promise that these Rust structs are the final MNCS representation.

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

An observation has four parts:

```text
Observation<T> {
    subject: Process | Service | Cgroup | Interface | Host,
    observed_at: timestamp,
    source: ProcStat | ProcStatus | Sysfs | Cgroup | Systemd | HostApi | ...,
    value: T,
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
- Pressure records retain a kind, qualitative level, source, and optional time window.

The monitor does not claim that similarly named fields from different operating systems have
identical semantics. Adapters must document source-specific assumptions and preserve `Unknown`
where normalization would be misleading.

## Identity and lifecycle

PIDs are reusable. `ProcessIdentity` therefore contains the PID and an optional host start marker.
If a source does not expose a start marker, the identity remains weaker and reconciliation must not
claim continuity across a reuse boundary.

Process start and exit events belong to a future event/reconciliation layer. A process disappearing
from one snapshot is not, by itself, proof of a clean exit.

## Uncertainty vocabulary

The project follows the MNCS-family distinction between known outcomes and missing evidence:

- `PASS` — the bounded check established its stated property;
- `FAIL` — the bounded check found a violation;
- `UNKNOWN` — the check or source was unavailable, unsupported, stale, malformed, or incomplete.

At the data boundary, `Unknown` enum values and `Option<T>` fields carry local uncertainty. They
must not be collapsed into normal values merely to make a table or API easier to consume.

## Ownership test

Before adding a type, ask:

1. Is this a fact about a host resource or monitor observation? It may belong here.
2. Is it about rectangles, focus, frames, terminal commands, or ANSI/VT meaning? It belongs in
   `mncs-tui` or its upstream authorities.
3. Is it a generic type, effect, identity, evidence, time, or bounded-computation primitive? Record
   language pressure and consider `mncs-language`.
4. Is it about source diagnostics, navigation, or agent/editor context? Consider
   `mncs-language-service`.

The goal is a clear vertical slice, not a second shared platform hidden inside the monitor.
