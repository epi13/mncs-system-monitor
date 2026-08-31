# Architecture

## Purpose

`mncs-system-monitor` is a companion application for the MNCS family. Its architectural question
is whether system state can remain semantically structured from host observation through both a
human terminal UI and machine-facing access. The initial repository establishes boundaries; it does
not freeze a production API.

## Pipeline

```text
┌────────────────────┐
│ host operating      │  /proc, /sys, cgroups, systemd, network, sensors
│ system              │
└──────────┬─────────┘
           │ raw facts + source failures
           ▼
┌────────────────────┐
│ Rust host collector │  parsing, privileges, sampling, freshness
└──────────┬─────────┘
           │ normalized monitor subjects
           ▼
┌────────────────────┐
│ semantic snapshot   │  processes, resources, relationships, provenance
└──────────┬─────────┘
           │ shared state, never rendered text
       ┌───┴────────────────┐
       ▼                    ▼
┌───────────────┐   ┌────────────────────┐
│ human view     │   │ machine projection │
│ mncs-tui       │   │ structured data   │
└──────┬────────┘   └──────────┬─────────┘
       ▼                       ▼
 terminal                local API / agent
```

The shared snapshot is the architectural center. A TUI frame and a machine response are different
projections of that state, not serialization stages in a human-output pipeline.

## Workspace ownership

```text
mncs-system-monitor
  owns monitor vocabulary, observation aggregation, sampling/delta policy,
  process/resource relationships, monitor-specific views, and projection contracts

mncs-host-collector
  owns /proc, /sys, cgroups, systemd/DBus, sockets, privileges, host clocks,
  platform parsing, and raw adapter failure modes

mncs-tui
  owns panes, tables, lists, charts when available, focus, hit-testing, layout,
  structured frames, terminal commands, and terminal lifecycle

mncs-language
  owns syntax, types, contracts, effects, identities, bounded computation,
  verification, lowering, generic time/collection primitives, and ANSI/VT meaning

mncs-language-service
  owns resident source analysis, diagnostics, navigation, context, and editor/agent adaptation
```

The Rust crates at the root of this repository are host/application code, not an attempt to
reimplement the MNCS language in Rust. The future `tui/` source should consume `mncs-tui`'s
authoritative geometry and interaction vocabulary.

## Semantic state

The monitor model retains the facts that are commonly lost in a string-first monitor:

- process identity includes a PID plus an optional start marker so PID reuse is visible;
- resource samples carry an interval where a rate or utilization was derived;
- observations retain subject, time, source, and value;
- process relationships remain typed rather than being reconstructed from indentation;
- unsupported, unavailable, malformed, and permission-limited facts remain distinguishable;
- missing values remain missing instead of becoming zeroes or plausible defaults.

The sampler now adds bounded history, deltas, rates, lifecycle reconciliation, and evidence-scoped
events around each accepted point-in-time snapshot. Retention is explicit: history is capped by
configuration and process history is capped per sample.

## Collection boundary

`mncs-host-collector` is the only crate allowed to acquire host facts. A platform adapter should
follow this sequence:

1. read a bounded source using declared authority;
2. parse into a source-specific intermediate value;
3. validate identity, units, freshness, and relationships;
4. translate into `mncs-monitor-core` subjects;
5. report unsupported, unavailable, malformed, or permission-limited results explicitly.

The Linux adapter now implements the /proc boundary with a /sys CPU-online fallback. Process
directory enumeration is only a candidate set: each process is re-read and may disappear before
its stat/status/I/O files are read. Such failures are recorded as issues, not converted to rows
with invented zeros. Permission, unavailable, malformed, and counter-regression statuses remain
visible in the semantic snapshot.

## Projection boundary

The machine projection borrows SystemSnapshot and emits the versioned
mncs.system-monitor.snapshot.v1 JSON envelope. It includes identity, raw counters, qualified
rates, source/status, timing, relationships, transitions, bounded history, events, and collection
issues. The TUI and JSON projection receive the same accepted snapshot; neither scrapes the other.

The TUI should follow the existing `mncs-tui` pipeline:

```text
semantic snapshot → widget graph → constraints → frame/diff → terminal projection
```

Layout, focus, hit-testing, ANSI parsing, and terminal cleanup are not monitor-owned semantics.

## State and effects

The intended monitor loop is:

1. wait for a bounded sampling interval or a relevant host event;
2. collect a bounded batch of host observations;
3. reconcile process identities and relationships;
4. derive bounded deltas, rates, and pressure signals when inputs are sufficient;
5. publish a new accepted snapshot to both projections;
6. let the mncs-tui host realization write the human frame;
7. expose the same accepted snapshot through the JSON machine-facing adapter.

Host reads, timers, process inspection, sockets, systemd access, and terminal I/O are effects. They
should remain at their declared boundaries with authority, failure behavior, and cleanup visible.

## Verification pressure

The first useful checks are local and bounded:

- a process identity changes when its start marker changes;
- a derived utilization has a non-zero interval and valid range;
- a sample never claims a stronger source than the collector established;
- unavailable data is not emitted as a zero-valued observation;
- process parent links refer to the same snapshot identity namespace;
- a machine view and TUI view observe the same accepted snapshot;
- collector failures preserve `UNKNOWN` rather than being hidden by a fallback;
- TUI layout and terminal lifecycle obligations remain covered by `mncs-tui` fixtures.

These are candidate obligations for the bootstrap, not production certification claims.

## Open questions

- Which process/service/cgroup relationship vocabulary should remain monitor-specific?
- What is the smallest bounded history representation for rates and deltas? The current sampler
  retains up to 120 samples and up to 512 process points per sample.
- How should namespace boundaries and containers affect identity and visibility?
- Which Linux facts require elevated privileges, and how should partial views be represented?
- Should pressure use a monitor-level qualitative vocabulary or preserve source-native detail?
- What machine-facing transport best preserves typed data without coupling the monitor to one API?
- Which reusable time, provenance, and bounded-series primitives should be proposed upstream?
