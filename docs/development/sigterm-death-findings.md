# FINDINGS: gnome-session SIGTERM death + /bin/ls segfault (fire-gate-merged-r1)

Branch: `feature/gnome-integration` (worktree `/home/william/rux-agents/gnome-int`)
Fixes: `bd095d0e` (R82 mm ordering), `78b6b830` (SI_TKILL siginfo + KILLTRACE)
Verification: `/home/william/rux-agents/gnome-img/fire-sigterm-r1/` (814s, MTTCG smp4/2G),
plus a targeted userspace ABI probe (`/tmp/sigtest.c`, serial: `/tmp/sigprobe-serial.log`).

## TL;DR

Both symptoms share ONE root cause: **R78 (commit 20e47277) put the exception-table
fixup BEFORE the VMA demand-fill in the riscv64 kernel-mode page-fault path**, so
every kernel `copy_to/from_user` fault on a resolvable page (not-yet-faulted-in or
COW) returned **EFAULT** instead of faulting the page in.

- The `si_pid=0/si_code=SI_USER` on the gnome-session death was an artifact, as
  suspected: the SIGTERM came through **tkill/tgkill** (glibc `raise()` — the only
  SIGTERM send path that queued no siginfo; kernel-internal SIGTERM senders are just
  the reboot-to-pid-1 path). It was **gnome-session's own failure-path suicide** after
  the EFAULT plague killed dbus-daemon. Now attributed: `SIGDEATH ... si_pid=<self>
  si_code=0xfffffffa` (SI_TKILL) and a KILLTRACE line naming sender+origin at send time.
- `/bin/ls` segfaulted because `statfs`/`fstatat` returned EFAULT (kernel could not
  copy results / read the path), so ls consumed an **uninitialized stat buffer** and
  dereferenced garbage (`0x10102464c457f`). With the fix, `ls -la /tmp/.X11-unix`
  exits 0.

## Evidence chain (original failures, fire-gate-merged-r1)

Serial (`grep -a`, log has NUL bytes):

```
EFAULTTRAC: pid=318 nr=43 a0=0x300001c6d0 a1=0x3fffffe8f0     <- statfs EFAULT
EFAULTTRAC: pid=318 nr=43 a0=0x300001c6e0 a1=0x3fffffe8f0
EFAULTTRAC: pid=318 nr=79 a0=0x4 a1=0x3000132688               <- fstatat EFAULT
EFAULTTRAC: pid=318 nr=79 a0=0xffffffffffffff9c a1=0x3000134c48
pagefault: Segfault at 0x10102464c457f, epc=0x30000f88ba, ..., pid=318, mode=user
SIGDEATH: pid=318 tgid=318 comm=Ok("ls") sig=11 ppid=1 si_pid=0 si_code=0x0
...
EXITNOTIFY: child=334 parent=330                                 <- gnome-session-failed exits
SIGDEATH: pid=330 tgid=330 comm=Ok("gnome-session-b") sig=15 ppid=321 si_pid=0 si_code=0x0
SIGDEATH: pid=332 tgid=330 comm=Ok("gmain") sig=9 ppid=321 (thread)  <- group teardown
```

Guest logs extracted from the image (debugfs dump of `/root/*.log`) tell the rest:

```
dbus-daemon: fatal error setting up standard fds: Failed to open /dev/null: Bad address   <- BOTH buses
** (process:330): WARNING ... Could not connect: No such file or directory                 <- no session bus
XIO:  fatal IO error 14 (Bad address) on X server ":0"                                     <- EFAULT on X socket IO
(gnome-session-failed:334): GLib-WARNING ... getpwuid_r(): failed due to: Bad address.     <- NSS read EFAULT
** (gnome-session-failed:334): WARNING ... Cannot open display:
```

