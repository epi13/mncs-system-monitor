# Contributing

`mncs-system-monitor` is a host-observability experiment as well as an application repository.
Contributions should clarify a boundary, add a reproducible fixture, or provide evidence for a
design choice.

## Before changing the repository

1. Read [the architecture](architecture.md) and identify the affected boundary.
2. Check whether the proposal belongs in this repository, `mncs-tui`, `mncs-language`, or
   `mncs-language-service`.
3. Keep host reads bounded and test parsing against repository-local fixtures.
4. State units, timing, privilege assumptions, namespace assumptions, and freshness behavior.
5. Preserve `PASS`, `FAIL`, and `UNKNOWN` distinctions; unsupported behavior is not a successful
   empty observation.

## Change categories

- **Host adapter:** OS reads, parsing, permissions, identity, and source-specific assumptions.
- **Monitor model:** process/resource subjects, relationships, reconciliation, sampling, and
  monitor-specific uncertainty.
- **Projection:** machine transport or `mncs-tui` application views over an accepted snapshot.
- **Upstream pressure:** generic language, effect, evidence, time, bounded-series, or TUI
  primitives that should be proposed to their owning repository.
- **Documentation/experiment:** a bounded fixture or explanation that makes a claim testable.

When a change crosses repositories, open and link the upstream issue or pull request. Do not create
a temporary duplicate authority here and allow it to become permanent by accident.

## Source conventions

- Keep Rust modules small and organized by ownership boundary.
- Keep source fixtures deterministic and explicit about host assumptions.
- Prefer typed records and relationships over stringly-typed snapshots.
- Preserve source provenance and missing values through every projection.
- Treat display labels and units as projection concerns, not semantic evidence.
- Call an experiment an experiment; do not call a local check a proof or certification.

## Verification expectations

For every change:

- run `cargo fmt --all -- --check`;
- run `cargo test --workspace` and `cargo check --workspace --all-targets`;
- inspect the resulting tree and diff;
- validate documentation links and examples for internal consistency;
- record unsupported or unresolved checks instead of silently skipping them.

Host adapters should add fixtures for malformed input, permission limits, stale reads, disappearing
processes, PID reuse, and partial source availability before they claim a stronger status.
