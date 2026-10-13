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
