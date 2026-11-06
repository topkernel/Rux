Lane: trim-verdict — the heap-corruption trim face (shell SIGSEGV at CRTC config)
Base: main@641c8b2c. Worktree = THIS (branch trim-verdict). Work
autonomously; never ask. NEVER write to /home/william/workspace/Rux.

## The bug
On good boots the shell starts, mutter draws, then during display
configuration (CRTC scaling path) libX11/libgobject walks into a
brk-trimmed region and SIGSEGVs (USEGV-FOR: cr2 in the 0x5555 heap
range, vma=NONE). gnome-session respawns; if it dies twice quickly
the user sees the "something has gone wrong" dialog. The kernel and
glibc agree the trim is legitimate (brk/VMA consistent — verified
by the BRK anomaly check, zero hits); the glibc bin chains must
have been corrupted BEFORE the trim. MALLOC_CHECK_=3 reports nothing
(checker itself sees consistent free lists).

## Instruments already in the tree (main@641c8b2c)
- TRIMQ (boot param trimq=1): brk-shrink records the freed range
  (PFN + first-16-bytes snapshots for 256 pages + present-at-trim
  bitmap for 18MB per slot); bad-area prints TRIMQ-HIT (who/what/
  age) + TRIMQ-PRESENT (the hit page was live at trim = bins chained
  resident data) + TRIMQ-FRAME (frame content then-vs-now).
  NOTE: past captures on full-device boots land 353-1591 pages into
  17MB trims — TRIMQ-PRESENT fires on ANY hit; TRIMQ-FRAME needs
  idx<256 (widen if the hits stay beyond).
- RCU monopolist namer (in RCU-GP-BOUND): stuck-CPU current task
  with preempt/tick/lastsys/tick-entry-rip.
- runit.c (scripts/test/gnome-x86/runit.c) is OUR code: it can set
  env for the session (MALLOC_PERTURB_ etc. — note MALLOC_TRACE=
  /tmp log killed a boot via I/O flood; /dev/shm target also died;
  if you need mtrace, gate it to a tiny window or use a different
  strategy).
- The image build (make ubuntu-image) stages from
  scripts/test/gnome-x86/build-img.sh; debugfs write into a reflink
  copy is the fast path for injecting runit variants (see git log
  for examples).

## Your mission
1. Boot full-device + trimq=1 until the shell-crash form appears
   (headless OK for this face — the crash is at CRTC config after
   mutter starts; grep 'Running GNOME Shell' then the USEGV-FOR with
   cr2 in 0x5555 range).
2. Read the TRIMQ-PRESENT verdicts on the hits: present=1 (bins
   chained LIVE data — corruption before trim, hunt the FIRST bad
   write) vs present=0 (wild pointer).
3. If present=1: use TRIMQ-FRAME content (trim-time w0 vs now) —
   intact-then-changed = post-trim overwrite (aliasing); intact-and-
   same = pure stale pointer in the bins. Pick the next instrument
   accordingly (the first-bad-write hunt needs a write watchpoint
   class or an interposer; the stale-pointer verdict means glibc's
   own unlink skipped a generation — audit our brk-shrink munmap
   ordering vs a racing free() in the OTHER thread).
4. FIX the root cause. Verify: 5 full-device boots, zero shell
   SIGSEGV, session alive to timeout (the "something has gone
   wrong" dialog gone). Gates: x3 builds, riscv make test, Tier-2
   pbench if mm touched. Push origin/trim-verdict.

## Hunt state (2026-11-07, resumed from quota death — verdicts in)

Mission steps 1-3 are DONE; step 4's fix is committed and
reproducer-verified. The full-device verification is currently blocked
by a SEPARATE intermittent boot face (see Park face below).

- Root cause of the trim face (steps 1-3 verdict chain): NOT glibc bin
  corruption. The heapstress reproducer (init=/sbin/heapstress +
  trimq=1 mmoverlap=1, catches in 10-170s pre-fix) showed in every
  catch a VA-OVERLAP between two LIVE glibc chunks of different
  threads (hs2: t1/b58 x t2/b63; hs3: t2/b93 x t3/b44), victim's
  foreign bytes = the OTHER block's inverted canary pattern (exact
  match), victim headers intact, zero MM-STALE-PTES: the kernel's
  sys_mremap EXPAND path checked can_expand under a vma_read guard,
  dropped it, and installed the extension with MAP_FIXED mmap —
  absorbing a same-mm racer's fresh chunk placed inside
  [old_end, new_end). Fixed in 3a7b28ea (atomic in-place expand under
  one vma_write hold, MAYMOVE fallback on failure).
