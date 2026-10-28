# pa97 main-tip panic — abi-review bisect evidence (2026-10-29 02:00-03:40)

Lead item: main d2a58ccb strong panics (BTreeMap navigate.rs:552 unwrap
on None, 4 CPU prints) in the pbench p1->p2 window. Bisect executed in
/tmp/sec-bisect (detached at d2a58ccb), all kernels release-strong x86_64
q35 TCG smp2/2G, pbench_boot channel, every boot gated on a fresh bzImage
md5 (the stale-ELF trap caught twice early — both runs discarded).

## Boot matrix (all first boots of each kernel, host load 13-19)

| kernel | md5 (bzImage) | result |
|---|---|---|
| d2a58ccb tip (= perf lane pa97, md5 130f4f9b) | 130f4f9b | PANIC t=93s (pa97repro) — signature matches |
| + revert 2fbb5a64 (exec one-path-resolution, the lane's prime suspect) | 1367bc33 | PANIC t=6s |
| + revert ac53dd6e (fdtable lever B) | 254e1aaf | clean 240s (no DONE, host-load slow) |
| lever B + readers-through-entry-lock (arm B2) | b6b130e1 | PANIC t=9s |
| arm B2 + inline removal semantics, no park/drain (arm B3) | 1a5c8372 | PANIC t=9s |
| RE-RUN of the revert-ac53dd6e kernel (254e1aaf, same md5) | 254e1aaf | **PANIC t=6s** |

## Verdicts

1. **The repro is STOCHASTIC, not deterministic** — the same reverted
   kernel binary booted clean once (240s) and panicked on the second run
   (t=6s). Every single-boot bisect arm above is therefore UNSOUND,
   including the apparent convictions; only "tip panics repeatedly
   across 4 independent kernels and 6+ boots" holds.
2. The exec family (2fbb5a64) is exonerated to the same single-boot
   standard as everything else — the revert panicked.
3. The panic signature is unchanged in every boot: BTreeMap
   navigate.rs:552 unwrap-on-None, preceded (when reached) by
   SIGKILL-SEND / WAKE-STRANDED 23-30s / DEFERFREE prints. Stale-stack
   symbolization shows put_dec/fmt frames (the printer path) — the map
   identity is NOT resolved.
4. Clean anchor: main a2525a26 strong completes the SAME board 25/25
   rc=0 (abi1-main, session 1). Regression window = a2525a26..d2a58ccb,
   21 kernel commits (list in git log a2525a26..d2a58ccb -- kernel/).

## Required method correction for the next bisect (whoever picks it up)

- N>=3 boots per arm, or a deterministic trigger (the panicking shape is
  fork/exec/exit churn + munmap under SMP2; oomchurn or a fork-bomb
  probe may reproduce faster and tighter than pbench).
- Candidate non-exec suspects by the signature (heap corruption
  surfacing as a VMA-map/page-cache BTreeMap violation under task
  churn): the test-coverage session-2 ROUTED finding "exitwalk
  MM-DROP-CTX owner-alive drops under SMP2 exit churn" (routed, possibly
  unfixed — d2a58ccb merge message), the fdtable lever family
  (ac53dd6e + lever A call-pattern changes), e3c1ec35 mkdir-flush
  deletion / 306d7cf6 NJ deferred writes (fs ordering), b433fb13 CFS
  min_vruntime.
- Bisect kernels from this session (volatile /tmp — rebuildable in
  minutes from the revert commits listed): /tmp/bisectA2-bzImage,
  /tmp/bisectB-bzImage, /tmp/bisectB2-bzImage, /tmp/bisectB3c-bzImage.
