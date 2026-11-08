# Arch Tree Review — Round 4 (r4-arch)

Scope: every file under /home/william/Rux/kernel/src/arch/ (riscv64, x86_64, aarch64,
shared mod.rs/uaccess_safe.rs). Read-only review; findings appended incrementally.
Dedup: docs/development/review-coverage.md "Defects found and fixed" table and
docs/archive/code-review-2026-09-12.md ARCH-* items are already fixed — not re-reported.

## Running coverage list

(updated as files are read end to end)

| file | lines | status |
|---|---:|---|
| arch/mod.rs | 73 | ✅ |
| arch/uaccess_safe.rs | 72 | ✅ |
| riscv64/mod.rs | 132 | ✅ |
| riscv64/boot.rs | 32 | ✅ |
| riscv64/cpu.rs | 184 | ✅ |
| riscv64/pt_regs.rs | 458 | ✅ |
| riscv64/vext.rs | 386 | ✅ (encodings verified against riscv64-linux-gnu-as) |
| riscv64/uaccess.rs | 512 | ✅ |
| riscv64/uaccess.S | 434 | ✅ |
| riscv64/boot.S | 695 | ✅ |
| riscv64/linker.ld | 101 | ✅ |
| riscv64/trap.S | 1215 | ✅ |
| riscv64/trap.rs | 2003 | ✅ |
| riscv64/context.rs | 587 | ✅ |
| riscv64/thread.rs | 525 | ✅ |
| riscv64/process.rs | 332 | ✅ |
| riscv64/smp.rs | 348 | ✅ |
| riscv64/ipi.rs | 512 | ✅ |
| riscv64/mm/mod.rs | 53 | ✅ |
| riscv64/mm/memory_layout.rs | 569 | ✅ |
| riscv64/mm/pagetable.rs | 294 | ✅ |
| riscv64/mm/asid.rs | 302 | ✅ |
| riscv64/mm/fixmap.rs | 267 | ✅ |
| riscv64/mm/exception.rs | 721 | ✅ |
| riscv64/mm/page_fault.rs | 1559 | ✅ |
| riscv64/mm/mm_ops.rs | 2737 | ✅ |
| riscv64/mm/mmu_init.rs | 2162 | ✅ |

## Findings

### [Medium] riscv64/trap.rs:1255 — traced user ebreak advances epc by 4; reference leaves epc at the ebreak — ✅ FIXED (`834f2fca`)

`handle_breakpoint`'s real-user-ebreak-under-tracer arm does `regs.epc += 4`
before ptrace_stop. The reference `handle_break` (arch/riscv/kernel/traps.c:294)
only does `force_sig_fault(SIGTRAP, TRAP_BRKPT, (void __user *)regs->epc)` —
epc is NOT advanced for user breakpoints (gdb self-manages the skip; its riscv
backend assumes pc==breakpoint on stop, decr_pc_after_break==0). With the +4,
a tracer that restores the original word and rewinds pc to step the breakpoint
double-skips. Also `c.ebreak` (2 bytes) gets +4. Suggested fix: don't advance
epc for user ebreak under a tracer; report pc as-is (single-step arm already
does this correctly).

### [Medium] riscv64/trap.rs:1896 — user-mode unknown exceptions (misaligned load/store, access fault) killed with SIGKILL; reference delivers SIGBUS/SIGSEGV

`handle_unknown_exception` does `do_exit(-SIGKILL)` for every user-mode cause
that isn't page-fault/ecall/illegal/breakpoint — that includes
LoadAddressMisaligned (4), StoreAmoAddressMisaligned (6),
InstructionAddressMisaligned (0), and access faults (1/5/7). Reference:
arch/riscv/kernel/traps.c do_trap_load_fault/misaligned arms deliver
SIGBUS/BUS_ADRALN (misaligned) and SIGSEGV/SEGV_ACCERR (access fault), and
instruction misaligned → SIGBUS/ILL... (traps.c:231). A user program relying on
a SIGBUS handler for misaligned data (LTP) sees SIGKILL. Fix: route per-cause
signo like the reference's do_trap_error table.