- Reproducer verification: 6/6 heapstress boots clean post-fix
  (run/fx1.log..fx1x5.log, "no catch in 6 boots") vs catch-every-time
  pre-fix (hs1-hs3, tv15).
- Full-device boots post-fix: fv1-fv1x3 + fv2 + bs3 = zero shell
  SIGSEGV, zero TRIMQ-HIT, zero fdreap stalls — but the session now
  parks at the check-accelerated stage in 6/6 boots (the Park face),
  so the CRTC-config crash path is never exercised. tv9-tv13 (pre-
  3d1da2dc) reached the shell 4/5; the park existed there too (tv9)
  but amplified ~1/5 -> 6/6 after 3d1da2dc.

## Park face (NEW charter item — not this lane's root cause)

Signature: gnome-session-check-accelerated spins at 100% of ONE cpu
in USERSPACE (bs3 debug=taskdump,periodic: pid 318 vruntime
+21.6s per 30s window, state RUNNING on_cpu=1, 7 dumps over 3.5min)
while its GL-helper child (319) sits ZOMBIE unreaped; gnome-session-b
(317) waits; Xorg/dbus alive; session log frozen at the 5.3s dbind
warning; shell never spawns. STALE-CLAIM-RESET heals fire repeatedly
on the spinning pid (also in good boots — known-racy claim area).

Ruled out by spinprobe (sp1/sp2 minimal-init boots, 521f9295):
waitpid(WNOHANG) reaps fresh+mature zombies (zeros measured are
fork-to-exit latency 0-27ms), no phantom/sticky POLLIN on
socketpair/pipe/accepted-conn, SIGCHLD exactly-once with no
redelivery. The kernel primitives a glib loop would spin on are
correct in isolation — the spin needs session context (GDK GL / Xlib
/ glib main-loop interplay). Next instrument: session-context
syscall/rip tracing via the /etc/runit-env hook (staged by debugfs
into a reflink image copy; GDK_DEBUG/GTK_DEBUG/MALLOC_* envs, or a
kernel tick-sampler of the spinning task's user rip as a tracepoint).

Rate changed across 641c8b2c..3d1da2dc (35 commits: drain fix, fork
FPU inheritance, COW fix, pipe/pty, io_uring ABI, sched same-owner,
cap fast path, alarm(2)/ENOSYS table). Bisect boots are 7min each —
charter a dedicated lane; do NOT burn this lane's quota.

## Discipline
One QEMU at a time; kill only yours; pkill FORBIDDEN. Boot rig:
qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2
-display none -serial stdio -monitor none -no-reboot -snapshot
-drive file=<main-repo>/work/gnome-x86.img,if=none,id=rootfs,
format=raw,file.locking=off -device virtio-blk-pci,disable-legacy=
on,drive=rootfs -device virtio-gpu-pci,xres=1280,yres=800 -device
virtio-keyboard-pci,disable-legacy=on -device virtio-tablet-pci,
disable-legacy=on -kernel <THIS-worktree>/target/x86_64-unknown-
none/debug/bzImage -append "root=/dev/vda rw init=/sbin/runit
console=ttyS0 trimq=1"
Final message: TRIMQ-PRESENT/FRAME verdicts + root cause + fix +
5-boot verification.
