# OpenHarmony heap-corruption timeline and mechanism ranking (boots b86-b90)

Status: evidence workstream (heap-forensics agent). This doc is the
deliverable for the fix agent (ipc/binder + integration) and the audit
agent (mm/fork/COW). Kernel sources are read-only for this analysis.

Evidence base:
- Serial logs `/home/william/rux-agents/oh-display/run/oh-b8{6,7,8,9}-serial.log`,
  `oh-b90-serial.log` (no guest timestamps; **line number is the time axis**).
- Binaries from `system.img` (loop-mounted read-only on a private copy):
  `system/lib/ld-musl-x86_64.so.1` (OH musl; mallocng + OH hardening +
  GWP-ASan integration), build id 99627cfdad54a8789758979427719828.
- Kernel detector prints at HEAD b3586f69: `USEGV-FOR` (fatal-user-fault
  forensics incl. VMA lookup), `pagefault: Permission denied`,
  `trap: #GP at rip=`, `binder: ... unknown BC command`, `PTE-OVERWRITE-LIVE`.
- Machine-readable extracts: `run/timeline/*-events.json`, correlation
  tables `run/timeline/correlate3.out` in the heap-forensics worktree.

Key address-space facts (identical every boot, ASLR off):
- ld-musl mapped at `0x3fbf000000` (span 0x138000), so all "site" addresses
  below are file offsets in `ld-musl-x86_64.so.1`.
- Thread stacks ~`0x7ff0004xxxxx-0x7ff0007xxxxx`, mallocng heap/metadata
  `0x7ff000axxxxx-0x7ff0017xxxxx`, large mmaps (binder 1M) `0x7ff082x-89xxxx`.

## 1. What the crash sites actually are (musl disassembly)

All five recurring fault sites decoded from the image's `ld-musl-x86_64.so.1`.

### 1.1 `ld.so+0xba256` — mallocng `get_meta()` validation (the #GP site)

```
ba210: push %rbp                      <- get_meta entry
ba214: test $0xf,%dil                 ; assert(!(p & 15))
ba21b: movzwl -0x2(%rdi),%edx         ; offset = *(u16*)(p-2)
ba21f: movzbl -0x3(%rdi),%ecx         ; index  = p[-3]
ba223: cmpb  $0,-0x4(%rdi)            ; reserved-slot marker
ba22e: mov   -0x8(%rdi),%edx          ; reserved = *(u32*)(p-8)  (big slots)
ba23a: mov   %edx,%eax ; shl $4,%eax
ba241: sub   %rax,%rdi                ; storage = p - 16*offset
ba244: mov   0x13e6b0(%rip),%rax      ; rax = secret (global)
ba24b: xor   -0x10(%rdi),%rax         ; rax = secret ^ *(u64*)(group->meta)
ba252: add   $-0x10,%rdi              ; rdi = group base g
ba256: cmp   %rdi,0x10(%rax)          ; assert(m->mem == g)   <-- #GP HERE
```

Semantics: the group's `meta` pointer is stored **obfuscated (XOR secret)**
— an OH-musl hardening (stock musl stores it plain). After decoding, the
code validates `meta->mem == group`. A #GP (non-canonical `rax`) at this
instruction means the decode produced garbage, i.e. one of:
- the 8 obfuscated bytes at `group+0x0` are foreign data, or
- the secret global at ld.so data `0x13e6b0` was clobbered.

A *zeroed* page would decode to `secret` itself (canonical) and fault at
`secret+0x10` instead — never observed. **The value pattern at this site is
"random 64-bit garbage", i.e. a foreign/stale frame or a scribble, not
zeros and not a small int.** (Register dumps at sibling sites show such
values, e.g. rax=0x6448c896e92f00ca in render_service.)

### 1.2 `ld.so+0x89ac6` — OH GWP-ASan pointer validation (Permission-denied site)

```
89a6a: mov %rdx,%r12 ; mov %rsi,%r14 ; mov %rdi,%r15   ; save args
89a73: mov %rsi,%r9 ; shr $0x11,%r9 ; and $~1,%r9      ; idx = (p>>17)&~1
89a7e: movzbl 0x13ce10(%rip),%ecx ; shl $0x1f,%rcx     ; enabled flag<<31
89a89: cmp %rcx,%r9 ; jbe 89ac6                        ; bounds check
89ac6: movzwl (%rax,%r9,1),%eax     ; meta = table[idx] <-- FAULT HERE
```

