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

(no entries yet — first wave running)
