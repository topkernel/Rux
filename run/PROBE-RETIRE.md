# probe-retire: hunt-print retirement for the ub2404 boot console

Branch `probe-retire` off main @ 69167992d. Worktree
`/home/william/rux-agents/probe-retire`.

Problem (user-visible): booting the Ubuntu 24.04 desktop flooded the
serial console with hunt diagnostics that were merged in unconditional
and never retired. This lane inventories every such print, retires it
per the repo policy (dfx switch table is FROZEN — no new switch names;
reuse existing gates), and verifies on a real 24.04 boot.

Policy applied (three buckets):

1. **gate** — hunt instruments moved behind an EXISTING dfx= switch
   (`ohhunt` master gate for the OH/ub2404 bring-up family, `sigtrace`
   for signal forensics, `schedtrace` for scheduler forensics,
   `epolltrace` for epoll hunts, `futextrace` for futex hunts,
   `sfddbg` for the signalfd probes). Budgets kept.
2. **keep-budgeted** — genuine error paths a product should keep, cut
   to ONE budgeted line family (first-N, N=8 unless noted).
3. **delete** — path-resolution diagnostics whose information the
   syscall's own errno already carries.

## Inventory (site → print → decision)

| Site | Print family | Before/boot | Decision | Gate/budget |
|---|---|---|---|---|
| fs/procfs/mod.rs:1235 | `NOBLEDBG free read_file(...)` | 1097 | gate | dfx=ohhunt, ≤256 |
| syscall/file.rs:4722 | `NOBLEDBG openat2 shortcut path=` | (in 1097) | gate | dfx=ohhunt, ≤64 |
| net/unix.rs:1533 | `UNIXCONNECT: /run/...` | 338 | gate | dfx=ohhunt, ≤64 |
| net/unix.rs:1397 | `UNIXBIND: /run/...` | 15 | gate | dfx=ohhunt, ≤64 |
| signal.rs:3428 | `KILLTRACE: pid=..(name) -> pid=..` | 4 | gate | dfx=sigtrace, ≤300 (existing) |
| process/exit.rs:1567 | `SIGDEATH: pid=.. comm=.. sig=..` | 18 | gate | dfx=sigtrace |
| syscall/misc.rs:5322 | `SFDDBG: pid1 read sfd consumed sig=` | 74 | gate | dfx=sfddbg, ≤100 (its two sibling arms in this file were already gated; this one was the stray) |
| syscall/dispatch.rs:934 | `EFAULTTRAC: pid= nr= ..` | 37 | gate | dfx=ohhunt, ≤60 |
| syscall/dispatch.rs:954 | `ENOSYS-TRACE: pid= nr= ..` | 2 | gate | dfx=ohhunt, ≤200 |
| syscall/file.rs:665-765 | `open ENOENT: '..' (pid ..)` (3 arms + dedup maps) | 140 | **delete** | errno already tells the story |
| syscall/file.rs:1817-1842 | `readlink fail(..) '..'` | 47 | **delete** | same |
| syscall/file.rs:305 | `o-path open: '..'` | 19 | gate | dfx=ohhunt, ≤20 |
| syscall/file.rs:1562 | `mkdirfail '..' mode ..` | 17 | keep-budgeted | first 8 (was 300) |
| syscall/file.rs:3088 | `umountfail '..' flags ..` | 54 | keep-budgeted | first 8 (was unbounded) |
| syscall/file.rs:2831 | `bindmount .. -> ..` (failures) | 9 | keep-budgeted | first 8 (was unbounded) |
| syscall/file.rs:2923 | `mount[n] .. -> .. type ..` (success tap) | 165 | gate | dfx=ohhunt, ≤128 |
| syscall/file.rs:2923 | `mount[n] ...` (failures) | (in 165) | keep-budgeted | first 16 |
| syscall/file.rs:2734 | `remount[n] '..' flags ..` | 116 | gate | dfx=ohhunt, ≤120 |
| syscall/file.rs:2685 | `remount fail '..' -> ..` | 0 | keep-budgeted | first 8 (was unbounded) |
| syscall/file.rs:2572 | `MSMOVE[n] .. -> ..` | 21 | gate | dfx=ohhunt, ≤60 |
| syscall/file.rs:2498 | `mnt-precheck-fail MS_MOVE ..` | 0 | gate | dfx=ohhunt |
| syscall/file.rs:7186 | `movemount[n] .. -> ..` (success) | 0 | gate | dfx=ohhunt, ≤120 |
| syscall/file.rs:7135 | `movemount-enoent target ..` | 0 | keep-budgeted | first 40 (existing, cold) |
| syscall/file.rs:7759 | `movemount-fail[n] ..` | 0 | keep-budgeted | first 8 (was 160) |
| syscall/file.rs:7790 | `mount_setattr-fail[n] ..` | 9 | keep-budgeted | first 8 (was 160) |
| syscall/file.rs:7941 | `mount_setattr[n] set=.. clr=..` | 71 | gate | dfx=ohhunt, ≤80 |
| syscall/process.rs:5566/5656 | `pivot_root: arg=..` / `new root ..` | 22 | gate | dfx=ohhunt |
| syscall/process.rs:5528/5538 | `pivot_root EINVAL(..)` | 0 | keep-budgeted | first 8 each (was unbounded) |
| process/fork.rs:332-359 | `clone: CLONE_* requires/incompatible/unknown flags` | 0 on 24.04 (LTP-visible) | keep-budgeted | first 8 family-wide via `clone_warn_budgeted()` helper |
| syscall/io.rs:480 | `stderr[n] pid n: <userspace stderr>` | 187 | gate | dfx=ohhunt (Linux never copies userspace stderr onto the kernel console) |
| net/netlink.rs:989 | `UEBIND pid= groups=..` | 19 | gate | dfx=ohhunt, ≤32 |
| net/netlink.rs:1188 | `UEBCAST[n] listeners=..` | 26 | gate | dfx=ohhunt, ≤80 |
| syscall/process.rs:1350 | `UEEXEC[n] pid= path=..` | 26 | gate | dfx=ohhunt, ≤80 |
| syscall/misc.rs:2716 | `UEEPCTL[n] pid= op=..` | 3 | gate | dfx=ohhunt, ≤120 |
| syscall/misc.rs:3959/4200 | `EPPROBE round= nfd=..` (pid-1 epoll probe) | 22, unbounded (every 500 rounds forever) | gate | dfx=epolltrace |
| syscall/misc.rs:4034 | `UESTARVE[n] .. maxevents=..` | 0 | gate | dfx=epolltrace, ≤80 |
| process/task.rs:3571/3610/3630/3836/3994 | `WAKE-REFUSE-PT1..PT5` (+null-deref fixed in PT1) | 13 | gate | dfx=schedtrace, ≤12 each (heal/resched logic unchanged) |
| process/task.rs:3796 | `W2-WAKE-REFUSED-SLEEPING` + MONOPOLIST-CAPTURE | 0 | gate | dfx=schedtrace (census counters stay always-on) |
| sync/futex.rs:11729 | `freeze_scan` → `rwfrz:N RWFRZ:` rows, `RWFRZ-HIGH/MEM/WAITER`, park dumps (self-arms on glib/udisksd mutex contention) | 150 in settled phase | gate | dfx=futextrace (whole scan early-returns; `rwfrzwatch=` cmdline pre-arm still works) |
| sync/futex.rs:8089 | RWFRZ exit-watch latch (`udisksd` contention) → exit.rs RWFRZ-THREADEXIT | (arms the above) | gate | dfx=futextrace |

