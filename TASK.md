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
