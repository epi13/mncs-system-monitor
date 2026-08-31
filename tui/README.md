# TUI application boundary

The human-facing monitor is built on the mncs-tui framework:
https://github.com/epi13/mncs-tui.

This directory must consume the monitor snapshot and the TUI framework's authoritative geometry,
layout, widget, event, focus, frame, diff, and terminal contracts. It must not grow a second table,
layout, terminal, or ANSI semantic implementation while the upstream framework evolves.

monitor-app.mncs is a bounded source-level contract fixture that consumes the framework's geometry
and sparkline modules. The live realization is the Rust CLI's mncs-tui-host session: it receives a
structured frame from the monitor view, uses the framework host boundary for raw mode,
alternate-screen/input/resize/diff/cleanup, and never exposes terminal text as the machine API.
The live views currently include an overview, process table, filter/sort/selection/scrolling,
structured process detail, and CPU history. Process graph and richer event panes remain deferred.
