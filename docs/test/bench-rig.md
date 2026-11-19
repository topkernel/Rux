# Benchmark rig book — how to get numbers FAST

*The 3-minute rule (user directive, 2026-11-02): a test request should
produce numbers in ≤3 minutes. This page is the playbook.*

## The assets (NEVER delete)

Permanent images at `/home/william/rux-images/testimages/` (reflink-
copy into your worktree's `work/`; they are boot-ready — no rebuild,
no debugfs injection):

| Image | Contents | Boot |
|---|---|---|
| `lmbench-x86.img` (700 MB) | Ubuntu rootfs + 24 lmbench binaries + PID-1 runner + types.h placeholder | `init=/test/lmbench_run lmbench=<case>` on the cmdline |
| `ltp-riscv64-rootfs.img` (1.3 GB) | minimal rootfs + LTP suite + ltpscan runner | `init=/test/ltpscan` |
| `geekbench-x86.img` (1.8 GB) | Ubuntu rootfs + Geekbench 5.5.1 + capture rig | per the geekbench lane's runner |

Kernel: `make -C build build-release-opt PLATFORM=x86_64` once, copy
`target/x86_64-unknown-none/release/bzImage` to `work/bzImage-lm` and
KEEP it — rebuild only when the kernel actually changed.

## Quick mode (the default answer): `userspace/lmbench/quick.sh`

- 10 headline cases, 3 boots parallel + serial retry, 30 s/case budget
- **Wall time 1.5–2.5 min**, no load gate (output marked INDICATIVE;
  deltas > 10 % are real, smaller is noise)
- `./userspace/lmbench/quick.sh` → prints the table, saves
  `run/lmbench-quick.txt`
- **Load dependency (physics, not a rig defect):** at host load < 5
  all 10 rows land; at load > 7 boots starve (a KVM boot needs ~8–12 s
  of a quiet host) and rows drop. The script reports exactly which
  rows starved. When the parallel session's lanes saturate the host,
  run `quick.sh` anyway for the rows that land, or wait for their boot
  gap.

## Standing mode (report refresh): `run-rux.sh` / `run-linux.sh`

For docs/test/lmbench-report.md refreshes: all 28 cases, per-case
boots, med-of-runs, quiet-window discipline (load ≥ 6 voids a run).
Budget ~40–60 min of mostly-waiting. **Never block a user question on
this mode** — answer from quick.sh, refresh the standing when a quiet
window exists.

## Geekbench

Inherently a **15-minute-class** run (21 workloads × 2 sections in the
guest) — there is no quick mode. Rerun only for report refreshes; the
standing table carries the last dated result. Boot: -smp 4 -m 4G KVM,
strong kernel, capture rig per `perf-geekbench` lane's scripts.

## What made refreshes slow before (the recorded lessons)

1. **Rebuilding the image every time** — solved: testimages/ images
   are permanent; `cp --reflink=auto` is instant.
2. **Rebuilding the kernel every time** — keep `work/bzImage-lm`;
   rebuild only after kernel commits.
3. **Waiting for load < 6 to start at all** — wrong default. Quick
   mode runs at any load and reports what landed; only standing
   medians need quiet.
4. **Per-case serial boots × 28** — quick mode is 10 cases, parallel
   3-up.
5. **The agent re-derived the rig each session** — this page + the
   kept images are the fix.
