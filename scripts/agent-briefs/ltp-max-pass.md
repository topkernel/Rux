# Agent ltp-max-pass (worktree /home/william/rux-agents/ltp-fix-b, branch feature/ltp-fix-b)

Mission: maximize the LTP pass rate on the 1,869-test sweep list
(goal: 100%). Fix kernel bugs one failure family at a time; every fix
lands as a separate English commit on this branch.

## Where the work stands (2026-10-15)

- main (6bf12a15) already carries many merged LTP fixes (sysv sem ABI,
  /proc/threads-max, libgcc zbb/zcb pin, inotify IN_ISDIR, megapage
  demotion, reparent-safety, FS_IOC_GETFLAGS chattr, net/tcp
  poll-on-listen is on this branch unmerged).
- This branch's unmerged work: `111c8445` fix(net/tcp): poll/select on
  a listening socket reports pending connections.
- **Sibling branch feature/ltp-fix-a** (worktree
  /home/william/rux-agents/ltp-fix-a) has unmerged work you must
  evaluate and pull in — knowledge sharing, not duplication:
  - `940f1c29` fix(mm): slab-backed global allocator + dentry detach —
    kills the heap-exhaustion family
  - `368ac805` wip snapshot (check its diff for in-flight pieces)
  Cherry-pick what is sound onto feature/ltp-fix-b after review, then
  continue from the combined state. ltp-fix-a's worktree then goes idle.

## Harness (already set up in this worktree)

- LTP rootfs: `test/rootfs.img` (1.3GB, intact).
- Sweep driver: `.ltp/drive.py` — boots QEMU per test batch with
  `init=/test/sweep`, walks `.ltp/sweep.list`:
  ```
  python3 .ltp/drive.py --tag <run-tag> --start <idx> [--kernel .ltp/rux-<tag>]
  ```
  Results land in `.ltp/results/`; serial logs `.ltp/serial-<tag>.log`.
- Build (riscv64):
  ```
  echo 'platform_target=riscv64' > build/.config
  cargo build --target riscv64gc-unknown-none-elf
  ```
  Boot with `-cpu rv64,zbb=true,zba=true,zbs=true` per the driver.
- Baseline trend: 35 → … → 850 → 887+ on earlier rounds. Re-baseline the
  combined branch once (full sweep), then work the failure list top-down
  by family.

## Working loop

1. Pick a failure family from the sweep results; find the smallest
   reproducing case; root-cause in the kernel; fix; re-run that test
   in isolation twice (host-load flakes are real: accept02, getrusage04,
   kill02, genfmod, rename/statfs families flaked before).
2. Commit per fix. Before starting any long sweep, commit a
   `wip(snapshot)` so quota death loses nothing.
3. Every ~10 fixes, run a fresh tagged sweep to measure the trend and
   catch regressions; new failures get the 2x isolated rerun rule
   before they count as regressions.

## Rules

- NEVER launch detached test loops (`setsid`/`nohup` + `&`) — a
  detached driver survived its agent once and burned 8 hours of QEMU.
  Run sweeps as tracked background Bash (run_in_background); kill only
  QEMU PIDs you started (never `pkill qemu`).
- Test runs default to `-smp 2 -m 1G`; SMP4/2G only for final
  validation sweeps. Boot shared/rootfs images with `-snapshot` +
  `file.locking=off`.
- Do NOT touch main or other agents' worktrees (reading
  ltp-fix-a to cherry-pick is fine; don't build or commit there).
- English commit messages only.
