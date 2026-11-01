# perf-path lane notes — lmbench stat/open path-lookup gaps

Branch `feature/perf-path` (off main bfaaeffc). Rig: x86_64 + KVM,
strong profile (`make -C build build-release-opt PLATFORM=x86_64`),
QEMU q35 `-smp 2 -m 2G`, image `work/lmbench-x86.img` (private copy of
the shared lmbench x86 image; the guest runner got a `budget=` cmdline
knob this lane — benchmarks themselves untouched).

## 1. The standing table's rows are stale — fresh baseline first

`docs/test/lmbench-report.md` lists stat 18.77x / open 13.88x from the
2026-11-01 sweep, but that sweep ran under host loadavg 10-19 (void by
the report's own validity rule) and main has since landed the
perf-syscall lane's path work. Re-measured on 2026-11-01 at host load
3.5-4.3, med-of-5, one case per boot:

| Case | base (bfaaeffc) | reference (native, same host) | ratio |
|---|---|---|---|
| lat_syscall stat | 1.041 us | 0.213 us | 4.9x |
| lat_syscall open | ~1.71 us (noisy, load 4-5.3) | 0.395 us | ~4.3x |

Reference numbers this session (3x each, load <4): stat 0.212-0.223,
open 0.390-0.401, fstat 0.104-0.109, read 0.0645-0.0649, null
0.0386-0.0395 — all match the standing report's column, so that column
stays valid; the Rux column did not.

Honest-comparison note (mission item): the guest's stat target is the
injected 33-byte placeholder at /usr/include/linux/types.h
(build-rootfs.sh), the reference column stats the host's real 1829-byte
file — both small, cached, regular files; equivalent work.

## 2. Attribution — fsw=1 pathcost probe (commit fc071225)

New PC counters (NS sums; each bracket pair adds ~20-30 ns of its own
clock-read cost to its phase — read phase numbers as upper bounds, the
ranking and the aggregate split are solid).

stat boot (lat_syscall,stat; 2.39M stats at dump): 1030 ns/call total —
- syscall prelude (2 user-copy probes + resolve_user_path + gates):
  ~135 ns
- walk prologue (root fetch via global VFS_STATE lock, normalize,
  components Vec alloc): ~181 ns
- component loop: ~733 ns = 4.0 components x ~150 ns
  (A get_inode+perm 52.8, B lookup_child 42.6, C icache refresh 8.7,
  D mount+symlink 46.7; loop machinery ~130 ns)
- walk tail (final get_inode + trailing-slash checks): ~32 ns
- getattr + copyout: ~90 ns (measured as stat-TAIL minus walk)

open boot (lat_syscall,open; 1.73M opens at dump): prelude ~290 ns
(resolve + 5 chroot-gate probes, each an fs Arc clone), dispatch
(file_open) ~1084 ns of which the walk is ~930 ns; close (unbracketed)
~300-400 ns — lmbench's open case times open+close.

Root cause class: per-component spinlock pairs + Arc refcount churn
(each Spinlock::lock does preempt bookkeeping + CAS; each Arc
clone/drop is an atomic RMW pair), plus a global-lock icache re-check
per dcache hit and a heap Vec + String per lookup. The reference kernel
walks the hot dcache under RCU with no locks at all.

## 3. Fixes (commit 5542a7b6)

1. `Dentry.has_mount()` — lock-free mirror of the vfsmount slot; the
   per-component mount test and follow_mount skip the vfsmount lock +
   Option<Arc> clone when there is no mount (the 99.9% case).
2. Carried inode: the walk reuses the previous iteration's child inode
   for the parent dir+x checks — removes one dentry-inode lock pair +
   Arc churn per component.
3. icache identity generation: global `ICACHE_REGEN` bumped on every
   membership change (add/remove/flush); dentries stamp the generation
   their inode was verified at. Stamp match = cached Arc is current =
   skip the global-lock icache_lookup. The old refresh semantics (swap
   in the icache's object when identity changed) are preserved via the
   slow path.
4. Component split on the stack ([&str; 16] + spill) — no heap Vec per
   lookup.
5. follow_symlink_hinted borrows the inode hint (no Arc clone) for the
   is_symlink test.
6. sys_fstatat / sys_openat: ONE 128-byte NUL-gated user copy serves
   the magic-link probe, the empty-path probe and (absolute path,
   non-chrooted) the lookup itself — drops the second copy_from_user
   and the String alloc. Paths >127 bytes fall back to
   resolve_user_path (never walked truncated — NUL gate).
7. sys_openat computes the chroot verdict once (was 5 probes, each an
   fs Arc clone + RwSpinlock read).
8. FdTable::install_lowest_fd — alloc+install in one irqsave
   entry-lock section (was two).

## 4. Results

Interleaved base/fixed boots (run/abx-base-fixed.txt; one case per boot,
alternating sides per rep so host-load drift hits both equally; 5 reps;
host load 3.5-9.8 during the sweep — every pair's per-boot load is in
the file; 3 of 60 boots hit in-guest budget timeouts and are excluded
as NONE):

| Case | base median | fixed median | delta | ratio vs reference |
|---|---|---|---|---|
| lat_syscall stat | 1.0669 us (n=4) | 0.6508 us (n=4) | **-39.0%** | 5.01x -> 3.06x |
| lat_syscall open | 1.7727 us (n=5) | 1.2221 us (n=5) | **-31.1%** | 4.49x -> 3.09x |
| lat_syscall fstat | 0.1071 us | 0.1081 us | +0.9% | flat (no path) |
| lat_syscall read | 0.1475 us | 0.1482 us | +0.5% | flat (no path) |
| lat_syscall write | 0.1279 us | 0.1262 us | -1.3% | flat (no path) |
| lat_syscall null | 0.0706 us | 0.0726 us | +2.8% | flat (noise band; |
| | | | | ran under the sweep's |
| | | | | highest load, 7.4-9.2) |

Reference (native, quiet host, 3x each): stat 0.213, open 0.395, fstat
0.107, read 0.0647, null 0.039 us.

Probe re-run on the fixed kernel (fsw=1 stat boot, 3.48M stats) — the
phase deltas behind the win:

| Phase (ns/call, probe-inflated) | base | fixed |
|---|---|---|
| stat syscall prelude | 135 | 77 |
| walk prologue | 181 | 97 |
| component loop (4 comp) | 733 | 512 |
| — A get_inode+perm (per comp) | 52.8 | 38.1 |
| — B lookup_child (per comp) | 42.6 | 38.9 |
| — C icache refresh (per comp) | 8.7 | 8.4 |
| — D mount+symlink (per comp) | 46.7 | 8.7 |
| walk tail | 32 | 24 |

Next targets, in order, if this lane continues: the per-component
`lookup_child` (BTreeMap under a spinlock — now the top per-component
cost at ~39 ns), the walk-start VFS_STATE global lock (task-fs root
read instead), and the close side of the open case (~300-400 ns:
get_file + park/drain lock pairs + File free).

Functional probe (userspace/lmbench/pathprobe.c, injected as
init=/test/pathprobe, both kernels): 18/18 PASS — hot stat/open, 200x
open/close, lowest-fd discipline, ENOENT, symlink follow/lstat/
relative/chained, /proc magic link + mount traversal, trailing-slash
ENOTDIR, dot/dotdot, 20-component deep path (stack-array spill),
>127-byte long path (NUL-gate fallback), fstatat(dirfd), unlink/recreate
(icache identity regen), open/close churn.

## 5. Gates

- Dual-arch build: x86_64 build-release-opt rc=0; riscv64 `make build`
  rc=0; aarch64 `make build PLATFORM=aarch64` rc=0.
- make test (riscv64, this tree): suite verdict green — 1278 passed /
  0 failed (a second standalone boot of the same test kernel: 1279
  passed / 0 failed). Post-suite VW-WEDGE watchdog traces appear while
  the halted guest lingers under host load 8-9; they are idle-phase
  traces, not suite failures.
- riscv64 regression boot (run/rv-smoke-perfpath.sh, minimal rootfs,
  scripted serial): shell reached, `cd /proc` + `ls` + done-mark echoed,
  zero panics/KERNPANIC/ALLOCTHROW. Base-tree control boot: identical.
- Note for the merge gate: the lmbench guest runner in THIS worktree's
  image copy carries the `budget=` knob (upstreamed in the lmbench_run.c
  commit); the shared store image and the report rig are unchanged.