Kept deliberately (crash-class, existing budgets, not hunt noise):

- `USEGV-FOR` segfault forensic decodes (arch/x86_64/trap.rs, riscv64
  exception.rs) — 47 lines = real guest SIGSEGVs (avahi-daemon family);
  a product must show crash origins.
- `WAKE-WILD-PTR` / `WAKE-POISONED-TASK` raw putchar guards — defended
  wild-pointer drops, never fired on these boots.
- BMSSCAN/DRI/paramws/apptap/devopen families in file.rs — already
  gated behind `oh_hunt_prints_on()` / `dfx=datatap` / pid filters, and
  OH-image-only (0 lines on 24.04).

## Verification (ub2404.img, x86_64 q35 KVM SMP8 8G, release build)

Same rig both sides: `scripts/test/run-x86-ub.sh console` with
`KERNEL_ELF/BZIMAGE` env overrides pointing at
`target/x86_64-unknown-none/release/` (the script hard-coded the debug
paths before; this lane added the env-override support — behavior
unchanged when the vars are unset).

| Metric | baseline @69167992d | final (this lane) |
|---|---|---|
| serial lines to `ub2404 login:` | 3064 | 576 (5.3x fewer) |
| serial lines, first ~9 min (settled) | 4571 @25 min, still growing | 1062 |
| `Reached target getty.target` | yes (line ~3060) | yes |
| `Reached target multi-user.target` | **never** (log cut at 25 min) | **yes** |
| `Reached target graphical.target` | no | **yes** |
| kernel-side hunt lines after login | continuous (EPPROBE every 500 rounds, RWFRZ ~150, NOBLEDBG ...) | 0 |
| login-prompt latency | ~5-6 min | ~80 s |