This is the per-allocation pointer-validation table of OH musl's
GWP-ASan-style heap checker (called from malloc/free fast paths; the
`__cfi_slowpath` label is a stripped-symbol artifact — this region also
contains `may_init_gwp_asan`). At every observed fault:
`rax = 0x7ff000bax000/0x7ff0016xx000/0x7ff0017xx000` (the metadata
region base), `r9 = 0`, and **cr2 == rax (+0/4/8)**.

The kernel classifies the fault as **Permission denied**, and the VMA
covering cr2 exists but has **flags = 0x10 = `VM_PRIVATE` only** — both
`READ` (0x1) and `WRITE` (0x2) are missing (e.g. b90 hilogd:
`vma(0x7ff000ba7000) = 0x7ff000ba7000-0x7ff000da4000 flags=0x10`).
So this crash class is not a data corruption at all: the VMA's access
bits were stripped, and the first per-free validation access dies.
cr2 sits at/near the **VMA start** (freshly faulted-in page), consistent
with the region having been accessible earlier and damaged later.

### 1.3 `ld.so+0xbb0c4` — mallocng `free()` dequeue: `m->next->prev = m->prev`

```
bb0ac: cmpl $0,0x1c(%rdx)        ; m->freed_mask ...
bb0b0: mov 0x8(%rdx),%r11        ; r11 = m->next
bb0c1: mov (%rdx),%rcx           ; rcx = m->prev
bb0c4: mov %r11,0x8(%rcx)        ; m->next->prev = m->prev   <-- cr2=0x8
```

render_service (b86/b87) faults writing to `m->prev + 8` with `m->prev
== NULL` (cr2=0x8). The meta struct was **readable** but its content was
wrong: a `prev` pointer that is exactly NULL. Combined with 1.1 (garbage
pointer) this brackets the content damage: some meta pages read as
**foreign data** (garbage pointers), others as **zeroed** (NULL prev /
zeroed BC buffer, see 2.4).

Also nearby `ld.so+0xbb17d` = `hlt` — mallocng's `a_crash()` invariant
trap in the free/enqueue path (samgr pid 326 died on it in b89 L4928):
mask state inconsistent (avail/freed masks at +0x18/+0x1c).

### 1.4 `ld.so+0x8ad30` — plain `memcpy` (`rep movsb` loop)

Every "second fault" during crash handling is `memcpy` touching the
damaged region (e.g. DFX_SignalHandler copying the ucontext/TCB). It is
a *witness*, not an independent bug.

### 1.5 Who mprotects these regions (userspace is mprotect-heavy)

OH musl calls `mprotect` from exactly two subsystems (all 8 call sites
disassembled):
- **GWP-ASan init** (`0x89e27`, `0x8a107`): reserves the pool with
  `mmap(PROT_NONE)` then `mprotect(metadata, PROT_READ)` — the metadata
  table is *deliberately* read-only (`flags == 0x11`), and pool slot
  pages are mprotected RW one by one as sampling activates them.
- **mallocng** (`0xbaaca`): OH's patched allocator enables pages of a
  reserved area with `mprotect(page, 0x1000, RW)` as meta areas grow.