So dbus-daemon died at startup (opening `/dev/null` — a `.rodata` string literal page
whose FIRST touch was the kernel's `strncpy_from_user`), gnome-session ran its
failed-dialog path, the dialog died too, and gnome-session killed itself with SIGTERM.
The exact same log signature exists in all three sched-bisect runs
(`/home/william/rux-agents/gnome-sched2/runs/b1-nodbus,b2-nor79,b3-nor77` — all
contained R78), and does NOT exist in the stock-main reference run. That is why the
bisect could not exonerate R78 "individually": every arm had it.

## Root cause (code)

`kernel/src/arch/riscv64/mm/exception.rs`, `do_page_fault()` kernel-mode block.
R78 inserted an unconditional `fixup_exception(regs.epc)` at the TOP of the block.
The `kernel_data_fill` branch (added Sep 29, `dee62756`) — which demand-fills
VMA-covered user pages for kernel accesses, exactly like Linux `do_user_addr_fault`
-> `handle_mm_fault` before considering the fixup — became unreachable for uaccess
faults. The x86_64 twin had already fixed this exact ordering and documents it as
the "X3 fix" (`kernel/src/arch/x86_64/mm/exception.rs`, "do NOT fixup uaccess-window
kernel faults here").

What turns into a spurious EFAULT: first kernel touch of any untouched page
(string literals in .rodata, fresh stdio/NSS buffers, brk/stack growth) and COW
pages during kernel copies.

R78's own goal (don't PANIC on wild-address uaccess faults under the R74
double-run corruption) is preserved: the demand-fill only handles VMA-covered
addresses, and the fixup still runs BEFORE the stack-overflow panic.

## Fixes

1. **`bd095d0e` — R82 demand-fill before fixup (riscv64)**: kernel-mode faults at
   user addresses (non-EXEC) covered by a VMA are resolved via `handle_mm_fault`
   (now including `CowPending` -> `handle_cow_fault`, mirroring the user-mode path)
   and the instruction retried. Wild/uncovered addresses fall to the fixup first,
   then the R74/R78 stale-context + stack-overflow containment, unchanged.
   Also queues real siginfo (SEGV_MAPERR/SEGV_ACCERR) on the fault-signal path so
   SIGDEATH distinguishes hardware faults (si_code 0x1/0x2) from kills.
2. **`78b6b830` — SI_TKILL siginfo + KILLTRACE widening**:
   - `sys_tkill`/`sys_tgkill` now queue `si_code=SI_TKILL(-6)` with the sender's
     pid/uid (Linux `do_send_specific`). glibc `raise()` is tgkill, so a raise()d
     fatal signal is attributable at delivery. This is also an LTP-visible ABI gap
     closed (kill(2) already queued SI_USER via `send_signal_from_user`).
   - KILLTRACE covers every SIGTERM, cross-group SIGKILLs, and SIGSEGV/SIGKILL to
     `gnome-session*`/`ls` targets: prints sender pid+comm, target, and an origin
     tag (SI_USER/SI_TKILL/SEGV_*/SI_KERNEL/none). Same-group SIGKILL teardown noise
     still skipped; rate limit 300.

## Verification

### fire-sigterm-r2 (confirmation run, full 901s, no panic)

Second identical run: **zero SIGDEATH, zero EFAULTTRAC, zero KILLTRACE lines for the
entire 901s**; gnome-session-b and both dbus-daemons alive at t=898s (vruntime tails
in summary.txt). This run was slower than r1 (gnome-shell did not spawn within the
window; Xorg softlockup @843s went D-state — the known stall landscape, unrelated to
signal/EFAULT). Clean end at the driver timeout.

### fire-sigterm-r1 (900s budget, stopped 814s by a panic — see below)

- `/bin/ls -la /tmp/.X11-unix`: **st=0**, zero EFAULTTRAC lines (was: 4 EFAULTs + segfault).
- Both dbus-daemons alive the whole run; `/root/dbus-session.log` empty (was: fatal
  `/dev/null` Bad address).
- **gnome-session-binary never died** — no SIGDEATH, no SIGTERM; alive and RUNNING
  from t=132s to the end. (Before: SIGTERM death at t=142s.)
- Chain milestones reached for the first time in this chain: `gnome_shell @223s`,
  `gsd @342s`, `migrations M1 @232s`, `M2 gsd_daemon @348s`, `mutter_wm @429s`,
  `xterm_x_ok @526s`. Pages in use grew to 158k (633MB) — the real desktop stack
  is running; no fake OOM.
- SIGDEATH sender attribution works live:
  `KILLTRACE: pid=331(dbus-daemon) -> pid=384(dbus-daemon) ... sig=9 origin=0x0`
  then `SIGDEATH: pid=384 ... si_pid=331 si_code=0x0` (a kill(2) sender, correctly shown).

### ABI probe (static binary, injected into an image copy, 1-shot boot)

```
KILLTRACE: pid=304(sigtest) -> pid=305(sigtest) ... sig=15 origin=0xfffffffa
HANDLER sig=15 si_pid=304 si_code=-6        <- tgkill: SI_TKILL + sender, as Linux
KILLTRACE: pid=304(sigtest) -> pid=306(sigtest) ... sig=15 origin=0x0
HANDLER sig=15 si_pid=304 si_code=0         <- kill: SI_USER + sender
KILLTRACE: pid=307(sigtest) -> pid=307(sigtest) ... sig=15 origin=0xfffffffa
SIGDEATH: pid=307 ... sig=15 si_pid=307 si_code=0xfffffffa   <- raise() suicide attributed
```

Applied to the original incident: a recurring gnome-session SIGTERM will now print
`KILLTRACE: pid=<X>(<comm>) -> pid=<gs> ... sig=15 origin=<code>` at send time and
`SIGDEATH ... si_pid=<X> si_code=<code>` at death; origin `0xfffffffa` (SI_TKILL)
means glibc raise()/tgkill (self or thread), `0x0` (SI_USER) means kill(2), `none`
means a kernel-internal info-less path (timer/mq/pdeathsig — none of which send
SIGTERM today except reboot-to-pid1).

## Remaining issues (NOT addressed here, per task scope)

- The r1 run stopped at t=814s with `trap: KERNPANIC pfault badaddr=0x5b1000002b0`
  preceded by a duplicate `SIGDEATH ... pid=397 (gsd-smartcard)` (two harts exiting
  the same task), `SIGILL-FORENSIC ... STAMPED pfn=... owner_pid=394` and a garbage
  `noctx pid=3773592849` — the known R74 slotless double-run family under full-session
  load, contained by R80 and explicitly a separate workstream. This run reached
  t=812s of full GNOME load before hitting it (the pre-fix gate never got past the
  dead session at t=142s, so it never loaded the machine this hard).
- gsd components fail to register before timeout (gnome-session.log warnings) — a
  later-stage D-Bus/speed issue, not a death.

## Run evidence paths

- Before: `/home/william/rux-agents/gnome-img/fire-gate-merged-r1/` (serial.log,
  summary.txt; guest logs dumped via debugfs)
- Bisect runs showing the same signature: `/home/william/rux-agents/gnome-sched2/runs/{b1-nodbus,b2-nor79,b3-nor77}/`
- After: `/home/william/rux-agents/gnome-img/fire-sigterm-r1/` and
  `/home/william/rux-agents/gnome-img/fire-sigterm-r2/` (serial.log, summary.txt)
- ABI probe: `/tmp/sigtest.c`, `/tmp/sigprobe-serial.log`
