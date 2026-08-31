# TUI application boundary

The future human-facing monitor belongs here as an application built on
[`mncs-tui`](https://github.com/epi13/mncs-tui).

This directory must consume the monitor snapshot and the TUI framework's authoritative geometry,
layout, widget, event, focus, frame, diff, and terminal contracts. It must not grow a second table,
layout, terminal, or ANSI semantic implementation while the upstream framework evolves.

The initial views are Overview, Processes, Process graph, and Events/pressure. They are intentionally
not implemented yet; this boundary exists so the first UI work has an obvious owner.