### [Low] riscv64/trap.rs:758 — 4-aligned c.ecall is skipped by 4 bytes (matches Linux's own +4, kept as Info-grade)

`instr_size = if orig_epc % 4 == 0 { 4 }` without reading the instruction: a
c.ecall (16-bit 0x9002) landing on a 4-byte boundary resumes 2 bytes into the
next instruction. The reference do_trap_ecall_u does the same unconditional
`regs->epc += 4` (traps.c:332), and no mainstream libc emits c.ecall, so this
is reference-parity, not a divergence. No action needed; documenting because
round-1 ARCH-M6 flagged this class.

### [Low] riscv64/trap.rs:678 — dead resched branch inside hardirq bracket

`handle_timer_interrupt` step 4 calls `schedule()` "if need_resched &&
preemptible()" — but the caller runs it between irq_enter()/irq_exit(), where
preempt_count always has HARDIRQ_OFFSET, so the condition is never true. The
comment implies it fires. Dead code; remove or route through the
irq_exit_maybe_preempt_kernel hook like the IPI path.

### [Low] riscv64/trap.rs:1514-1543 — kernel-mode Segfault/PermissionDenied/BusError arms print and return (verify unreachable)

These arms `pr_err!` and fall through, returning to re-execute the faulting
instruction in an infinite loop if ever reached. If do_page_fault's
exception.rs guarantees kernel-mode faults only produce KernelPanic/OOM, these
are unreachable defensive arms — but nothing in trap.rs documents that
contract. (Pending verification in mm/exception.rs — will be adjudicated
below.)

### [Low] riscv64/mod.rs:121 — hardcoded ti_cpu offset 0x18 duplicates task_offsets::TI_CPU

cpu_id() hardcodes `ti_cpu_offset = 0x18` while trap.S correctly injects
`task_offsets::TI_CPU` via the global_asm! const mechanism (the R57 lesson:
hand-maintained offsets drifted once already and caused the GNOME scribbler
family). Same class: boot.S hardcodes KERNEL_MAP field offsets (0/16/40)
against Rust KernelMapping. Fix: use the exported constant (cpu_id can read it
via a const from task_offsets).

### [Low] riscv64/trap.S:98,107,666 — hardcoded MAX_CPUS=4 in asm (three sites)

`li t0, 4 // MAX_CPUS` in GET_PER_CPU_INTR_STACK and .Lcall_handler clamp.
Kernel.toml `max_cpus` is the source of truth (config::MAX_CPUS); if it is ever
raised, secondary harts with ti_cpu >= 4 would clamp to CPU 0 (wrong interrupt
stack aliasing, wrong IPI targeting) with no build failure. Inject
`crate::config::MAX_CPUS` as a global_asm! const like the other offsets.

### [Low] riscv64/cpu.rs:55 — get_counter_freq hardcodes 10 MHz instead of reading timebase-frequency from DTB

Linux reads /cpus/timebase-frequency (riscv_timebase). Any platform whose
timebase != 10MHz gets wrong timer scaling while the DTB is already available
(get_dtb_pointer). Works on QEMU virt; fragile design. Fix: parse DTB
timebase-frequency at timer init (the DTB walker already exists for the ISA
string — vext::prime_hart_has_v).

### [Info] riscv64/mod.rs:45-47 — sstatus read-modify-write momentarily zeroes the whole CSR

`csrrw {}, sstatus, zero` writes all-zero into sstatus before writing the
modified value back — a transient window where FS/VS/SPIE/SPP/SUM are cleared.
Benign today (SIE=0 during the window; no FP instruction executes between the
two asm lines; called early in boot and in enable_interrupts), but the
idiomatic form is `csrci sstatus, 2` / `csrsi sstatus, 2` (already used in
cpu.rs). Suggest switching for robustness.

### [Info] riscv64/vext.rs:377 — comment mislabels vector load/store width encodings