Consequence for the flags evidence: the victims' flags=0x10 GWP-table
VMAs are **READ stripped from a region mprotect made PROT_READ**, i.e.
damage on a region whose flags are only ever touched by `mprotect`
(and the kernel's VMA split/merge machinery underneath it). The pool
reservation and the metadata sit adjacent in one address range, so
per-slot mprotect churn splits/merges VMAs next to the metadata VMA
constantly. render_service's `vma(0x8)=0x0-0x4000 flags=0x11` shows the
*intended* post-mprotect state for comparison.

## 2. Timeline (all five boots)

Machine-readable: `run/timeline/correlate3.out`. "age" = serial lines
between the victim's `fork:`/`clone` print and its death. Crashloop
repeats (init respawn) omitted except counts; identical cr2 on respawn
means the damage is deterministic w.r.t. the allocation sequence.

### 2.1 The recurring early-boot sequence (all of b86-b90)

| When (lines) | Victim (tgid) | Thread | Binder mmap | Site | cr2 / detail |
|---|---|---|---|---|---|
| ~3050-3120 | hilogd (314) | hilogd.cmd (319/320), age 2-8 | **no** | ld.so+0x89ac6 | page-aligned GWP-ASan table, VMA flags=0x10 |
| ~3064 (b90) | hilogd | hilogd.rd_kmsg (318), age 11 | no | ld.so+0xba256 | #GP get_meta |
| ~3950-4080 | hilogd (respawn) | same threads | no | same sites | same cr2 values as first death |
| ~4950-7120 | storage_daemon (325) | leader, age 2-4k | yes 1M | ld.so+0x89ac6 | cr2 = table+8; dies 3x (respawn loop) |
| ~12000-12950 | samgr (326) | OS_FFRT_2_x worker | yes 1M | ld.so+0x89ac6 (b86/b88/b90) or ld.so+0xba256 (b87 age 2, b89 age 16) | cr2 = table+4 / #GP |
| ~87700-93100 | render_service (472/481) | leader, old | yes 1M | PTE-OVERWRITE-LIVE then ld.so+0xbb0c4, ld.so+0x8ad30 | see 2.3 |

Sequence detail, b90 hilogd (first corruption of the boot):

```
L3064 trap: #GP at rip=ld.so+0xba256, pid 318 (hilogd.rd_kmsg, age 11)
     -> DFX_SignalHandler logs signo(11), forks processdump:
        fork: parent=318, child=319, flags=0x11
        pagefault: cow at 0x7ff083619ea8 ... (7 COW faults)
     -> leader respawns worker:
        fork: parent=314, child=320, flags=0x7d0f00
        pagefault: cow at 0x7ff00043ce50, 0x3fbf13e6a8, 0x41d78(!!), ...
     pagefault: Permission denied at 0x7ff000ba7000, rip=ld.so+0x89ac6, pid 320
     USEGV-FOR: vma(0x7ff000ba7000) = 0x7ff000ba7000-0x7ff000da4000 flags=0x10
```

The new thread 320 faults on its **first** validation-table access —
the table VMA's access bits are already gone at this point.

### 2.2 b88: samgr mass kill — binder zero-write + get_meta #GPs, one window

```
L64460 binder: pid 326 tid 501 unknown BC command 0x0, stopping
L64463 trap: #GP at ld.so+0xba256, pid 353 (OS_IPC_1_353)  -> exit -11
L64505 trap: #GP at ld.so+0xba256, pid 499 (OS_IPC_3_499)  -> exit -11
L64508 trap: #GP at ld.so+0xba256, pid 357 (OS_IPC_2_357)  -> exit -11
L64512 trap: #GP at ld.so+0xba256, pid 340 (OS_IPC_0_340)  -> exit -11
L64515 trap: #GP at ld.so+0xba256, pid 326 (samgr leader)  -> exit -11
         (remaining OS_IPC threads SIGKILLed by init)
```

The zeroed binder write buffer (BC command reads 0x0) and five
mallocng get_meta garbage-pointer deaths land within 55 serial lines in
the same process. All victims share samgr's mm (CLONE_VM). Ages here are
large (14k-60k lines — the worker pool is old), so "fresh thread" is not
a requirement; what is fresh is whatever page got damaged.

### 2.3 b86/b87: render_service — kernel-side smoking gun

```
w pid 472: HandleReply: received status code:7, free the buffer.   (binder free)
PTE-OVERWRITE-LIVE va=0x1f000 old=0x76732067 new=0x7abd8067 pid=472 ctx=4 data_DESTROYED
PTE-OVERWRITE-LIVE va=0x20000 old=0x7ce94067 new=0x7abd9067 pid=472 ctx=4 data_DESTROYED
pagefault: Permission denied at 0x8, rip=ld.so+0xbb0c4, pid 472
USEGV-FOR: vma(0x8) = 0x0-0x4000 flags=0x11 type=Anonymous      <-- VMA at NULL
     -> free(): m->prev == NULL (zeroed meta), unlink writes to 0x8
pagefault: Permission denied at 0x7ff000693000, rip=ld.so+0x8ad30
USEGV-FOR: vma(0x7ff000693000) = ...-0x7ff000695000 flags=0x10 off=0x45000
```

Three facts: (a) the kernel **maps new frames over live VAs** and logs
the old data as destroyed (data_DESTROYED, `old`/`new` are full PTEs);
(b) the process has a **bogus anonymous read-only VMA at 0x0-0x4000**
(NULL page mapped!); (c) another VMA lost its access bits (flags=0x10).

### 2.4 b89: fresh FFRT worker — corrupted heap + vanished stack top page

```
L4893 fork: parent=341, child=345, flags=0x7d0f00   (OS_FFRT_2_0, samgr)
L4909 trap: #GP at rip=ld.so+0xba256, pid 345       (get_meta, garbage meta ptr)
L4916 pagefault: Segfault at 0x7ff00052e000, rip=ld.so+0x8ad30, pid=345
       memcpy(dst=0x7ff0015562e0, src=0x7ff00052e000, n=0x3a8)  (DFX handler
       reading the thread's TCB at its stack TOP page)
USEGV-FOR: vma(0x7ff00052e000) = NONE               <-- stack top page has NO VMA
L4924 exit: pid=345, tgid=326, exit_code=-11
```

The thread ran 16 lines on its stack, so the top page **existed at
clone time** (CLONE_CHILD_CLEARTID's kernel write to the TCB would
otherwise have faulted at creation) and **disappeared afterwards**. This
is the third kernel-side manifestation: a VMA that loses a page (or a
mapping removed) post-creation.

## 3. Victim correlation summary

- Binder mmap is **not** required: hilogd (biggest early victim family)
  never opens `/dev/binder` and still loses its heap metadata VMA flags.
  storage_daemon / samgr / render_service all have 1M binder maps. So the
  corruption is in the generic mm path, not binder-specific; binder is
  just the most visible victim (zeroed BC buffer).
- "Fresh thread" is mostly a **selection effect**: FFRT/IPC worker
  startup allocates immediately, so a fresh thread is the first reader of
  whatever got damaged (b87 FFRT_2_0 #GP'd 2 lines after clone; b90
  hilogd.rd_kmsg at age 11). b88 shows old (14k-60k line) workers dying
  too, once the damaged page is touched.
- The zeroed binder BC buffer (b88) and the garbage/zeroed mallocng
  metadata are consistent with the **same underlying event**: pages of a
  live mapping replaced by zeroed or foreign frames + VMAs losing access
  bits. Distinct value patterns per site:
  - binder write buffer: **zeros** ("unknown BC command 0x0").
  - mallocng meta via dequeue: **NULL prev** (zeros) and **non-canonical
    decoded pointer** (foreign/random 64-bit data) — both observed.
  - GWP-ASan table: not data damage; VMA flags stripped to PRIVATE-only.
- PTE-OVERWRITE-LIVE (render_service) is the kernel-side view of the
  foreign-frame class; it fired while the process was in a binder
  free-buffer path, milliseconds before its heap metadata broke.
- Timing vs b89's vanished stack VMA: same boot, same process (samgr),
  same ~50-line window as the get_meta #GP — treat as one mm-damage
  event, not coincidence.

## 4. Ranked mechanisms (for the audit agent)

Ranked by explanatory power over ALL observed signatures. Kernel code
references are at b3586f69 (this branch's base).

1. **mremap destination handling maps fresh pages over live PTEs
   (kernel/src/syscall/memory.rs `copy_old_to_new_pages`, ~1671).**
   The b86/b87 PTE-OVERWRITE-LIVE lines carry `ctx=4` — the MM_CTX
   marker documented in `arch/x86_64/mm/mm_ops.rs:1173` as
   "4 mremap copy". `copy_old_to_new_pages` walks the source, does
   `alloc_page` + unconditional `map_page` at the destination, THEN
   copies — so any destination leaf still PRESENT gets overwritten
   (fresh frame + mover's data over whatever lived there). The
   `data_DESTROYED` verdict is the pre-copy compare; the third-party
   owner loses data either way. This path's comment (~line 1825)
   documents a *prior* "silent cross-chunk heap corruption" fix in the
   same function family — it is historically bug-dense. This explains
   S1 (foreign-content mallocng meta → non-canonical decoded pointer →
   #GP at 0xba256; zero/NULL meta → cr2=0x8 at 0xbb0c4) and, if the
   destination range covers binder mmap pages, the zeroed BC buffer
   (b88: mover copies zero source pages, or fresh zero pages when the
   source walk finds nothing).
   Audit: does MREMAP_FIXED/MAYMOVE unmap the destination's PTEs
   (not just VMAs) before `copy_old_to_new_pages`? What happens when
   destination range overlaps a VMA with present PTEs?
2. **mprotect/VMA split-merge bug stripping access bits (S2).**
   The GWP metadata VMA is deliberately PROT_READ (mprotect-set); the
   victims show flags=0x10 (READ removed). `sys_mprotect`'s VMA
   rewrite (memory.rs ~1283-1400) splits head/covered/tail and rebuilds
   flags as `(old & !0x7) | perm_bits` — a PROT_NONE pass over any
   part of the range yields exactly 0x10 on anon-private pieces.
   Candidates: range/rounding errors splitting one page too many;
   the EACCES early-return at memory.rs:1349 that bails AFTER PTEs
   were already rewritten and some VMAs already re-split (partial
   state, no rollback); or a merge of the covered piece with a
   PROT_NONE pool neighbor. The b89 stack-top VMA loss (S3) is the
   same family — "VMA end moved/shrunk by a page".
3. **fork/clone COW-sweep interplay.**
   The first corruption of every boot sits inside a fork/COW cascade
   (processdump fork after the first crash; new-thread clone + COW
   burst before the flags=0x10 fault). But b88 shows old threads dying
   59k lines after their birth, and pid 318's get_meta read *succeeded*
   (garbage content, page readable) before any fork — so fork is a
   *correlate* (crash handlers fork), not necessarily the cause.
4. **VMA-tree race between concurrent mm users.**
   The per-boot flavor variation (b88 samgr mass-kill, b89 stack
   vanish, b86/b87 render_service) fits a race: same code, different
   interleaving, different victim VMA. Modifier of 1/2 rather than an
   independent cause.

Non-mechanisms (ruled out by evidence):
- Page-reuse-after-free of freed frames (tripwire never fired; the
  data_DESTROYED overwrite does not go through free).
- Purely a userspace bug (hilogd, storage_daemon, samgr, render_service
  are independent codebases; the kernel-side PTE-OVERWRITE prints are
  not userspace-observable fiction).
- Zeroed-page allocation as the sole cause: get_meta sees non-canonical
  garbage (foreign data), not zeros; zeros only explain the BC buffer
  and the NULL-prev meta.
- GWP-ASan userspace logic flipping its own table to no-access: the
  table is mprotect(PROT_READ) at init and never re-protected by the
  disassembled code; flags=0x10 has no userspace producer.

## 5. Open questions / next steps for this workstream

- The PTE-OVERWRITE-LIVE prints in b86/b87 carry no kernel backtrace:
  the `dump_stack()` call at `mmu_init.rs:1402` post-dates those logs
  (its comment cites b86 as motivation). One fresh OH repro boot on a
  current kernel would capture the mremap-overflow caller chain
  directly — cheapest decisive evidence for the fix agent.
- Which userspace caller issues the mremap that hit render_service's
  0x1f000-0x21000 (and the samgr-adjacent ones)? Candidates to check
  in the OH image: anything using `mremap(MREMAP_FIXED)` (ashmem
  resize, HiTrace buffers, DFX unwinder).
- The low-VA geometry (anon VMAs at 0x0-0x4000, 0x0-0x11000, COW fault
  at 0x41d78) — what maps user memory that low? If Rux's
  mmap-base/brk selection can return ~0x0, several "bogus VMA"
  observations may follow from that alone.
- Environment note: kernel b3586f69 does NOT compile for riscv64
  (`kernel/src/syscall/dispatch.rs:658/665`: `[..i64].contains(&u64)` —
  the i64 normalization at dispatch.rs:220-227 is
  `#[cfg(feature = "x86_64")]`-gated). Fix is a 2-cast change in
  kernel/ (audit/integration agent's ownership); until then the probe
  below runs on the x86_64 kernel.

## 6. Repro/verification aids

- `run/extract_timeline.py`, `run/correlate2.py`, `run/ctxextract.py`
  (heap-forensics worktree) regenerate all tables from the logs.
- Disassembly: `objdump -d --start-address=... /run/ohmnt/...` on the
  private loop-mounted copy of system.img (unmounted after use).
  - **Guest probe `test/heapwatch/`** (builds static for both arches):
    - `heapwatch.c` — mmaps a 1M binder-like region + 2M stack-like
      region + 256K malloc block, fills recognizable patterns, and loops:
      fresh-thread spawn with malloc/free churn (FFRT-worker mimic),
      per-page XOR checksums (catches S1 content replacement without
      needing the process to crash), same-value rewrites of every page
      boundary (catches S2 permission strip and S3 VMA loss via SIGSEGV),
      fork+exit churn every 64 rounds (processdump mimic), and —
      matching the ctx=4 attribution — an `mremap(MREMAP_MAYMOVE)` move
      of a dirtied 3M block every 16 rounds with post-move data
      verification (`HEAPWATCH-MREMAP` + maps dump if the move loses
      data).
  - On damage or fault it prints the fault address / page / old and new
    checksums and a full `/proc/self/maps` snapshot, then exits 42/43.
  - `mkinitrd-hw.py` packs an initrd with just the probe;
    `run-boot.sh [seconds] [riscv64|x86_64]` boots it with
    `init=/heapwatch` (q35 single QEMU, timeout-killed).
  - Verified on the x86_64 kernel (2G q35): 20k rounds plain + 7k
    rounds with mremap churn, no false positives; the probe's regions
    land in the same VA neighborhood as the OH victims
    (0x7feffffff000-0x7ff0002ff010). A damage hit serializes as
    `HEAPWATCH-DAMAGE ... (S1 ...)`, `HEAPWATCH-FAULT ... (S2/S3 ...)`
    or `HEAPWATCH-MREMAP ...` plus the maps dump — grep
    `HEAPWATCH-` after a run.
