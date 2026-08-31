# Examples

Examples and integration fixtures hold deterministic host fixtures and small projection probes. They should describe
the source, units, timing, privilege assumptions, and expected uncertainty rather than capturing a
machine-specific live process list.

The Linux collector's repository-local fixtures cover host/process counters, rates warming on the
first sample, disappearing processes, PID reuse, malformed input, and unavailable I/O. The live
collector remains Linux-specific and retains partial-source uncertainty in the snapshot.
