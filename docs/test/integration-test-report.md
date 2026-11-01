# Integration test report — fleet, probes, CI gate

**Rig**: `test/suite.sh` — the unattended multi-leg fleet (modes:
unit / formal / probe / perf / stress / stability / monkey / fuzz /
integration; arch riscv64 / x86_64 / both; `--daemon` for quota-death
coverage, records under `run/suite/<stamp>/`). Per-change integration
evidence is the **A/B probe** discipline: a guest C probe under
`test/` (packed in `test/probes.tar.gz`) demonstrates the failing
behavior before a fix and the passing behavior after, on the same
image.

## Standing (2026-11-01, main 09d2969c)

- Guest suite legs green on both arches (see unit-test-report).
- `make verify` green: mirror sync + 1104 host proptests.
- CI smoke gate live on every push: three-architecture kernel builds +
  host verification + full riscv64 guest suite (`.github/workflows/
  smoke.yml`); heavyweight formal jobs (Kani/Miri/SPIN) moved to the
  nightly schedule so per-commit signal is fast and real.
- Recent probe-verified fixes landed through integration: socket
  sockaddr write-back clamps (`sockaddr_clamp` probe, 6/6), SMAP bad
  user-pointer EFAULT across 11 syscalls (`uafault` probe), loopback
  TCP nettest legs (close-delivery, echo) green at smp1/smp2.

## Fleet notes

The fleet's standing limitation is environmental, not functional:
perf-class legs skip when the host carries more than two foreign QEMUs
(noise dressed as data), and full-mode sweeps run under quota death by
design (that is what the daemon mode is for — records land and get
adjudicated on resume). The adjudication rules (isolation rerun 2x
before calling a regression; known host-load flake families) live in
the repo root AGENTS.md and in `test/patrol-runbook.md`.

## Known open integration-level items (2026-11-01)

- Loopback TCP is green again (RX checksum fix); the nettest probe
  stays in the pack as the regression tripwire.
- `lat_sem`-class SysV semaphore blocking race (EAGAIN instead of
  blocking) — fix lane in flight; see lmbench-report queue.