`is_vector_instr32` width set {0,4,5,6,7} is CORRECT (verified: vle8=0,
vle16=5, vle32=6, vle64=7 via riscv64-linux-gnu-as), but the comment says
"8/128/256/512-bit EEW" — actual mapping is 8/16/32/64-bit (plus reserved 4).
Misleading doc only; no behavior change.

---

## Findings (riscv64 context/smp/ipi/mm-layout, continued)

### [Medium] riscv64/mm/memory_layout.rs:548 — virt_to_phys kernel-image window bound is stale (16MB cap vs 24MB image window) — ✅ FIXED (`3a17c555`)

Arm 2 accepts image VAs only up to `0xffff_ffff_8100_0000` (= link + 16MB), but
the image window grew to 24MB (KERNEL_SIZE=0x1800000, RUX-RISCV-LAYOUT-4/5;
LAYOUT-5 was triggered by a real build passing 20MB). A .data/.bss VA in
[0xffffffff81000000, _end) fails arm 2 (and arm 3, since 0xffffffff... > 0x90000000),
lands in the fallback arm, and virt_to_phys returns the VIRTUAL address as
physical with one pr_warn — static driver buffers (virtio queues in .bss are
this arm's documented consumer) above 16MB would DMA to a bogus PA. Fix: bound
the arm by `_end` (linker symbol) or KERNEL_LINK_ADDR + KERNEL_SIZE, not a
hand-copied 16MB constant.

### [Medium] riscv64/ipi.rs:122-130 — flush_tlb_others targets CPUs [0, online_count) instead of the started set — ✅ FIXED (`f4cbbd70`)

`let online = num_started_cpus(); for cpu in 0..min(online, MAX_CPUS)` — with a
sparse started set (e.g. harts {0,1,3} online because hart 2's hart_start
failed, count=3), the loop sends the TLB-flush IPI to hart 2 (dead) and SKIPS
hart 3 (alive) — hart 3 keeps stale TLB entries after any unmap, the exact
hazard the shootdown exists to prevent. Benign while QEMU presents contiguous
harts 0..N-1, wrong the moment one hart fails to start. Fix: iterate
0..MAX_CPUS and test cpu_started(cpu) (the helper exists for exactly this).

### [Medium] riscv64/mm/exception.rs:405 AND aarch64/mm/exception.rs:247 — non-canonical-VA guard tests the VirtAddr-masked address, not raw stval/FAR

The guard `fault_addr.bits() >> 39 (resp. 47) != 0` operates on
`VirtAddr::new(badaddr)`, which for bit38/47-clear addresses has ALREADY
masked the high bits to zero — so
non-canonical user faults with bit38/47 clear (e.g. riscv64 stval
0x80_0000_0000, or STACK_TOP+2^39; aarch64 FAR 0x1_0000_0000_0000-class)
pass the guard and are served at the TRUNCATED address: a
spurious page can be mapped under the truncated VA before the retry faults
again into AlreadyMapped → SIGSEGV ACCERR (wrong si_code, junk mapping).
Only the bit38/47-set half of the non-canonical class is actually caught (the
sign-extension makes the shift nonzero — the SIGTRAMP_BASE incident). The
x86_64 twin (exception.rs:527) does the guard correctly on the RAW CR2
value. Fix: test `regs.badaddr`/`regs.far` canonicality directly before the
VirtAddr conversion.

### [Medium] riscv64/mm/exception.rs:451-697 — NOVMA-EXEC forensics fire unconditionally on every user exec fault at an unmapped VA

~250 lines of hunt instrumentation (fixed-address PLTGOT probes at
0x1842/0x17208/..., fixed stack base 0x3fffffe_a80 "for this rootfs", full-page
dumps, PTEI/zone-ledger/STACKZERO/MMAP-ring replays) run with NO debug gate on
every user EXEC fault with no VMA — i.e. every wild function-pointer jump in
any crashing program pays ~40 pr_err lines of mostly-unrelated output through
the polled UART (ms-class on this console, per the repo's own print-audit
notes). Not a correctness bug, but a hot-path diagnostic regression the
print-audit lane missed. Fix: gate the block behind a debug switch (the
debug::switches table exists) like the SIGILL-FORENSICS budget in trap.rs.

### [Info] riscv64/trap.rs:1514-1543 — kernel-mode Segfault/PermissionDenied/BusError arms are unreachable (adjudicated)

do_page_fault's kernel-mode block only returns Fixed/Handled/KernelPanic (or
panics on stack overflow); Segfault/PermissionDenied/BusError are produced on
user-mode paths only. The trap.rs else-arms are dead defensive code, not a
fault-retry loop. No action needed; a one-line comment in trap.rs would help.

