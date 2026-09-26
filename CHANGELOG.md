# Changelog

## Unreleased

- added canonical execution correlation (`mncs-execution-correlation`): ingested Forge-shaped
  records and `mncs.test-result/1` outcome envelopes, exact PID-plus-start-marker linkage with
  PID-reuse withholding, six anomaly kinds with explanatory detail, restart reconciliation
  (`--reconcile`), versioned `correlation`/`restart` JSON sections, and deterministic
  fixture-driven tests plus CLI smoke tests;
- added `--executions` and repeatable `--watch-exe` flags, correlation lines in the human
  summary, and `examples/executions-sample.json`;
- documented canonical-subsystem boundaries (Forge/Store/Commons/Atlas/Debug/Doctor/TUI) and
  the UNKNOWN-preservation contract.

- added a real Linux `/proc`/`/sys` collector with explicit partial-source issues;
- added bounded sampling, interval-qualified CPU/I/O/network/disk rates, PID-reuse reconciliation,
  lifecycle events, and bounded history;
- added a versioned JSON projection and an interactive `mncs-tui-host` terminal application with
  overview, process table, filtering, sorting, selection, scrolling, detail, and CPU history;
- added deterministic Linux fixtures, JSON smoke checks, and MNCS/TUI source validation in CI;
- established the initial Rust workspace and monitor ownership boundaries;
- added process/resource observation models with provenance, timing, identity, and uncertainty;
- added explicit host-collector and machine-projection seams;
- documented the architecture, semantic vocabulary, roadmap, and contribution workflow.
