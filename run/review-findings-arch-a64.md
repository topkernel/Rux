# Review findings: arch-a64 (rv-arch-a64)

Scope: kernel/src/arch/aarch64/** vs the riscv64 twin; map =
docs/architecture/aarch64.md. Registered leftovers in the map's last
section NOT re-filed. Focus areas per charter: TLB maintenance,
per-task state switching, GICv3 encoding.

Verification per change: `make build`, `make build PLATFORM=x86_64`,
`make build PLATFORM=aarch64` — all rc=0 for every commit. One QEMU
boot smoke (45 s, -smp 2 -m 1G, gic-version=3) A/B'd pre-fix vs
post-fix kernel: serial logs identical modulo timestamps
(run/aa64-prefix-serial.log vs run/aa64-boot-serial.log) — both reach
`init task (PID 1) enqueued`, 2 CPUs online, zero panics; the
no-shell-prompt terminal state is pre-existing on this main-lineage
kernel + the borrowed rootfs driver, not introduced here.

## FIXED

### F1. switch_mm ASID encoding (context.rs) — commit 4594bf4d

The scheduler's `switch_mm(next_ppn, asid)`:
1. wrote TTBR0_EL1 as `ppn << 12` — ASID field [63:48] zero. With
   TCR_EL1.A1 = 0 (decoded from boot.S TCR 0x5B5103510, bit22 = 0) the
   TLB-tag source IS TTBR0_EL1[63:48], so every mm ran under tag 0;
2. `msr contextidr_el1, asid` — ARMv7-style ASID register, does NOT
   tag ARMv8 TLB entries (asid.rs's own module doc says so);
3. `tlbi aside1is, {asid}` with an UNSHIFTED operand — the by-ASID op
   takes the ASID in operand bits [63:48], so it named ASID 0 for every
   mm (the same silent-no-op encoding class as the GIC IGROUPR/SGI1R
   bugs already documented in the tree);
4. `dsb nsh` after an inner-shareable broadcast op — under-barrier for
   completion.

Correctness held only through a lattice of accidents (all generic PTE
flushes go through flush_tlb_page(...,0) → vaae1is, all-ASID, which
hits entries regardless of tag; exec/exit use write_satp which does a
full vmalle1is; the one wrong flush happened to flush everything
because everything was tag 0). The ASID wiring the scheduler performs
(alloc_asid per mm) was inert, and flush_tlb_asid / ASID reuse would
silently miss (no callers today).

Fix encodes the riscv64 twin's exact semantics (satp with ASID inside +
ASID-scoped sfence): TTBR0 = root | asid<<48; `tlbi aside1is, asid<<48`;
`dsb ish`. Blast radius checked: every in-tree arch flush site uses
asid=0 (vaae1is/vmalle1is) or flush_tlb_others — all remain correct
under real per-mm tags.

Gates: reference contract (ARM DDI EL1 TLB tagging + TLBI operand
layout; in-tree precedent asid.rs/write_satp; twin riscv64
context.rs switch_mm) / three builds rc=0 / boot A/B identical.

### F2. Stale uaccess contract note (uaccess.rs) — commit 2828ccef

copy_to_user doc claimed the twin's fault_in_write retry was still
missing ("a resolvable COW fault reports a short count"; "no user
tasks exist on this board yet"). Both false now: mm/exception.rs
do_page_fault has the R82 kernel-uaccess demand-fill arm (VMA-covered
not-present/COW faults resolved in-handler before the fixup), and user
tasks exist. Documented the actual contract; no code change needed.

### F3. Parity copy_thread latent pc+4 skip (process.rs) — commit 2828ccef

Dead parity path advanced the fork child's pc by 4 past the SVC —
the pre-port riscv64 habit. On this ABI the frame's pc (ELR) already
points after the svc (map + trap.rs + fixed handle_syscall all
agree), so +4 would skip one user instruction per fork, the exact bug
family that killed PID 1's brk in bring-up. The live generic
copy_thread copies verbatim. Fixed the parity twin (removed +4) before
anyone wires it in.

### F4. handle_syscall doc-block contradiction (trap.rs) — commit 2828ccef

Doc bullet asserted "ELR points AT the svc; MUST advance pc by 4" —
the opposite of the implemented (correct) contract and of the map.
Code comment below it was right. Aligned the doc.

### F5. ESR EC decode mislabels (pt_regs.rs, trap.rs) — commit 2828ccef

0x12 was mapped to EcallSupervisor commented "HVC (PSCI conduit)" —
0x12 is HVC in AArch32 state; the AArch64 HVC that smp.rs actually
issues (PSCI, consumed at EL2) raises EC 0x16, which fell to Unknown
→ oops. Mapped 0x16 to the supervisor arm, relabeled 0x12, corrected
the trap arm comment ("nothing issues HVC today" — smp.rs does, it
just never reaches EL1). 0x11→EcallUser relabeled SVC32 (mapping kept;
no AArch32 userspace exists on the port).

### F6. boot.S dead DTB store — commit 2828ccef

After the EL2→EL1 drop, x0 holds the eret landing address; the block
"2. Save the DTB pointer" stored that (or 0 on direct-EL1 entry) into
dtb_pointer — never a DTB pointer. Dead (real publishes at .Lat_el1
and post-MMU re-publish precede any reader) but a trap for the next
editor. Removed; boot.rs's "set by boot.S from x0" doc fixed too
(boot.rs comment fixed in a follow-up commit with the ledger).

## OBSERVATIONS (no action — safe-direction quirks, twin parity)

- context_switch root-ppn compare masks 48 bits, so composed asid bits
  [47:44] pollute the compare for asid >= 16 → one extra (safe) switch
  + flush per switch-in. The riscv64 twin has the same quirk (52-bit
  mask over a 44-bit PPN field). Left for symmetry.
- Satp::new keeps 60 PPN bits vs build_satp's 48 vs the format doc's
  44 — unreachable with real PPNs (RAM ≤ 2 GB → PPN < 2^18).
- aarch64 AsidContext::free lacks the twin's flush_tlb_asid — moot:
  the bump allocator never reuses an ASID within a boot, switch-in
  invalidation (now correctly encoded, F1) covers reuse for the
  exhaustion-fallback ASID-0 case, and AsidContext has no callers
  (generic code uses MmStruct's asid field + arch alloc/free).
- ASID pool is 8-bit/254 and never reused; exhaustion falls back to
  ASID 0 per-switch-flush (same documented twin fallback, safe). The
  generation-rollover is already recorded in-code as stage-2 work.
- psci_call asm lacks preserves_flags — SMCCC says condition flags may
  be clobbered; without the option rustc assumes they're preserved.
  Theoretical on TCG; not fixed in this sprint (matches the twin's SBI
  call shape; would want a targeted test first).
- EC 0x22 (PC alignment) and 0x07 (FP trap) fall to Unknown → SIGILL
  for user; reference kernel routes alignment to SIGBUS. No producers
  on this port (A64-only userspace, CPACR FPEN always set); noted for
  the LTP-on-aarch64 lane.

## FALSE-POSITIVE RETRACTIONS (suspected, re-derived, dismissed)

- "missing flush in AsidContext::free is a stale-TLB hazard" —
  re-derived: no ASID reuse (bump allocator) + F1's switch-in flush +
  full-flush write_satp on exec/exit close every window.
- "flush_tlb_page(asid==0) → vaae1is misses ASID-tagged entries" —
  VAA* matches ALL ASIDs by definition (and global entries); this is
  exactly why the generic all-asid-0 flush discipline is correct here.
- "user-tree global entries leak across mms" — kernel half is TTBR1
  (global, untouched by aside1is); user trees carry no global leaves
  except deliberate device/vmalloc aliases shared by design.
- "el0 FPSIMD bracket corrupts kernel FP state across schedule" —
  traced both paths: user image (user_fpsimd) and kernel image (fpu)
  are disjoint buffers; switch/restore sites never cross them.
- "tlbi aside1is after msr ttbr0 is ordered wrong" — re-derived: the
  flush targets the NEW asid (fresh entries), stale entries of the old
  mm can only be hit under their own tag at that mm's next switch-in,
  which flushes first. Twin's sfence order is identical.