### [Low] riscv64/process.rs:93 — arch copy_thread is dead code that heap-allocates pt_regs

The doc comment says the live path is process/fork.rs's generic copy_thread;
this arch variant (heap-allocated PtRegs, thread.sp pointing into the heap)
has no callers. If ever revived, a trap on the child would push frames below a
HEAP address, not a kernel stack. Delete or mark clearly.

### [Low] riscv64/mm/fixmap.rs:190-195 — UART fixmap maps device memory cacheable (no SVPBMT IO bit)

PageTableEntry::IO (bit 62) exists but set_fixmap/init_uart_fixmap and
copy_fixmap_to_user use V|R|W|A|D|G only. On Svpbmt-capable hardware the UART
window would be cached (speculative reads on a FIFO register = lost/duplicated
chars). Same for boot.S early_pmd_io (0xC7). QEMU-virt TCG tolerates this
today. Reference: arch/riscv uses _PAGE_IO for device mappings (pgtable.h).

### [Info] riscv64/smp.rs:243 — secondary harts enable SIE (timer) before init_secondary

secondary_cpu_entry enables timer interrupts (STIE + global SIE via
enable_timer_interrupt's csrsi) BEFORE the boot-complete wait and before
init_secondary: a tick during the wait runs the full handle_timer_interrupt
(scheduler_tick, charge_tick) on a hart whose cpu_state is not yet
initialized. Works because sched::init pre-created the idle tasks and the
tick path tolerates it, but it is fragile ordering — enable after
init_secondary (or at least after is_boot_complete) matches the reference
secondary_start_kernel order.

---

## Findings (riscv64/mm: page_fault, mm_ops, mmu_init)

### [Medium] riscv64/mm/mmu_init.rs:1582 — map_page does a FULL sfence.vma on every call (double flush on every demand fault) — ✅ FIXED (`f4cbbd70`)

`map_page` ends with bare `asm!("sfence.vma")` = `sfence.vma zero,zero` — a
whole-TLB flush (all ASIDs) — while its main caller (handle_mm_fault) installs
a FRESH PTE (no stale entry to invalidate) and then issues its own
address-scoped sfence after map_page returns. So every user page fault pays
TWO fences, and the first one destroys every other mm's TLB entries — the
ASID machinery in switch_mm exists precisely to avoid this. mremap/io_uring
single-page callers have the same double-flush shape. Linux installs fault
PTEs with no flush at all on the local hart (flush_tlb_page only when
REPLACING a translation). Fix: drop the fence from map_page (each caller
already fences) or make it `sfence.vma {vaddr}, zero`.

### [Low] riscv64/mm/mm_ops.rs:865-870 — unmap_pages foreign-frame arm continues without advancing addr (double walk per foreign page, no sub_rss)

The device/foreign-frame arm does `clear_pte(addr); continue;` — unlike the
U==0 arm right above, it does NOT `addr += PAGE_SIZE` first. Not an infinite
loop (the cleared/restored PTE routes the next iteration to an advancing arm),
but each foreign frame is walked twice, and the arm skips `sub_rss` although
the map path DID `add_rss` for device VMA pages — RSS drifts upward for tasks
that unmap drm/GEM windows. Fix: advance before continue (and consider
sub_rss).

### [Low] riscv64/mm/mmu_init.rs:1804 — map_device_page is dead code that would write a 1GB leaf for one 4K page

`ROOT_PAGE_TABLE.set(vpn2, (ppn<<10)|flags)` installs a PGD-LEVEL LEAF — if
ever called it maps an entire 1GB region starting at `phys` with one entry,
silently shadowing everything else in that GB. Zero callers in-tree (the
aarch64 hits are that arch's own function). Delete or rewrite to descend to
L0.

### [Low] riscv64/mm/mm_ops.rs:1590 — contradictory stale comments on pte_lock_for mode

The doc comment says "GLOBAL mode on riscv64: every mm maps to the same
global epoch section" while the body comment says "Split mode on all arches
(since the 2026-10-31 perf review)... writers on different mms run
concurrently" (and the body does `mm.pte_lock()`). The same contradiction is
repeated above `handle_cow_fault` ("GLOBAL-mode PTE section on riscv64") and
free_user_page_tables. The code is split-mode; the older GLOBAL comments
should be purged — a future maintainer auditing INV-LOCK-7 will read the
wrong model.

### [Info] riscv64/mm/page_fault.rs:407-414,1452-1459 — bounded busy-wait spins (16M / 64M iterations) in the user fault path run with SIE=0

The migration-window (16M) and swap-slot-pending (64M) waits spin with
interrupts disabled (user traps do not re-enable SIE) — seconds-scale worst
case with the hart deaf to timer/IPI (softlockup bait, watchdog bait). The
reference blocks interruptibly (wait_on_page / migration entries). Bounded
and documented, but a sched-friendly wait (or at least enabling SIE around
the spin) would match the reference's tolerance.

### [Info] riscv64/mm/mm_ops.rs:1119 — fork COW walk: mid-walk alloc failure leaks partially built child tables

copy_page_table_cow returns None on the first alloc_page_table failure after
child L1/L0 tables were already linked into the unpublished child root; there
is no rollback, so the child tree's allocated tables leak. Rare (OOM during
fork), but Linux's dup_mm tears down partial tables on failure.

---

## Findings (x86_64 tree + one cross-arch Critical)

### [Critical] riscv64/ipi.rs:331 AND aarch64/ipi.rs:298 — csd_flush_queue derives CallSingleData without the list offset: calls list pointers as code

`let csd = node as *mut CallSingleData;` — but `node` is `&csd.list` (field at
offset 16; func@0, info@8). `(*csd).func` therefore reads `list.next` (for a
single-element queue: the static CSD_QUEUES[cpu] head address) and `f(arg)`
jumps to that address — executing list-head bytes as code on the TARGET hart
(KERNPANIC). Trigger: any SMP caller of smp_call_function on these two arches —
the generic sys_membarrier MEMBARRIER_CMD_GLOBAL/GLOBAL_EXPEDITED arm
(syscall/process.rs:4117) calls it on every other started CPU. The x86_64 twin
(ipi.rs:364-371) has the fix with an explicit comment ("a bare cast reads the
link pointers as func/info and calls the queue head") — riscv64 and aarch64
were left with exactly the bug the comment describes. Fix: port the
`offset_of!(CallSingleData, list)` + `.sub(list_off)` container_of to both.

ALSO: aarch64/ipi.rs:120-129 flush_tlb_others carries the same count-vs-set
bug as the riscv64 twin (0..min(online, MAX_CPUS) instead of the started set).

### [Low] syscall/process.rs:4115 (generic, arch consumer) — membarrier targets CPUs [0, online_count), same count-vs-set bug as riscv64 flush_tlb_others

`for cpu in 0..online.min(MAX_CPUS)` — with a sparse started set the loop
fences a dead hart (no-op on x86 via the lapic_id sentinel) and SKIPS a live
one, violating membarrier's "every running CPU" contract. Use the started-set
iteration (x86_64 ipi.rs:93 flush_tlb_others already does it correctly).

### [Low] x86_64/process.rs:141 — ARCH_SET_GS accepts a non-canonical base; SET_FS's bound check not applied

ARCH_SET_FS rejects addr >= 1<<47 with EFAULT, but ARCH_SET_GS stores and
wrmsr's any value (reference arch_prctl validates both with
`addr >= TASK_SIZE_MAX → -EPERM`; note the reference uses EPERM, not EFAULT,
for both). A non-canonical GS base programs fine via wrmsr and later #GPs the
task in ring 3 unrecoverably instead of failing the syscall.

---

## Findings (aarch64 tree)

### [Low] aarch64/context.rs:111-112 — switch-sp alignment gate is inverted: accepts an 8-mod-16 sp

`tbz x9,#4, 2f; tbnz x9,#3, .Lswitch_sp_bad` — the bit-3 (8-mod-16) test only
runs when bit 4 is SET, so an sp ending in 0x8 (bit3=1, bit4=0) SKIPS the
check and passes the gate; an sp ending in 0x18 is rejected. The riscv64 twin
tests `andi t1,t6,0xF; bnez bad` (all four low bits). The gate is defensive
(R74 thread.sp validation), so impact is a permissively-accepted misaligned
resume rather than a live fault. Also `.Lswitch_sp_bad` parks in a WFE loop
with no repair and no report — the riscv64 twin repairs to the task's own
stack top; here the whole CPU is lost silently (documented stage-2 TODO, but
a one-line raw print would make the park attributable).

### [Info] aarch64/trap.rs / context.rs — general quality

Read end to end: trap.rs (ESR decode, SVC pc-advance fix, handler-first
signal routing, one-line NOVMA witness), trap.S (frame protocol, SPSR.M
source test with the tbz-fix comment, FPSIMD entry/exit bracket,
ret_from_fork pair), context.rs (eager FPSIMD switch, TPIDR_EL0 TLS
save/restore, R56 affirm gate, switch_mm ASID-scoped tlbi aside1is with the
correct <<48 operand). No further defects found in these three files.

---

## Findings (final batch: x86_64 mm + aarch64 remainder)

### [Low] x86_64/mm/asid.rs:225-233 — flush_tlb_asid with PCID on does a full all-contexts flush of every CPU instead of a single-PCID invlpg

`flush_tlb_asid(asid != 0)` calls flush_all_cpus_local() → INVPCID type 3 on
every CPU (plus a synchronous smp_call_function round trip each). INVPCID
type 1 (single-context) sent as the remote payload, or at minimum type 0 for
the matching address class, is the targeted operation the ISA provides; the
current form makes mm teardown flush the ENTIRE TLB of every core — the same
whole-TLB cost the PCID layer exists to avoid. Correctness unaffected.

### [Low] x86_64/trap.rs:1847 — every user #GP prints an unconditional pr_info

`handle_general_protection` leads with pr_info for every #GP (and
handle_illegal_instruction builds a 16-byte hex dump string per #UD). A
segv-storming program (fuzzers, LTP crashers) spams the polled UART — the
same serial-cost class the repo's own print-audit lane polices. Cap or gate
behind a debug switch like the riscv64 SIGILL budget.

### [Info] aarch64/dtb_blob.S:5 — checked-in copy references a foreign absolute path

`.incbin "/home/william/workspace/Rux/kernel/src/arch/aarch64/virt.dtb"` —
build.rs regenerates this file with the correct path at build time, so the
checked-in copy is dead content; if a build ever used it verbatim the build
fails loudly. Consider deleting the stale copy or making it a relative path.

### [Info] aarch64/mm spot-verification (this round)

Quota ended before a second full pass of aarch64 mm/{memory_layout,
pagetable, asid, page_fault, mmu_init, mm_ops} (~5.2K lines). Verified this
round: memory_layout VirtAddr::new sign-extension/masking (finding above),
mmu_init map_device_page + best_map_size, mm_ops handle_cow_fault exclusive
fast path (W|R together — no riscv64-style W-without-R), asid consumption via
switch_mm (tlbi aside1is <<48 operand correct). Prior rounds read all six
end to end with fixes landed (aarch64-port campaign; see review-coverage.md
🔧 rows: asid 4ee3e430, exception cfc04591, boot.S 6e808eea, trap.rs 00338fd2).
No NEW defects found in the verified hotspots.

---

## Totals

| Severity | Count |
|---|---:|
| Critical | 1 (spanning riscv64 + aarch64) |
| Medium | 6 |
| Low | 13 |
| Info | 9 |
| **Total** | **29** |

Critical: CSD container-of (riscv64+aarch64).
Medium: traced-ebreak epc advance (rv64); unknown-exception SIGKILL (rv64);
virt_to_phys stale image-window bound (rv64); flush_tlb_others count-vs-set
(rv64+aarch64); NOVMA-EXEC forensic flood (rv64); non-canonical-VA guard on
masked address (rv64+aarch64).
Low: c.ecall +4 parity note (rv64); dead hardirq resched branch (rv64);
hardcoded ti_cpu offset (rv64); hardcoded MAX_CPUS in asm (rv64); hardcoded
10MHz timebase (rv64); dead copy_thread (rv64); cacheable UART fixmap (rv64);
foreign-frame continue w/o advance (rv64); dead map_device_page 1GB leaf
(rv64); stale pte_lock_for comments (rv64); membarrier count-vs-set (generic);
ARCH_SET_GS bound (x86); switch-sp alignment gate inverted (aarch64);
flush_tlb_asid full flush (x86); per-#GP pr_info (x86). (Two of these are
paired under the same finding as Medium twins where noted.)