A second unmodified-baseline boot (variance check) produced the same
flood shape (NOBLEDBG 1783, UNIXCONNECT 428, SFDDBG 100) and never
reached the login prompt within 14 minutes — the print flood itself
(busy-poll UART at KVM speed) was a boot-time cost.

Top remaining kernel-side lines in the final boot (to login): the
one-time boot table (smp/fpu/arch, ~30 lines), `USEGV-FOR` crash
decodes 47 (real guest crashes), budgeted error paths umountfail 8 /
mount_setattr-fail 8 / bindmount 8 / mkdirfail 7 / mount-fail 4. The
rest is the guest's own systemd output.

Failure-set check (no new failures introduced): the final boot's
failed units (timesyncd, resolved, NetworkManager, power-profiles,
gdm, polkit — all pre-existing) are a subset of the unmodified
baseline's variance (baseline run 2: gdm x5, power-profiles x10,
logind x10, resolved x5). dev-mqueue.mount succeeded in both retired-
kernel boots. Logs: `run/serial-baseline*.log`, `run/serial-final.log`.

## Gates

- x86_64 `make build` rc=0; aarch64 `make build` rc=0; riscv64 `make
  build` rc=0 (logs `run/gate-x86_64-build.log`, `run/gate-aarch64-build.log`,
  `run/gate-riscv64-build.log`). Note: `make build` regenerates the
  gitignored `kernel/src/dfx/ksyms.bin` (+3.2 MB kernel image); removed
  before the release A/B so both sides carry the placeholder table.
- riscv64 `make test` — **1329/1329 passed, 0 failed** (log
  `run/gate-riscv64-test.log`; suite count grew past the 1096 in
  AGENTS.md on newer main).
- Perf note: the change is print-path only; no hot-path logic changed.
  The gated checks are relaxed atomic loads on error/refusal arms.
  Tier-1/2 syscost+pbench numbers are the coordinator's merge gate; the
  removed prints were pure UART cost on the measured boots.

## Switch used to re-arm everything retired here

`dfx=ohhunt` re-arms the OH/ub bring-up family (NOBLEDBG, UNIXCONNECT/
BIND, mount taps, uevent family, o-path, stderr relay, EFAULTTRAC,
ENOSYS-TRACE, pivot traces); `dfx=sigtrace` KILLTRACE+SIGDEATH;
`dfx=sfddbg` the SFDDBG triple; `dfx=schedtrace` WAKE-REFUSE/W2;
`dfx=epolltrace` EPPROBE/UESTARVE; `dfx=futextrace` the RWFRZ freeze
machinery. No new switch names were added (table is frozen).
