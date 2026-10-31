# Perf review — syscall entry/dispatch + openat (perf-syscall lane, 2026-10-31)

Method: A/B with the reference kernel bzImage
(/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage)
through the SAME rig — test/pbench_boot.py, q35, `-accel tcg,thread=multi`,
`-cpu max`, SMP2/2G, minimal ext4 rootfs, init=/test/pbench.
All numbers are medians in microseconds unless noted.

## 0. Ground truth (true A/B, both TCG)

IMPORTANT calibration finding: the TASK.md gaps ("p1 15-90x", "openat
50-250x") came from comparing Rux-TCG (or Rux-debug) numbers against
reference-KVM numbers (run/ref-r1/r2, pbref-boot.sh machine class) and
syscost debug-boot numbers. Under a FAIR A/B (same accel, same rootfs,
same probe) the reference itself is ~6x slower than its KVM columns:

| row            | Rux strong base | reference TCG | ratio | reference KVM (ref-r1) |
|----------------|-----------------|---------------|-------|------------------------|
| p1_null        | 0.611           | 0.240         | 2.5x  | 0.040                  |
| p1_clock       | 0.741           | 0.832         | ahead | 0.070                  |
| p2_ctxsw       | 1.377           | 1.518         | ahead | 0.085                  |
| p6_stat        | 18.294          | 3.877         | 4.7x  | 0.271                  |
| p6_openclose   | 32.582          | 6.241         | 5.2x  | 0.431                  |
| p6_read_cached | 40.502          | 12.122        | 3.3x  | 0.671                  |
| p6_append      | 85.811          | 9.377         | 9.2x  | 0.471                  |
| p6_unlink      | 88.164          | 25.216        | 3.5x  | 26.239                 |
| p6_mkdrmdir    | 32352           | 144.1         | 224x  | 40.4                   |
| p7_sleep1ms    | 1215            | 16.5          | 74x   | 14.6                   |

Out-of-lane rows worth flagging to their owners: p6_mkdrmdir 224x
(ext4 create path), p7_sleep1ms 74x (timer), p5b/p5c stream/dgram ~15x
(unix-socket write path), p6_append = per-write journal commit (see F5).

## Fixes (each measured on its own build)

### F1 — gate per-syscall forensic instrumentation (dfx=sysforensic)

syscall_handler/`_inner` carried UNCONDITIONAL per-syscall work: the
SYSCALL_RING entry+exit records (2 RMW + 10 atomic stores), a duplicate
post-translate LASTSYS latch, the per-task last_calls/init_calls/birth
rings, the fail-tail ring, and the sc-series comm-gated hooks
(appioctl/applog/appconn/prctl-einval/missstat/openmiss — the ioctl and
connect ones did comm() copies + 5 string scans on EVERY ioctl/connect).
Also check_and_deliver_signals did two atomic RMWs (xchg) per syscall
RETURN (ISIG latch + CAD latch) — load-first now, RMW only when nonzero.

All of it now sits behind one atomic load (`DfxSwitch::SysForensic`, new
switch `dfx=sysforensic`); the pre-translate LASTSYS latch stays always-on
(taskdump's lastsys= needs it, 3 stores). Hunt boots that need the rings
or sc-hooks must add dfx=sysforensic.

  p1_null 0.611 -> 0.481 (-21%), p1_clock 0.741 -> 0.621. fs rows flat (expected).

### F2 — path_lookup prologue: zero-alloc fast path + walk churn

The prologue paid ~15 heap objects per lookup before the walk started:
make_absolute_rooted's String, THREE get_process_root() Strings (each =
FsStruct::get_root() Vec clone — or a full parent-chain build_path() walk
when a root dentry is recorded — plus path_normalize's TWO Vec<&str>
collects and output String), path_normalize's output, and the
components Vec. Now: `process_rooted()` reads the fs_struct slots with
no alloc; absolute + already-normal paths take a `Cow::Borrowed` path
(`path_is_normalized_fast` = one byte scan); chrooted/odd paths take the
old allocating arms unchanged. The sbx sandbox-trace computation
(4 byte-scans per lookup) is behind sysforensic.

Walk loop: the ENOTDIR check and the DAC check each called
`current.get_inode()` (spinlock + Arc clone) — now one call serves both,
in both walk loops (path_lookup + the symlink-resolution walk).
The icache refresh's `set_inode(fresh)` ran even when the icache returned
the SAME Arc (lock + Arc swap per cached component) — ptr_eq-guarded now.

  p6_stat 18.294 -> 9.395 (-49%), p6_openclose 32.582 -> 24.709 (-24%),
  p6_read_cached 40.502 -> 32.131 (-21%), p6_unlink 88.164 -> 67.817 (-23%).

### F3 — sys_openat shortcut-chain trims

sys_openat's shortcut chain (O_PATH trace, /proc/exe, /proc/*/environ,
ns links, procfs read_file) ran per open: TEN `vfs::chrooted()` calls
(each = get_process_root() = the ~5-alloc stack from F2), TWO
Vec<&str> collects for /proc parsing (ungated by any /proc prefix
check), try_open_ns_path's Vec collect, and procfs::read_file's Vec
collect + scans. All now: `process_rooted()` (0 allocs) + a
`starts_with("/proc")` gate before every /proc-shaped collect.
(The probe opens non-proc paths, so this is pure removed overhead.)

  p6_openclose 24.709 -> 14.380 (-42%), p6_read_cached 32.131 -> 22.396 (-30%),
  p6_stat 9.395 -> 8.277 (-12%).