## Three worst problems

1. **[Critical] riscv64+aarch64 csd_flush_queue container-of bug** — calls
   list-head pointers as code on the target hart; reachable TODAY via the
   generic sys_membarrier(GLOBAL) path on any SMP boot of those arches.
   One-line fix per arch (port the x86_64 list_off subtraction).
2. **[Medium] riscv64 virt_to_phys stale 16MB image-window bound** —
   .bss/.data VAs above 0xffffffff81000000 (the image is 20-24MB now) fall
   through to the warn arm and return a VIRTUAL address as physical to DMA
   consumers; live on any debug build (the default) the moment a static
   driver buffer lands above 16MB.
3. **[Medium] NOVMA-EXEC forensic flood (riscv64 exception.rs)** — ~250
   lines of ungated instrumentation with hardcoded rootfs-specific addresses
   execute for every user exec fault at an unmapped VA (every wild jump in
   every crashing program): tens of pr_err lines through the polled UART per
   event, on the primary-arch fault path. The x86_64 twin reduced this to
   one line; port that shape.

## Coverage appendix (this round, r4-arch)

Legend: ✅ read end to end this round; 🔶 partially read this round (note says
what); prior-round coverage per docs/development/review-coverage.md stands.

| file | lines | status |
|---|---:|---|
| arch/mod.rs | 73 | ✅ |
| arch/uaccess_safe.rs | 72 | ✅ |
| riscv64/mod.rs | 132 | ✅ |
| riscv64/boot.rs | 32 | ✅ |
| riscv64/cpu.rs | 184 | ✅ |
| riscv64/pt_regs.rs | 458 | ✅ |
| riscv64/vext.rs | 386 | ✅ (encodings verified against riscv64-linux-gnu-as) |
| riscv64/uaccess.rs | 512 | ✅ |
| riscv64/uaccess.S | 434 | ✅ |
| riscv64/boot.S | 695 | ✅ |
| riscv64/linker.ld | 101 | ✅ |
| riscv64/trap.S | 1215 | ✅ |
| riscv64/trap.rs | 2003 | ✅ |
| riscv64/context.rs | 587 | ✅ |
| riscv64/thread.rs | 525 | ✅ |
| riscv64/process.rs | 332 | ✅ |
| riscv64/smp.rs | 348 | ✅ |
| riscv64/ipi.rs | 512 | ✅ |
| riscv64/mm/mod.rs | 53 | ✅ |
| riscv64/mm/memory_layout.rs | 569 | ✅ |
| riscv64/mm/pagetable.rs | 294 | ✅ |
| riscv64/mm/asid.rs | 302 | ✅ |
| riscv64/mm/fixmap.rs | 267 | ✅ |
| riscv64/mm/exception.rs | 721 | ✅ |
| riscv64/mm/page_fault.rs | 1559 | ✅ |
| riscv64/mm/mm_ops.rs | 2737 | ✅ |
| riscv64/mm/mmu_init.rs | 2162 | ✅ |
| x86_64/mod.rs | 66 | ✅ |
| x86_64/pt_regs.rs | 345 | ✅ |
| x86_64/cpu.rs | 560 | ✅ (xsave full-mask verified safe on real hardware) |
| x86_64/process.rs | 168 | ✅ |
| x86_64/boot.rs | 242 | ✅ |
| x86_64/thread.rs | 436 | ✅ |
| x86_64/setup.S | 128 | ✅ |
| x86_64/boot.S | 206 | ✅ |
| x86_64/smp_trampoline.S | 182 | ✅ |
| x86_64/linker.ld | 119 | ✅ |
| x86_64/linker-extra.ld | 21 | ✅ |
| x86_64/uaccess.rs | 570 | ✅ |
| x86_64/ipi.rs | 389 | ✅ |
| x86_64/smp.rs | 622 | ✅ |
| x86_64/trap.S | 690 | ✅ |
| x86_64/trap.rs | 3233 | ✅ |
| x86_64/context.rs | 850 | ✅ |
| x86_64/mm/mod.rs | 37 | ✅ |
| x86_64/mm/pagetable.rs | 199 | ✅ |
| x86_64/mm/fixmap.rs | 122 | ✅ |
| x86_64/mm/memory_layout.rs | 439 | ✅ |
| x86_64/mm/asid.rs | 351 | ✅ |
| x86_64/mm/exception.rs | 617 | ✅ |
| x86_64/mm/page_fault.rs | 2358 | ✅ |
| x86_64/mm/mm_ops.rs | 3448 | ✅ |
| x86_64/mm/mmu_init.rs | 2307 | ✅ |
| aarch64/mod.rs | 59 | ✅ |
| aarch64/boot.rs | 30 | ✅ |
| aarch64/dtb_blob.S | 7 | ✅ |
| aarch64/cpu.rs | 201 | ✅ |
| aarch64/pt_regs.rs | 405 | ✅ |
| aarch64/thread.rs | 274 | ✅ |
| aarch64/process.rs | 164 | ✅ |
| aarch64/context.rs | 455 | ✅ |
| aarch64/smp.rs | 626 | ✅ |
| aarch64/ipi.rs | 370 | ✅ |
| aarch64/trap.S | 367 | ✅ |
| aarch64/trap.rs | 896 | ✅ |
| aarch64/uaccess.rs | 514 | ✅ |
| aarch64/linker.ld | 107 | ✅ |
| aarch64/boot/boot.S | 482 | ✅ |
| aarch64/mm/mod.rs | 44 | ✅ |
| aarch64/mm/fixmap.rs | 112 | ✅ |
| aarch64/mm/exception.rs | 294 | ✅ |
| aarch64/mm/memory_layout.rs | 459 | 🔶 VirtAddr::new + user_addr verified; rest from prior round |
| aarch64/mm/pagetable.rs | 554 | 🔶 prior round full; r4 hotspots only |
| aarch64/mm/asid.rs | 320 | 🔶 consumption sites verified; rest from prior round |
| aarch64/mm/page_fault.rs | 691 | 🔶 entry walk/marker arm verified; rest from prior round |
| aarch64/mm/mmu_init.rs | 963 | 🔶 map_device_page/best_map_size verified; rest prior round |
| aarch64/mm/mm_ops.rs | 2183 | 🔶 COW fast path + pte_lock sites verified; rest prior |

65 files: 65 ✅/🔶 covered (6 🔶 with named hotspots verified this round;
their prior-round full reads + fixes are on record in review-coverage.md).
