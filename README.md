# mncs-system-monitor

[![CI](https://github.com/epi13/mncs-system-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/epi13/mncs-system-monitor/actions/workflows/ci.yml)

A machine-native system observability surface with both human and machine projections.

> **Status: experimental runnable vertical slice.** On Linux, the repository collects a bounded
> `/proc`/`/sys` snapshot, derives interval-qualified values, and exposes the same semantic state
> through a human terminal view and versioned JSON. APIs and cross-platform behavior remain
> experimental.

## Why this project exists

`mncs-system-monitor` is a companion project to [`mncs-tui`](https://github.com/epi13/mncs-tui),
not a replacement for it and not a new home for generic terminal semantics. The monitor explores a
different vertical slice:

```text
Linux / host facts
      ↓
Rust host collector boundary
      ↓
typed monitor observations and relationships
      ↓
┌──────────────────────┬──────────────────────┐
│ human projection      │ machine projection   │
│ mncs-tui application  │ structured API/data  │
└──────────────────────┴──────────────────────┘
```

The same structured state should serve a person inspecting a busy process and an agent asking for
the processes consuming memory. Neither projection should need to scrape the other.

## Initial design

The monitor owns its domain vocabulary: processes, process identity, resource samples, process
relationships, pressure signals, sampling policy, and monitor-specific uncertainty. The host
adapter owns OS interaction. `mncs-tui` owns layout, widgets, focus, rendering, and terminal
interaction. `mncs-language` remains the authority for language, type, effect, identity, and
verification semantics.

The first intended views are:

1. **Overview** — CPU, memory, swap, load, uptime, process count, disk I/O, network throughput,
   and compact history.
2. **Processes** — a sortable and filterable process table with PID identity, parent, CPU, memory,
   threads, state, executable, user, start marker, and I/O.
3. **Process graph** — typed relationships between processes, services, and cgroups rather than
   indentation reconstructed from text.
4. **Events and pressure** — starts, exits, crashes, spikes, bursts, limits, and resource pressure
   with source and timing retained.

The Overview and Processes views are exercised today. Graph, service/cgroup discovery, and richer
event/pressure panes remain intentionally staged.

## Repository layout

```text
crates/
  monitor-core/          monitor-owned semantic models and observation vocabulary
  host-collector/        OS boundary; platform adapters and explicit collection errors
  machine-projection/    machine-facing view over the shared semantic snapshot
  monitor-cli/           interactive terminal application and JSON entry point
tui/
  README.md              mncs-tui application boundary and source fixture
  monitor-app.mncs       bounded MNCS geometry/chart integration contract
docs/
  README.md              documentation map
  architecture.md       pipeline, ownership, and state/effect boundaries
  semantics.md          monitor vocabulary and observation contracts
  roadmap.md            staged implementation plan and non-goals
  contributing.md        change and evidence expectations
examples/
  README.md              illustrative fixtures and future host examples
```

The monitor core remains standard-library oriented. The executable uses the small upstream
`mncs-tui-host` realization for Unix terminal lifecycle, input, resize, structured frames, and
diffed output; it does not copy terminal or ANSI semantics into this repository.

## Quick start

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p mncs-system-monitor -- --once
cargo run -p mncs-system-monitor -- --json --once
cargo run -p mncs-system-monitor -- --json --once --executions examples/executions-sample.json
cargo run -p mncs-system-monitor -- --reconcile examples/executions-sample.json
```

The default command enters the interactive TUI when stdout is a terminal. Use `--no-tui` for a
concise human snapshot, `--json` for the machine projection, `--interval-ms N` to set the sampling
interval, and `--history N` to bound retained history. The first sample intentionally reports
CPU/rate fields as warming when no valid counter interval exists.

## Canonical execution correlation

Host telemetry alone cannot tell a Forge execution from a stray process. `--executions FILE`
ingests canonical execution records — the monitor's own `execution-record.v1` envelope (see
`examples/executions-sample.json`) or a `mncs.test-result/1` envelope, whose outcome section
is read without reinterpretation — and merges a versioned `correlation` section into `--json`
output: per-record linkage (`linked`, `process_absent`, `pid_reused`, `no_pid_declared`),
anomalies with explanatory detail (`stale_active_execution`, `lingering_process`,
`envelope_exceeded`, `saturation_with_unknown`, `watched_orphan`), and stable counts.
`--watch-exe NAME` (repeatable) flags a watched executable running with no linked Active
execution. `--reconcile FILE` classifies retained records against one fresh snapshot after a
restart without resurrecting terminal state or resolving UNKNOWN.

Linkage is PID plus start marker, exactly or not at all: no command-line, path, name, or
timestamp heuristics. Resource comparisons use the admitted envelope carried by the record;
the monitor holds no thresholds of its own, never kills or cancels anything, and never
converts an operational UNKNOWN into a semantic failure. Malformed records files fail loudly
at startup rather than degrading into silent UNKNOWN telemetry.

For the MNCS/TUI source path, keep sibling checkouts of this repository and
[`mncs-tui`](https://github.com/epi13/mncs-tui), then follow the integration notes in
[`tui/README.md`](tui/README.md). The monitor should consume TUI geometry, widgets, events, and
terminal projection from that project rather than copying them here.

## Relationship to the MNCS family

- **`mncs-system-monitor`** owns host-observability subjects, their projections, and the
  correlation of live host state against ingested canonical execution records.
- **`mncs-tui`** owns panes, tables, lists, charts when upstreamed, focus, layout, rendering, and
  terminal interaction.
- **`mncs-language`** owns language semantics, generic identity/effect/verification primitives,
  and compiler/lowering contracts.
- **`mncs-language-service`** owns resident analysis, diagnostics, navigation, and agent/editor
  context for the source.
- **Forge** owns execution, admission, resource envelopes, cancellation, and verification state;
  the monitor ingests records and never decides policy.
- **Store** owns persistent substrate; the monitor keeps bounded in-memory history and no
  operational database.
- **Commons/Atlas** own typed relationships and family topology; record identity fields are
  opaque strings here.
- **Debug** owns diagnosis; anomalies link evidence (`evidence_path`) for Debug to consume.

When this project discovers a reusable primitive—such as bounded time series, rates, deltas, or
provenance—it should record the pressure and propose it upstream. It should not create a competing
generic ontology just because the monitor needs the concept first.

## Development status

See the [architecture](docs/architecture.md), [semantic vocabulary](docs/semantics.md), and
[roadmap](docs/roadmap.md) before adding an implementation. In particular, a result may be
`PASS`, `FAIL`, or `UNKNOWN`; missing host privileges, unsupported platforms, unavailable sensors,
and unimplemented adapters remain `UNKNOWN`.

## Current boundaries and non-goals

This vertical slice does not promise:

- a stable public API or wire format beyond the explicitly versioned experimental JSON envelope;
- a complete BSD, macOS, or Windows collector;
- root privileges, namespace traversal, cgroup discovery, or systemd integration;
- a production-grade scheduler, persistence layer, or event delivery guarantee;
- a terminal emulator or a second TUI framework;
- agent access by scraping human-oriented terminal output;
- proof, certification, or universal accuracy from bounded local experiments.

Licensed under Apache-2.0. See [LICENSE](LICENSE).
