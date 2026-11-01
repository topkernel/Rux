# Test Reports

Every test report lives here. A report is a dated snapshot of a measured
result with its rig description; deep methodology docs (the stress
fleet's design, the coverage ledger's mechanics) stay next to their
code when that reads better, but the numbers land in this directory.

## Standing reports

| Report | What it measures | Latest |
|---|---|---|
| [unit-test-report.md](unit-test-report.md) | In-kernel guest suite (make test), per architecture | 1267/1267 riscv64 · 1263/1263 x86_64 · 2026-11-01 |
| [integration-test-report.md](integration-test-report.md) | Multi-leg unattended fleet, probes, CI smoke gate | 2026-11-01 |
| [ltp-report.md](ltp-report.md) | LTP compatibility, both rigs (riscv64 TCG, x86_64 KVM) | 1010/1869 (x86+KVM r21) · 1028/1869 (riscv64 r20) |
| [lmbench-report.md](lmbench-report.md) | lmbench: Rux vs reference kernel, per-case | 2026-11-01 baseline |
| [geekbench-report.md](geekbench-report.md) | Geekbench 5: all 21 workloads, Rux vs reference | SC 1941/2165 · MC 7139/7027 |
| [stress-fleet.md](stress-fleet.md) | Stress/stability/monkey/fuzz fleet design + results | active |
| [formal-verification-report.md](formal-verification-report.md) | Kani / proptest / SPIN / Miri standing | 157 harnesses · 1104 proptests |
| [test-coverage.md](test-coverage.md) | Coverage ledger (generated) | regenerated per change |
| [ltp-tracking.md](ltp-tracking.md) | Live LTP tracker notes | per scan |

## Conventions

- Reports are **dated and append-only in spirit**: a new run appends a
  section (or refreshes the standing table with its date), it does not
  silently rewrite history.
- Every number carries its **rig** (arch, accelerator, kernel profile,
  host load discipline). A number without its rig is not a result.
- Optimization queues in the perf reports (lmbench, geekbench) are the
  working order for the perf lanes — attack from the top.
- Raw records for the fleet live outside git under `run/suite/<stamp>/`
  (host-lifetime artifacts); the report carries the verdict and the
  numbers that matter.
