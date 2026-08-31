# Examples

Examples will hold deterministic host fixtures and small projection probes. They should describe
the source, units, timing, privilege assumptions, and expected uncertainty rather than capturing a
machine-specific live process list.

The bootstrap has no live-data fixture yet. The Linux collector returns `UNKNOWN` until bounded
`/proc` and `/sys` parsing is implemented and covered by repository-local fixtures.
