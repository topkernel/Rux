# Stress-Test Bug List (living document)

Random-workload (monkey) and soak findings. One entry per distinct bug:
deduplicate by panic signature / root cause, not by instance. Entries are
fixed top-down when scheduled; do not delete entries — mark them FIXED
with commit + wave that verified the fix.

Format:

## BUG-S###  <one-line title>
- Class: PANIC / WEDGE / INIT-DEATH / QEMU-ABORT / CORRUPTION
- Signature: <the stable marker in the serial log, e.g. first panic line>
- First seen: wave-<tag>, instance <n>, seed <s>
- Frequency: <n>/<total> instances across waves so far
- Logs: <path(s)>
- Status: OPEN / TRIAGED / FIXED(<commit>)
- Notes: <triage hypothesis, repro command>

---

## BUG-S001  virtio-blk used-ring completion order != submission order under concurrent I/O
- Class: PANIC-adjacent (I/O error injection)
- Signature: `virtio-blk: out-of-order completion #1 (used-ring order != submission order)`
- First seen: wave-20261007-015149, instances 7/11/+1, seeds 100007/100011/...
- Frequency: 3/16 logs (wave 1)
- Logs: test/stress/runs/wave-20261007-015149/serial-*.log
- Status: OPEN
- Notes: fires under concurrent flush+read (monkey `sync` + file churn). Likely
  race in the completion path (historically bug-dense per CLAUDE.md: "virtio
  completion ordering ... read the R-numbered comments before changing").
  Root-cause candidate for S002 (EIO below).

## BUG-S002  ext4 entry barrier sync returns EIO under load, cascades into user-visible I/O failures
- Class: CORRUPTION-risk / WEDGE-trigger
- Signature: `ext4: entry barrier sync blk 1 failed (errno -5)` followed by
  `exec: transient read of /bin/<bin> failed (pid N, attempt 1..2), retrying`
  and `cat: No such file or directory`
- First seen: wave-20261007-015149 (systematic)
- Frequency: 15/16 logs (wave 1)
- Logs: same wave dir
- Status: OPEN
- Notes: after the barrier EIO, subsequent page-cache reads of binaries fail
  transiently. Repro: boot rootfs, run a write burst + `sync` loop. Check
  whether every instance logging S001 also logs S002 first (3 overlap seen);
  if S001 is the only producer, fixing S001 may fix S002.

## BUG-S003  WEDGE: fifo reader blocks forever when its writer died on EIO
- Class: WEDGE (manifestation of S002)
- Signature: serial log ends at `cat /tmp/pN` with no prompt afterwards
- First seen: wave-20261007-015149, all 14 WEDGE instances
- Frequency: 14/16 (wave 1)
- Logs: same wave dir
- Status: TRIAGED (downstream)
- Notes: `mkfifo p; (echo x > p &); cat p` — when the background writer fails
  with EIO (S002), the reader blocks indefinitely. POSIX-wise a dead writer
  should deliver EOF via pipe release; verify the kernel's pipe release path
  runs on writer do_exit even when the write itself errored. Fix S002 first,
  re-check whether S003 persists.


## BUG-S004  (toolchain, resolved by CPU model) userspace SIGILL on Zbb instructions (zext.b) from partially-unpinned builds
- Class: INIT-DEATH (userspace)
- Signature: `SIGDEATH ... comm="sh" sig=4`, QEMU int log shows illegal_instruction with tval = zext.b encoding (0x9fe1 etc.)
- First seen: LTP baseline attempts; repro `y=$((2+3))` in mrsh
- Root cause: Ubuntu 25.10 cross-gcc default march includes Zbb/Zba/Zbs/Zcb(+V); the
  -march=rv64gc_zicsr pins caught musl/toybox/LTP but some mrsh objects still got Zbb
  (build plumbing TBD — flag was in config.mk yet objects contain zext.b).
- Resolution: run QEMU with `-cpu rv64,zbb=true,zba=true,zbs=true` — stateless scalar
  bitmanip, no context-switch impact (V stays forbidden: kernel saves no vector state).
- Status: MITIGATED (CPU model); toolchain hygiene item OPEN (audit every build script
  for the pin; add a post-build scan rejecting zext.b/rev8/andn/etc).
  S005 progress on the hygiene item: the same unpinned-libgcc leak was confirmed
  for Zcb (c.zext.w) and Zicond (czero.eqz) encodings, not just Zbb. mrsh and
  toybox now link a pin-clean soft-fp overlay and carry build gates that reject
  zcb1p0/zicond1p0 attributes in the final binary (see BUG-S005). LTP binaries
  and the Ubuntu-rootfs programs remain unaudited until their next rebuild.

## BUG-S005  Repeated core-dumping children corrupt the parent shell (SIGILL after ~2 aborting children)
- Class: CORRUPTION → INIT-DEATH  (misfiled: actually TOOLCHAIN/ISA, see root cause)
- Signature: children die correctly (sig=6 SIGABRT, core dumped); after the 2nd-3rd
  such child, the PARENT sh (even PID 1) dies with sig=4 SIGILL
- Repro (2-3 min, riscv64, -cpu rv64,zbb=true,zba=true,zbs=true, smp1 or smp2):
  `i=0; while [ $i -lt 8 ]; do /test/linux-ltp/testcases/bin/abort01 >/dev/null 2>&1; echo "iter $i rc=$?"; i=$((i+1)); done`
  → iter0/iter1 ok, iter2 kills the shell
- First seen: LTP r2 baseline runner death (run_ltp.sh stops after first test)
- Frequency: deterministic (3 iterations)
- Logs: /tmp/repro4.log
- Status: FIXED (feature/s005-coredump: pinned soft-fp overlay in the userspace builds)
- Root cause: NOT the coredump path. A plain `i=0; while ...; i=$((i+1)); done` loop
  with zero children kills the shell identically (verified with a kernel-side
  SIGILL forensic print: pid=1 sh, epc=0x3d99a, insn=0x9ff1, fs=Dirty — text page
  byte-identical to the on-disk ELF). 0x9ff1 is `c.zext.w`, a Zcb encoding.
  Ubuntu 25.10's riscv64 cross libgcc.a (gcc 15) is built with zbb+zba+zbs+zcb+
  zicond, so every static link that resolves a __*tf3/__floatsitf/__clzdi2 member
  through -lgcc ships those encodings; mrsh's musl libc pulls exactly that set
  (strtold/floatscan → __floatsitf for the integer part). The pinned QEMU CPU
  model has zbb/zba/zbs but NO Zcb/Zicond → illegal_instruction → correct SIGILL.
  The "2-3 children" correlation was coincidence: the loop's arithmetic reaches
  the first nonzero __floatsitf on the 3rd $(( )) evaluation, right after the
  2nd child — abs01's loop just didn't hit that path as early.
- Fix: vendor gcc's soft-fp sources (userspace/soft-fp/, from releases/gcc-15)
  plus a plain-shift __clzdi2/__clzsi2, compile with the same -march=rv64gc_zicsr
  pin, and link libsoftfp-pin.a ahead of every -lgcc in build-mrsh.sh and
  build-toybox.sh (userspace/build-softfp.sh). Post-link gates fail the build if
  the final binary's .riscv.attributes mention zcb1p0/zicond1p0. LTP's build.sh
  got the same overlay for its next rebuild; the 5595 binaries currently in
  userspace/linux-ltp/output still carry zcb/zicond (atof01 FAILs with SIGILL on
  czero.eqz = Zicond from the same libgcc) — rebuilding LTP is follow-up work.
- Verified: repro loop reaches iter7 + WHILE-OK with the shell alive (49/50 first
  LTP runner tests PASS, runner proceeds well past abort01).
