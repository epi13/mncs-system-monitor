# Roadmap

This is a research sequence. “Scaffolded” means the boundary exists; it does not mean the
capability is complete or production-ready.

## Phase 0 — Repository bootstrap

**Status: complete and superseded by the exercised phases below.**

- establish a Rust workspace for monitor core, host collection, machine projection, and CLI;
- define process/resource subjects, provenance, timing, and explicit uncertainty;
- document ownership with `mncs-tui`, `mncs-language`, and `mncs-language-service`;
- keep the executable honest by making unsupported and incomplete collection explicit.

## Phase 1 — Linux host facts

**Status: exercised.**

- add bounded, fixture-driven `/proc` parsing for process identity, state, CPU, memory, threads,
  and I/O;
- add `/sys` and host load/uptime sources with unit and freshness checks;
- model permission-limited, disappearing, malformed, and PID-reuse cases;
- define explicit collector scope and namespace assumptions.

## Phase 2 — Sampling and reconciliation

**Status: exercised.**

- add a bounded sampling scheduler and retention policy;
- derive rates and deltas only when counter continuity and intervals are established;
- reconcile process starts, exits, parent links, services, and cgroups;
- preserve source provenance and stale/unknown states through aggregation.

## Phase 3 — First human projection

**Status: experimental and runnable.**

- build the Overview and Processes views as a separate `mncs-tui` application layer;
- consume `mncs-tui` layout, table, focus, event, frame, and terminal contracts;
- add process selection and inspection without turning TUI text into an API;
- propose generic chart/time-series primitives upstream when the monitor demonstrates the need.

## Phase 4 — Machine projection

**Status: exercised with versioned JSON.**

- choose a versioned local transport and serialization format;
- expose snapshots, process inspection, filters, and bounded history to agents;
- retain source, freshness, identity strength, and unknown fields;
- test machine responses against the same snapshot consumed by the TUI.

## Phase 5 — Relationships, pressure, and events

**Status: experimental.**

- add typed process/service/cgroup graph views;
- add memory, I/O, CPU, and file-descriptor pressure signals;
- add starts, exits, crashes, spikes, bursts, and limit events with evidence boundaries;
- make event loss, sampling gaps, and clock assumptions visible.

## Phase 6 — Platform and MNCS feedback

**Status: deferred where it requires new language/runtime effects. The genuine reusable TUI
host-realization gap and bounded sparkline need were implemented upstream in mncs-tui; no
language-service change was required by this vertical slice.**

- add other platform adapters only after the Linux boundary is testable;
- identify reusable time, provenance, bounded-series, rate, and delta primitives;
- upstream generic discoveries to `mncs-language` rather than copying them here;
- add language-service context for semantic monitor inspection and repair.

## Explicit non-goals

The bootstrap will not freeze a final wire format, promise universal operating-system parity,
require root access, emulate a terminal, or create a second TUI framework. It will not infer clean
process exits from missing rows, claim exactness from stale counters, or turn a successful render
into evidence that the underlying host observation was correct.

## Acceptance vocabulary

- **scaffolded** — structure and design boundary exist;
- **experimental** — a bounded path exists while semantics or APIs remain in motion;
- **exercised** — representative fixtures and checks have run;
- **deferred** — intentionally postponed;
- **blocked/unresolved** — dependent on missing upstream semantics, host capability, or evidence.
