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