### F4 — icache init fast path + stats gating (kept, see F4b for the copy part)

icache_init() took the global ICACHE spinlock on EVERY lookup just to
check "already initialized" — fronted with an AtomicBool now. The
hit/miss counter fetch_adds (2 per component lookup) are behind
`icache_stats_arm()` (default off; the LRU time updates stay — eviction
order is functional). The first attempt of this round also switched
strncpy_from_user to a whole-window bulk copy — that REGRESSED
(p6_openclose 14.380 -> 17.503): a PATH_MAX window meant a 4 KiB memcpy
for a 20-byte name. Reverted to chunked (F4b); init/stats kept.

### F4b — strncpy_from_user: chunked copy instead of per-byte get_user

Both arches copied user strings ONE BYTE AT A TIME through get_user —
on x86_64 each byte is a call + access_ok + AC bracket (pushfq/orq/popfq
— the SMAP-era STAC/CLAC replacement) + 1-byte rep movsb + AC clear +
ret ≈ 30 instructions per path byte, paid by every openat/stat/exec/
chdir path argument. Now: 64-byte chunked raw exception-table copies
(one AC bracket pair per chunk) + NUL scan in kernel memory; a
mid-chunk fault zero-fills the tail and returns the same Ok/EFAULT the
byte loop produced. Short names cost ONE chunk; nothing ever copies
more than 63 bytes past the NUL.

## Cumulative (strong profile, TCG, med-of-1 per build; med-of-3 pending)

| row            | base    | fixed   | delta | vs reference TCG |
|----------------|---------|---------|-------|------------------|
| p1_null        | 0.611   | ~0.48   | -21%  | 2.5x -> 2.0x     |
| p6_stat        | 18.294  | 8.277   | -55%  | 4.7x -> 2.1x     |
| p6_openclose   | 32.582  | 14.380  | -56%  | 5.2x -> 2.3x     |
| p6_read_cached | 40.502  | 22.396  | -45%  | 3.3x -> 1.8x     |
| p6_unlink      | 88.164  | 67.8    | -23%  | 3.5x -> 2.7x     |

## Open findings (not fixed in this lane)

F5. p6_append (a single 4K write() to an open fd) = 77-89us vs reference
    9.4us. ext4_write_at_core starts a JOURNAL TRANSACTION and commits
    metadata per write (data=ordered inline discipline) — the ledger's
    known "~88us append class" (route-1/J-arm). The reference kernel does
    delayed allocation + writeback: write() touches no disk I/O. This is
    the fs-lane's route-1 work; naming it here because the charter asked
    for the write path too. p6_write_fsync 19.6ms vs 16.0ms (1.2x, close).

F6. p6_getdents 544us vs reference 882us — we are AHEAD (full-dir sweep
    of 1000 files).

F7. Remaining p6_openclose gap (14.4 vs 6.24): per-component walk locks
    (dentry children lock, inode lock, vfsmount lock, icache global lock,
    follow_symlink's extra get_inode per component) and VFS_STATE's
    global spinlock per lookup. The reference walks dcache RCU-style with
    zero locks. Candidates: bucket-level icache locking, dentry-embedded
    RCU-walkable name hash, VFS root ArcSwap.

F8. Relative-path opens rebuild the cwd from the dentry parent chain on
    every make_absolute (FsStruct::get_cwd -> build_path() — locks + a
    String per ancestor). Probe rows use absolute paths; shells don't.

F9. p1_null residual 2.0x (0.48 vs 0.24): trap entry SCRIBBLE2-GUARD
    (PER_CPU load + stack-bounds check per user entry), CURRENT_PT_REGS
    save/restore pair, seccomp gate, LASTSYS latch, syscost/trace armed
    checks, exit-path signal+resched checks. Each is small; the total is
    the gap. Not worth dismantling the safety nets for the last 2x under
    TCG; revisit under KVM measurements.

## Artifacts

- Kernels: run/sys-{base,fix1,fix2,fix3,fix4,fix5}-bzImage
- Results: run/<tag>-results.txt (PBENCH lines), serial logs in work/
- Reference TCG column: run/sysref-tcg1-results.txt
