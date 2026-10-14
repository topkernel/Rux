# gnome-int — integration branch findings (merged 2026-10-13 night)

This branch merges feature/gnome-cpu-storm (event-driven poll),
feature/dbus-reply-loss (pselect6 + deferred exit-notify), the paint chain
fixes (fbdev ioctls + bitfields, R75 buddy rejection, R76 page-backed
FileBuffer, ALLOCTHROW rawframes), and feature/sched-fastpace
(R77 semaphore CAS, R78 exception-table fixup, R80 stale-LIVE containment,
R81 printer bound, slot-writer shadow diag, pselect6 test fix).

## Milestone: GNOME desktop painted (bigalloc6, kernel 8decfc8b, -smp 1)
Screen lit t=421-483s, 0.9999 non-black, held to 1500s, ZERO
ALLOCTHROW/panics. Frame = GNOME Shell overview (Activities, clock,
workspace thumbnails, dash, privileged-user banner). Colors were
byte-rotated; fixed by 44c7c06f (fbdev bitfields match format 3).

## Open items
1. Orphan-cycle producer (R74 family): slotless double-run with
   slotw=1/pick-publish, own live claim, oncpu=0 — context resume
   bypasses slot bookkeeping; ksp sometimes non-canonical (bigalloc9:
   0x9496725480609280). R80 containment keeps the box alive (agent's
   final-r1: 2100s smp4, 0 panics, 127 bounded heals).
2. Early session death t~150 (SIGTERM to gnome-session-b) on the full
   fire6 chain; minimal gs.sh path (direct gnome-shell --replace) paints.
3. make test boot panic on the sched branch's test kernel (layout-
   sensitive codegen shift) — needs its own hunt; GUI builds boot fine.
4. rootfs plain-Vec storage: same oversized-alloc class as R76 (follow-up).
5. Full run history: runs/bigalloc{1..9}, driver runs/bigalloc-hunt.py,
   image runs/gp3i.img (X + both dbus + gnome-shell, auto-clears X locks).
Sched-branch evidence: /home/william/rux-agents/gnome-sched2/FINDINGS.md.

# HANDOVER (2026-10-14, pausing for OH debug on another machine)

Pushed state: main @ be8817dc = full GNOME integration (poll event-wake,
D-Bus reply-loss pair, R75 buddy, R76 page-backed FileBuffer, fbdev
ioctls, R82 EFAULT ordering, R83 trap.S user_sp fix, R84 diagnostics,
R85 ti_kernel_sp anchor gating). Verified on this tree: GNOME Shell
overview paints (bigalloc11, smp4, t=1082s, held to 1500s, zero
panics); full-chain 2100s smp4 zero KERNPANIC/PREEMPT-UNDERFLOW with
session alive; make run smoke-boots.

PENDING (resume list, in priority order):
1. make test REGRESSION: stock main 41d7aad1 = 1087/1087 green; merged
   tree = test-kernel boot wedge (VW-WEDGE disk=0 used=6 irqs=0) or
   SIGILL after ext4 init. Bisect was started (git bisect between
   41d7aad1..HEAD) and aborted at step 1 — redo it. NOTE: GUI/fire6
   boots work fine on the same kernel content; suspect test-config
   THREAD/layout sensitivity. Repro: make test (rootfs at test/rootfs.img).
2. R85 full-gate verification pending (only build+smoke done): run
   fire6 2100s smp4 + bigalloc-hunt paint check on this kernel.
3. Producer-#2 (last ksp scribbler) still open: signature hi32 const
   0x19999995 (= (2^32-46)/10), lo32 0x400-aligned monotonic us.
   Traps in tree: R83 idle header audit, R83b ext4-wait self-watch
   (widen to non-ext4 ksp changes), freed-page 0xC3 paint, R84 tick
   audit + heap fingerprint + ksp tripwire.
4. Full-chain M5-paint: the GNOME overview paint is proven on the
   minimal session; the full fire6 chain reaches xterm-level lit only
   (components all alive) — closing producer-#2 likely unlocks it.
Artifacts: runs under rux-agents/gnome-int/runs (bigalloc*, drivers),
gnome-img/fire-* (gate runs). Worktrees with extra state: gnome-int
(integration), gnome-sched2 (sched branch), gnome-dbus2, gnome-paint,
gnome-cpu (FINDINGS + rigs).
