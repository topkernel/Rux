# Formal Verification Test Report

> **Last updated**: 2026-04-09
> **Test command**: `cd kernel/verify && cargo test --target x86_64-unknown-linux-gnu`
> **Sync check**: `python3 scripts/verify_sync_check.py`

## Summary

| Metric | Value |
|--------|-------|
| **Total test cases** | 1,088 |
| **Test modules** | 98 |
| **Kernel subsystems covered** | 11 (mm, sync, arch, net, fs, security, signal, process, sched, interrupt, ipc, drivers) + errno |
| **Test framework** | [proptest](https://crates.io/crates/proptest) 1.5 (property-based, randomized) |
| **Environment** | std, host machine, `x86_64-unknown-linux-gnu` target |
| **Default cases per test** | 256 (configurable via `PROPTEST_CASES`) |
| **Result** | 1,087 passed, 1 failed (pre-existing) |

## Approach

Each test file copies the relevant pure types and functions from `kernel/src/` into `kernel/verify/src/` and verifies invariants using proptest randomized input generation. This avoids a shared-crate dependency chain while keeping kernel source clean. When kernel types change, the copies here must be updated accordingly — the sync check script detects divergences automatically.

## Test Modules

### mm/ (Memory Management) — 264 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
...existing entries...

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `page_flags_test` | 6 | `mm/page_desc.rs` | Bitmap set/test/clear, from_raw, clear_all, test_and_set |
| `buddy_test` | 11 | `mm/buddy_allocator.rs` | Alignment, buddy involution, pair contiguity, size_to_order, get_buddy_idx |
| `vma_test` | 9 | `mm/vma.rs` | Non-overlap, adjacent VMAs, overlap rejection, find, remove, split, contains, overlaps, can_merge |
| `refcount_test` | 6 | `mm/page_desc.rs` | Never negative, get/put symmetry, underflow protection, try_get |
| `list_test` | 10 | `mm/list.rs` | Circular list integrity, add/del, FIFO/LIFO, forward/backward symmetry, for_each |
| `buddy_alloc_test` | 11 | `mm/buddy_allocator.rs` | Order calculation, buddy involution, addr roundtrip, alloc+free conservation, merging |
| `zone_test` | 13 | `mm/zone.rs` | Newton's method int_sqrt, pfn/phys roundtrip, GFP→zone mapping, watermark formula |
| `vmscan_test` | 14 | `mm/vmscan.rs` | nr_to_scan priority-shift formula, ScanControl reclaim target, priority loop termination, LRU index bounds |
| `compact_test` | 16 | `mm/compact.rs` | CompactResult enum, scanner convergence, MAX_SCAN_PAGES limit, migration filter predicate (free/reserved/dirty/refcount) |
| `rmap_test` | 16 | `mm/rmap.rs` | Sv39 VPN extraction/reconstruction roundtrip, addr_to_vpn bounds, page_mapped/mapcount guards |
| `page_flags_ops_test` | 16 | `mm/page_desc.rs` | PageFlag/PageType enum discriminants, PageFlags set/clear/test_and_set/test_and_clear/clear_all, flag isolation, idempotency |
| `swap_test` | 10 | `mm/swap.rs` | Swap entry encode/decode: make_swap_entry, is_swap_entry, swap_entry_type, swap_entry_offset roundtrip |
| `page_addr_test` | 15 | `mm/page.rs` | PhysAddr/VirtAddr floor/ceil/is_aligned/frame_number/ppn, PhysFrame/VirtPage roundtrip, PAGE_SIZE |
| `slab_test` | 12 | `mm/slab.rs` | Size class lookup: find_cache_index for zero/oversize/exact/between, OBJECT_SIZES monotonicity/power-of-2/doubling |
| `hugepage_test` | 16 | `mm/hugepage.rs` | Shift hierarchy (PAGE<PMD<PGDIR), size power-of-2 (2MB/1GB), mask coverage, alignment round-up/down, HugePageType size/order, PTE/VM flags distinct, kernel vs user huge flags |
| `vmemmap_test` | 6 | `mm/vmemmap.rs` | struct Page = 64 bytes, PAGES_PER_VMEMMAP_PAGE = 64, pfn↔vmemmap roundtrip, vmemmap pages needed, descriptor alignment |
| `config_test` | 16 | `config.rs` | PAGE_SIZE power-of-2, PCP watermark ordering/batch divisibility, heap within physical memory, PID hierarchy/power-of-2, symlink depth nesting, TCP RTO ordering, CFS granularity ≤ latency, KERNEL_HZ divides 1000, stack page-aligned, cache sizes power-of-2 |
| `page_flag_test` | 7 | `mm/page_desc.rs` | 16 PageFlag variants distinct powers-of-2 (bits 0-15), pairwise disjoint, fit in 16 bits, set/unset/toggle/combine operations, Cow=bit14, Anonymous=bit15 |
| `memblock_test` | 10 | `mm/memblock.rs` | Region contains/end, PFN arithmetic roundtrip, page_count, MemBlockFlags distinct (NONE/NOMAP/MIRROR), boundary checks, non-overlap detection, saturating available |
| `layout_test` | 8 | `mm/layout.rs` | KernelMemoryLayout init_from_memblock: heap page-alignment, slab follows heap, user_phys capped at 64MB, quarter rule, frame_alloc accounts all memory, region contiguity |
| `oom_kill_test` | 10 | `mm/oom_kill.rs` | OOM badness scoring: immunity at OOM_SCORE_ADJ_MIN, zero-adj baseline, positive adj increases, negative adj decreases, max adj boost, near-min adj reduction, saturation avoidance, small totalpages no adjustment, symmetry |
| `meminfo_test` | 14 | `mm/meminfo.rs` | is_memory_low (5% threshold), should_trigger_oom (1% threshold), boundary integer division, OOM implies low, heap_usage_percent, mem_used identity |
| `pfn_valid_test` | 12 | `mm/page_desc.rs` | pfn_valid/phys_valid range checks, boundary conditions, far-below/far-above rejection, PFN↔physical address roundtrip, contiguous range, MIN_PFN/MAX_PFN constants |

### sync/ (Synchronization) — 50 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `spinlock_test` | 4 | `sync/spinlock.rs` | try_lock/unlock, lock/unlock, unlock_unlocked, contention |
| `seqlock_test` | 8 | `sync/seqlock.rs` | Initial state, write mutates, locked state, try_write, sequence increments, read consistency, struct atomicity |
| `futex_test` | 16 | `sync/futex.rs` | FutexKey private/shared matching, futex_hash distribution, futex_to_flags, bitset intersection, opcode constants |
| `rwlock_test` | 10 | `sync/rwlock.rs` | WRITER_BIT (bit 31), READER_MASK (bits 30:0), disjoint, full coverage, reader/writer extraction |
| `semaphore_test` | 12 | `sync/semaphore.rs` | Semaphore counter: down decrements, up increments, trylock restores on failure, down/up symmetry, exhaust-and-refill, binary mutex, counting semaphore, zero initial |

### net/ (Networking) — 140 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `route_test` | 11 | `net/ipv4/route.rs` | Longest-prefix match, host route, default route, masking, add/remove, interleaved ops |
| `arp_test` | 14 | `net/arp.rs` | LRU eviction, cache capacity, update/remove, packet parsing, MAC/IP extraction |
| `checksum_test` | 10 | `net/ipv4/checksum.rs` | RFC 1071 ones-complement, zero-length, complement identity, carry fold, pseudo-header |
| `tcp_test` | 16 | `net/tcp.rs` | RFC 6298 RTT estimator, RTO clamping/backoff, RFC 5681 congestion (slow start/CA/timeout), seq_before, TCP header flags |
| `ethernet_test` | 7 | `net/ethernet.rs` | MAC address classification: unicast/multicast/broadcast mutual exclusivity, addr_eq |
| `ipv4_udp_test` | 9 | `net/ipv4/mod.rs`, `net/udp.rs` | IPv4 header version/IHL, big-endian field roundtrips, UDP port/length/protocol accessors |
| `ipv4_test` | 12 | `net/ipv4/mod.rs` | IpHdr layout (20 bytes), fragment flags (RB/DF/MF/OFFSET_MASK), flags+offset disjoint, MTU constants |
| `buffer_test` | 11 | `net/buffer.rs` | EthProtocol/IpProtocol round-trip, IANA values, PacketType discriminants, distinctness |
| `icmp_test` | 6 | `net/icmp.rs` | IcmpHdr layout (8 bytes), field offsets, type constants (ECHO_REPLY/DEST_UNREACH/ECHO_REQUEST/TIME_EXCEEDED) |
| `tcp_state_test` | 11 | `net/tcp.rs` | TcpState 11 discriminants (0-10), distinct, TCP_MAX_HLEN=15*4=60, header_len/dof roundtrip, flag bits (SYN/ACK/FIN/RST/PSH) distinct powers-of-2, MSS=1460, TCP_MAX_WINDOW=u16::MAX |
| `socket_test` | 8 | `net/socket.rs` | SockAddrIn size=16 bytes, port/addr big-endian roundtrip, loopback addr, protocol constants distinct per namespace, SOCK_STREAM=1/SOCK_DGRAM=2, IPPROTO_TCP=6/IPPROTO_UDP=17, AF_INET=2 |
| `transport_checksum_test` | 10 | `net/udp.rs`, `net/icmp.rs`, `net/tcp.rs` | UDP/ICMP/TCP checksum verify property (byte-array, big-endian), empty data, odd-length padding, source IP sensitivity, TCP vs UDP protocol distinction |
| `checksum_verify_test` | 7 | `net/ipv4/checksum.rs` | RFC 1071 extended: even-length verify roundtrip, all-0xFF carry chains, single-bit, large carry folding, word order, UDP pseudo-header combined checksum |

### fs/ (Filesystem) — 276 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `cmdline_test` | 15 | `cmdline.rs` | get_param, has_param, get_all_params, root device, init program, debug mode |
| `stat_test` | 11 | `fs/stat.rs` | File type mutual exclusivity, set/get mode roundtrip, type overwrite, random type+mode |
| `path_test` | 14 | `fs/path.rs` | Path normalization, dot/dotdot handling, root escape prevention, component splitting, parent/file_name |
| `inode_test` | 15 | `fs/inode.rs` | InodeMode file type classifiers (7 types), permission bits, S_IFMT isolation, inode_hash FNV-1a |
| `file_test` | 11 | `fs/file.rs` | FileFlags access-mode classification (RDONLY/WRONLY/RDWR), O_ACCMODE mask, add_flags/set_bits |
| `dev_t_test` | 10 | `fs/dev_t.rs` | DevNo major/minor packing: to_u64/from_u64 roundtrip, standard device constants (DEV_NULL, DEV_ZERO, etc.) |
| `elf_test` | 12 | `fs/elf.rs` | ElfType/ElfPtType discriminants, PF_R/PF_W/PF_X flag combinations, Elf64Phdr is_load/is_readable/is_writable/is_executable |
| `permission_test` | 12 | `fs/permission.rs` | DAC permission check: owner/group/other priority, mode bit extraction, CAP_DAC_OVERRIDE |
| `dentry_test` | 12 | `fs/dentry.rs` | DentryFlags hashed/unhashed, dentry_hash FNV-1a, DentryState variants |
| `ext4/indirect_test` | 10 | `fs/ext4/indirect.rs` | Direct/indirect block mapping, block iterator count, max_file_size, indirect level monotonicity |
| `ext4/allocator_test` | 12 | `fs/ext4/allocator.rs` | Bitmap scanner: start offset, max_bits, single free bit, all-ones/all-zeros, byte boundary |
| `ext4/namei_test` | 14 | `fs/ext4/namei.rs` | find_entry_space, add_entry_to_block, create_initial_entry, dot/dotdot entries, find_prev_entry, entry alignment |
| `ext4/superblock_test` | 8 | `fs/ext4/superblock.rs` | Ext4FsState feature flags: has_64bit (0x80), has_extents (0x40), has_flex_bg (0x200), independence, powers-of-2 |
| `jbd2/types_test` | 16 | `fs/jbd2/types.rs` | Journal header magic/block_type/sequence roundtrip, tag size calculation, feature flag power-of-2, tags_per_block |
| `jbd2/wrap_test` | 17 | `fs/jbd2/recovery.rs`, `fs/jbd2/commit.rs`, `fs/jbd2/checkpoint.rs` | wrap_block circular increment in [first,last), wrap_journal_block matches, log_space_left clamps to 0, freed-space wrap-around arithmetic, journal_tag_size 4 combinations (8/12/16), tags_per_block minimum and monotonicity, ceil_div formula, desc_blocks formula |
| `superblock_test` | 7 | `fs/superblock.rs` | SuperBlockFlags 9 flags powers-of-2 and distinct, SB_RDONLY=bit0, is_readonly/is_active for all flag combos, bits roundtrip |
| `mount_test` | 8 | `fs/mount.rs` | MntFlags 12 flags powers-of-2, distinct, sequential bits (0-11), is_readonly/is_noexec/is_nosuid for all combos, bits roundtrip |
| `file_flags_test` | 9 | `fs/file.rs` | O_ACCMODE=0b11, access modes (RDONLY/WRONLY/RDWR) extraction, non-access flags distinct and above ACCMODE, O_EXCL=O_CREAT<<1, O_CLOEXEC value, O_SYNC>O_DSYNC, bits roundtrip |
| `readahead_test` | 10 | `fs/readahead.rs` | ReadAheadState: initial state, zero-length no-op, non-sequential reset, activation threshold (2), RA count = MAX_READAHEAD_BLOCKS (4), last_read_end updates, ra_until monotonicity, block_size variation, sequential count increments |
| `pipe_test` | 10 | `fs/pipe.rs` | PipeBuffer circular buffer: empty/full detection, capacity invariant (read+write+1=size), write-read roundtrip, FIFO ordering, partial reads, available_read/write consistency, wraparound write verification |
| `umask_test` | 11 | `fs/fs_struct.rs` | apply_umask clears masked bits, preserves unmasked, idempotent, zero-mode/zero-umask identity, default 0o022 (0o777→0o755, 0o666→0o644), set_umask masks to 9 bits, high bits preserved |
| `ext4/extent_test` | 14 | `fs/ext4/extent.rs` | Ext4ExtentHeader/Ext4Extent/Ext4ExtentIdx struct sizes (12 bytes each), start_block/leaf_block hi/lo roundtrip, EXT4_EXT_MAGIC=0xF30A, interval containment (exact/mid/end-before/before), multi-extent search, entries<=max |
| `io_completion_test` | 14 | `fs/io_completion.rs` | IoCompletion state machine: initial not-done, complete sets status, idempotent overwrite, reset restores, try_wait returns Some/None, wait_for_all aggregation, first-error return, cycle reset |
| `page_offset_test` | 13 | `fs/buffer.rs` | Page offset/index arithmetic roundtrip, monotonicity, copy_len bounds, page_align_down/up, is_page_aligned, align diff |
| `bio_test` | 12 | `fs/bio.rs` | BufferState bitmap: set/test/clear, idempotent, bit independence, named flags (Uptodate/Dirty/Locked/Mapped) consistency, BlockCache hash_index range/determinism, power-of-2 hash_size |
| `ext4/dir_test` | 10 | `fs/ext4/dir.rs` | Ext4DirEntry from_bytes parsing, file type classification (REG/DIR/SYMLINK), get_name UTF-8, deleted entry skipping, find_entry search, rec_len iteration, empty block, name_len boundary, block_size truncation |
| `ext4/inode_test` | 13 | `fs/ext4/inode.rs` | Ext4InodeOnDisk mode decoding (is_dir/is_reg/is_symlink), S_IFMT mask, file type mutual exclusion, has_extent flag isolation, from_disk field copy, Ext4Inode mode/flags match, get_block_nr direct (0-11) / indirect (12+) boundary, S_IFMT type bits |

### security/ (Security) — 38 tests (unchanged from Phase 10)

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `capability_test` | 18 | `security/capability.rs` | POSIX capability bitmask: set/has/clear, boolean algebra (AND/OR/XOR/complement), De Morgan, subset, lo/hi halves |
| `lsm_test` | 10 | `security/lsm.rs` | HookId 7 discriminants (0-6), sorted chain insertion by order, MAX_LSM_COUNT boundary, dispatch all-allow, first-deny-wins, no-opinion skip, empty chain |
| `cap_lsm_test` | 10 | `security/cap_lsm.rs` | CapLsm hook dispatch: Capable allows with cap, denies without/null cred, SignalSend no-opinion without CAP_KILL, other hooks always allow, CAP_KILL=bit5, CAP_VALID_MASK=41 bits |

### signal/ (Signal Handling) — 30 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `signal_test` | 16 | `signal.rs` | Signal bitmap add/has/remove, first/first_unmasked, SigAction classification, signal mask ops |
| `sigpending_test` | 14 | `signal.rs` | SigSet bitmap add/has/remove/first/first_unmasked/clear, signal constants, SigFlags round-trip, RT signal range |

### process/ (Process Management) — 39 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `pid_test` | 9 | `process/pid.rs` | PID bitmap allocator: reserved range, uniqueness, free+realloc, exhaustion, double-free safety, nr_allocated |
| `exit_status_test` | 12 | `process/exit.rs` | POSIX wait status: WIFEXITED/WEXITSTATUS, WIFSIGNALED/WTERMSIG, WIFSTOPPED/WSTOPSIG, exit code clamping, signal masking, stopped low byte 0x7F |
| `task_state_test` | 12 | `process/task.rs` | TaskState bitmap: 7 distinct flags powers-of-2, bits roundtrip, contains, is_running (exact 0), is_sleeping (INT|UNINT), is_dead (ZOMBIE|DEAD), combined flags |
| `cred_test` | 8 | `process/task.rs` | Cred init: new_init all-zero IDs + full caps, new_user uid/gid propagation, empty caps except bounding, root vs init difference |

### sched/ (Scheduler) — 70 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `fair_test` | 18 | `sched/fair.rs` | CFS weight/wmult table monotonicity, LoadWeight, calc_delta_fair vruntime arithmetic, sched_slice proportionality, check_preempt |
| `deadline_test` | 16 | `sched/deadline.rs` | DL bandwidth clamped to 100%, consume/replenish runtime, deadline advancement, monotonicity |
| `rt_test` | 16 | `sched/rt.rs` | SchedRtEntity time_slice lifecycle (dec/reset/underflow), bitmap priority scan, set/clear/find_highest_prio |
| `rt_bitmap_test` | 11 | `sched/rt.rs` | RtRunQueue find_highest_prio bitmap: empty/word0/word1 priority, lowest-bit-wins, word0-over-word1, all-set, random consistency |
| `class_test` | 9 | `sched/class.rs` | SchedClassId ordering (Stop<Deadline<Rt<Fair<Idle), ENQUEUE/DEQUEUE/WF flag distinctness, PartialOrd chain |

### arch/riscv64/mm/ (RISC-V MMU) — 37 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `pagetable_test` | 13 | `arch/riscv64/mm/pagetable.rs` | PTE flag bits, user/kernel/ro pages, is_leaf, ppn extraction, Satp fields |
| `memory_layout_test` | 14 | `arch/riscv64/mm/memory_layout.rs` | Sv39 VirtAddr sign extension, VPN extraction at levels 0/1/2, VA_BITS/PTRS_PER_PTE, floor/ceil |
| `asid_test` | 10 | `arch/riscv64/mm/asid.rs` | SATP build/extract round-trip (ASID+PPN), mode field (Sv39=8), bit positions, ASID constants |

### interrupt/ (Interrupt Handling) — 38 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `irq_test` | 12 | `interrupt/irqdesc.rs` | IrqReturn equality/discriminants, IrqData::new initial state, IrqDesc depth/count, IRQF_SHARED |
| `softirq_test` | 6 | `interrupt/softirq.rs` | SoftirqIndex discriminants (0-9), NR_SOFTIRQS=10, distinctness, IANA assignments |
| `preempt_test` | 11 | `interrupt/preempt.rs` | PREEMPT/SOFTIRQ/HARDIRQ/NMI masks non-overlapping, PREEMPT_ACTIVE no overlap, offsets in own masks only, in_task==!in_interrupt, interrupt decomposition, preemptible only at zero, mask coverage (0x041FFFFF), irq/softirq/nmi enter-exit symmetry |
| `domain_test` | 9 | `interrupt/domain.rs` | IRQ identity mapping (hwirq==virq), out-of-range returns UNMAPPED, revmap lookup, unmapped returns None, idempotent mapping, multiple mappings, zero-size domain |

### arch/ (Architecture) — 13 tests

### errno/ (Error Codes) — 8 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `errno_test` | 8 | `errno.rs` | Errno enum/constant match, EWOULDBLOCK==EAGAIN, no duplicates, positive values, as_neg_i32/u64 consistency |

### ipc/ (Inter-Process Communication) — 22 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `ipc_id_test` | 12 | `ipc/util.rs` | IPC ID build/index/seq roundtrip, seq truncation, negative ID (high index), IPC_CREAT/EXCL/NOWAIT distinct powers-of-2, IPC commands distinct, update_mode permission preservation, perm bits extraction, SHM/MSG/MQ flags distinct |
| `sysv_msg_test` | 10 | `ipc/sysv_msg.rs` | find_msg_match: empty queue, msgtyp=0 first-match, exact type, MSG_EXCEPT, negative msgtyp lowest-type, single message, all-match EXCEPT |

### drivers/ (Device Drivers) — 34 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `virtio_offset_test` | 7 | `drivers/virtio/offset.rs` | VirtIO register offsets strictly increasing, queue LO/HI 4-byte spacing, status bits distinct powers-of-2, PCI BAR offset arithmetic, NUM_QUEUES alignment, CONFIG_GENERATION after STATUS |
| `virtio_queue_test` | 6 | `drivers/virtio/queue.rs` | Desc=16 bytes (8+4+2+2), UsedElem=8 bytes, AvailRing=4 bytes, UsedRing=4 bytes, vring size calculation positivity/alignment, page alignment bounds |
| `pci_offset_test` | 8 | `drivers/pci/mod.rs` | PCI config offsets increasing, BAR offsets sequential (4-byte stride), BAR index↔offset, command bits (IO/MEM/BUS_MASTER) distinct powers-of-2, I/O/memory/64-bit BAR detection for all u32 values |
| `netdev_test` | 14 | `drivers/net/space.rs` | IFF flags distinct powers-of-2, up/down sets/clears IFF_UP+RUNNING, up-down-up idempotent, down preserves other flags, ArpHrdType distinct (LOOPBACK=772/ETHER=1/VOID=0xFFFF), IFF flag values, DeviceStats default zero |
| `input/event_test` | 13 | `drivers/input/event.rs` | InputEvent struct 24 bytes, EV type constants distinct/sequential, key_event press/release, rel/abs event constructors, sync_event, BTN_LEFT/RIGHT/MIDDLE distinct, BTN range, ABS_MT constants ordering, new roundtrip, zero timestamp |

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `pt_regs_test` | 13 | `arch/riscv64/pt_regs.rs` | Cause enum from_cause parsing, is_interrupt/is_exception/is_page_fault, CSR constants (SR_SPP/SR_PIE/SR_SIE/SR_SUM/SR_UXL/SR_FS/SR_VS) |

## Detailed Test List

### mm/page_flags_test (6)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new_zero` | New PageFlags has all bits clear |
| 2 | `test_known_flags` | Each flag bit sets correctly |
| 3 | `test_from_raw_exact` | from_raw(raw).raw() == raw roundtrip |
| 4 | `test_set` | Set flag then test returns true |
| 5 | `test_clear` | Clear flag then test returns false |
| 6 | `test_and_set` | test_and_set returns previous value |
| 7 | `test_and_clear` | test_and_clear returns previous value |
| 8 | `test_clear_all` | clear_all zeroes all bits |

### mm/buddy_test (11)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_block_size_power_of_two` | Block size 4096 is power of 2 |
| 2 | `test_size_to_order_clamped` | size_to_order returns valid order |
| 3 | `test_size_to_order_exact` | size_to_order(power-of-2) exact |
| 4 | `test_size_to_order_roundtrip` | pages(order) == 1 << order |
| 5 | `test_size_to_order_monotone` | Larger size gives >= order |
| 6 | `test_size_to_order_one_page` | 1 page = order 0 |
| 7 | `test_size_to_order_small` | 0 pages clamped to order 0 |
| 8 | `test_get_buddy_idx` | Buddy index involution |
| 9 | `test_order0_any_pfn_aligned` | Order-0 buddy at any PFN is aligned |
| 10 | `test_buddy_is_involution` | buddy(buddy(x)) == x |
| 11 | `test_buddy_pair_alignment` | Buddy pairs are contiguous |
| 12 | `test_alignment_roundtrip` | addr_to_page_idx roundtrip |
| 13 | `test_order_to_pages` | 1 << order == pages |
| 14 | `test_unaligned_fails` | Unaligned addr returns None |

### mm/vma_test (9)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_adjacent_vmas_no_overlap` | Adjacent VMAs do not overlap |
| 2 | `test_no_overlap_after_adds` | Multiple non-overlapping VMAs |
| 3 | `test_overlap_rejected` | Overlapping VMA rejected |
| 4 | `test_contains` | contains returns true for exact match |
| 5 | `test_overlaps` | overlaps detects partial overlap |
| 6 | `test_find_contains` | find returns contained VMA |
| 7 | `test_remove` | remove deletes existing VMA |
| 8 | `test_split` | split divides at boundary |
| 9 | `test_can_merge` | can_merge for adjacent compatible VMAs |

### mm/refcount_test (6)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_refcount_never_negative` | Refcount never goes below 0 |
| 2 | `test_refcount_symmetry` | get increments, put decrements |
| 3 | `test_put_zero_underflow` | put at 0 is no-op |
| 4 | `test_try_get_positive_succeeds` | try_get succeeds when > 0 |
| 5 | `test_try_get_zero_fails` | try_get fails at 0 |
| 6 | `test_get_put_sequence` | Alternating get/put maintains non-negative |

### mm/list_test (8)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_add_is_lifo` | add inserts after head (LIFO) |
| 2 | `test_add_tail_is_fifo` | add_tail inserts at tail (FIFO) |
| 3 | `test_del_positions` | del removes from any position |
| 4 | `test_del_preserves_integrity` | List remains circular after del |
| 5 | `test_add_del_returns_empty` | add/del roundtrip returns empty |
| 6 | `test_forward_backward_symmetry` | Forward/backward traversal symmetric |
| 7 | `test_for_each_visits_all` | for_each visits all nodes |
| 8 | `test_interleaved_add_del` | Interleaved add/del safe |

### mm/buddy_alloc_test (11)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_addr_roundtrip` | page_idx_to_addr(addr_to_page_idx(p)) == p |
| 2 | `test_alloc_free_roundtrip` | alloc then free returns same page |
| 3 | `test_total_pages_conserved` | Total pages invariant across alloc/free |
| 4 | `test_heap_size_to_order` | heap_size_to_order returns valid order |
| 5 | `test_size_to_order_exact` | Power-of-2 sizes return exact order |
| 6 | `test_size_to_order_monotone` | Larger sizes return >= order |
| 7 | `test_size_to_order_one_page` | 1 page = order 0 |
| 8 | `test_size_to_order_small` | Sub-page clamped to order 0 |
| 9 | `test_buddy_involution` | buddy(buddy(x)) == x |
| 10 | `test_buddy_differs_at_order_bit` | Buddy differs in order bit |
| 11 | `test_buddy_merging` | Freeing buddies merges upward |

### mm/zone_test (13)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_int_sqrt_bounds` | n² >= int_sqrt(n)² for all n |
| 2 | `test_int_sqrt_perfect` | int_sqrt(k²) == k |
| 3 | `test_int_sqrt_monotone` | int_sqrt(n) <= int_sqrt(n+1) |
| 4 | `test_int_sqrt_edge` | int_sqrt at 0, 1, MAX |
| 5 | `test_pfn_phys_roundtrip` | phys_to_pfn(pfn_to_phys(p)) == p |
| 6 | `test_pfn_zero` | pfn_to_phys(0) == 0 |
| 7 | `test_gfp_kernel` | GFP_KERNEL maps to ZoneNormal |
| 8 | `test_gfp_dma` | GFP_DMA maps to ZoneDma |
| 9 | `test_gfp_dma32` | GFP_DMA32 maps to ZoneDma32 |
| 10 | `test_gfp_movable` | GFP_HIGHUSER_MOVABLE maps to ZoneMovable |
| 11 | `test_gfp_dma_priority` | GFP_DMA zone type <= GFP_KERNEL zone type |
| 12 | `test_watermark_order` | Watermark formula correctness |
| 13 | `test_watermark_order0` | Order 0 watermark is min_pages |

### mm/vmscan_test (14)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_scan_empty` | nr_to_scan returns 0 for size=0 |
| 2 | `test_scan_def_priority` | nr_to_scan at priority 12 returns size/1024 |
| 3 | `test_scan_priority_1` | nr_to_scan at priority 1 returns size |
| 4 | `test_scan_priority_2` | nr_to_scan at priority 2 returns size |
| 5 | `test_scan_priority_3_small` | nr_to_scan at priority 3 for small sizes |
| 6 | `test_scan_priority_4_small` | nr_to_scan at priority 4 for small sizes |
| 7 | `test_scan_monotone_priority` | Lower priority scans more |
| 8 | `test_scan_monotone_size` | Larger size scans more |
| 9 | `test_scan_bounded` | nr_to_scan never exceeds size |
| 10 | `test_reclaim_power_of_2` | nr_to_reclaim is power of 2 |
| 11 | `test_order0_reclaim` | Order 0 reclaims exactly 1 page |
| 12 | `test_loop_terminates` | Priority loop terminates within bounds |
| 13 | `test_loop_reclaims_enough` | Sufficient per-iteration reclaim stops early |
| 14 | `test_lru_indices` | LRU indices in valid range |

### mm/compact_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_start_ge_end` | start >= end terminates immediately |
| 2 | `test_start_lt_end` | start < end does not terminate |
| 3 | `test_scanners_meet` | Migrate and free scanners converge |
| 4 | `test_max_scan_limit` | MAX_SCAN_PAGES (4096) limits scanning |
| 5 | `test_migrated_complete` | Migrations yield Complete result |
| 6 | `test_no_migrate_skipped` | No migrations yields Skipped result |
| 7 | `test_scanned_count` | nr_scanned increments correctly |
| 8 | `test_free_scanner_floor` | Free scanner respects min_pfn |
| 9 | `test_migrate_goes_up` | Migrate scanner only advances upward |
| 10 | `test_free_goes_down` | Free scanner only advances downward |
| 11 | `test_filter_free` | Free pages not migratable |
| 12 | `test_filter_reserved` | Reserved pages not migratable |
| 13 | `test_filter_non_anon` | Non-anonymous pages not migratable |
| 14 | `test_filter_dirty` | Dirty pages not migratable |
| 15 | `test_filter_refcount` | refcount != 1 not migratable |
| 16 | `test_filter_ideal` | Ideal page (anon, mapped, rc=1, clean) migratable |

### mm/rmap_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_vpn_zero` | addr_to_vpn(0) == 0 |
| 2 | `test_vpn_monotone` | addr_to_vpn is monotonically non-decreasing |
| 3 | `test_vpn_bounds` | vpn * PAGE_SIZE <= addr < (vpn+1) * PAGE_SIZE |
| 4 | `test_vpn_roundtrip` | vpn * PAGE_SIZE <= addr |
| 5 | `test_sv39_vpn_range` | VPN indices always in [0, 511] |
| 6 | `test_sv39_vpn_roundtrip` | VPN → addr reconstruction roundtrip |
| 7 | `test_sv39_zero` | sv39_vpn_indices(0) == (0,0,0) |
| 8 | `test_mapped_negative` | Negative mapcount → not mapped |
| 9 | `test_mapped_nonnegative` | Non-negative mapcount → mapped |
| 10 | `test_add_lru` | Add to LRU when old_mapcount < 0 |
| 11 | `test_no_add_lru` | No LRU add when old_mapcount >= 0 |
| 12 | `test_remove_lru` | Remove from LRU when old_mapcount == 0 |
| 13 | `test_vpn_page_size` | addr_to_vpn(PAGE_SIZE) == 1 |
| 14 | `test_vpn_last_byte` | addr_to_vpn(PAGE_SIZE - 1) == 0 |
| 15 | `test_sv39_page_aligned` | Reconstructed addr is page-aligned |
| 16 | `test_sv39_offset` | Page offset preserved in reconstruction |

### mm/page_flags_ops_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new_empty` | New flags empty, all tests return false |
| 2 | `test_set_then_test` | Set then test returns true |
| 3 | `test_set_clear_roundtrip` | Set then clear then test returns false |
| 4 | `test_test_and_set_first` | test_and_set returns false on first call |
| 5 | `test_test_and_clear` | test_and_clear returns true if was set |
| 6 | `test_set_idempotent` | Setting same flag twice is idempotent |
| 7 | `test_clear_all` | clear_all removes all flags |
| 8 | `test_from_raw_roundtrip` | from_raw + raw roundtrip |
| 9 | `test_flags_distinct_pow2` | All 16 PageFlag variants are distinct powers of 2 |
| 10 | `test_set_multiple` | Setting multiple flags preserves all |
| 11 | `test_clear_isolated` | Clearing one flag does not affect others |
| 12 | `test_test_and_set_twice` | test_and_set returns true on second call |
| 13 | `test_test_and_clear_not_set` | test_and_clear returns false when not set |
| 14 | `test_page_type_range` | PageType discriminants are 0..=4 |
| 15 | `test_flags_fit_u16` | All flags fit in u16 |
| 16 | `test_random_ops` | Random raw value test/clear/set consistency |

### sync/spinlock_test (4)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_try_lock_unlock` | try_lock/unlock basic operation |
| 2 | `test_try_lock_fails_when_locked` | try_lock returns false when held |
| 3 | `test_lock_unlock` | lock/unlock acquires and releases |
| 4 | `test_unlock_unlocked` | Double unlock is safe no-op |

### sync/seqlock_test (8)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_initial_state` | Sequence starts at 0, not locked |
| 2 | `test_write_mutates` | Write lock increments sequence |
| 3 | `test_locked_state` | is_locked reflects write lock state |
| 4 | `test_try_write_fails_when_locked` | try_write fails when already locked |
| 5 | `test_try_write_succeeds_when_unlocked` | try_write succeeds when unlocked |
| 6 | `test_sequence_increments` | Each write increments sequence |
| 7 | `test_read_consistency` | Read sees complete or consistent state |
| 8 | `test_struct_atomicity` | Multi-field struct read is atomic |

### sync/futex_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_private_key_match` | Private key matches on uaddr + pid |
| 2 | `test_private_key_pid_mismatch` | Private key rejects different pid |
| 3 | `test_private_key_uaddr_mismatch` | Private key rejects different uaddr |
| 4 | `test_shared_key_match` | Shared key matches on uaddr only |
| 5 | `test_hash_in_range` | Hash always in [0, HASH_SIZE) |
| 6 | `test_hash_different_pids` | Different pids produce different hashes |
| 7 | `test_to_flags_default_shared` | No PRIVATE flag → SHARED |
| 8 | `test_to_flags_private` | PRIVATE flag set → not SHARED |
| 9 | `test_to_flags_clockrt` | CLOCK_REALTIME → FLAGS_CLOCKRT |
| 10 | `test_cmd_mask` | CMD_MASK strips PRIVATE and CLOCK_REALTIME |
| 11 | `test_bitset_match_any` | MATCH_ANY always matches |
| 12 | `test_bitset_zero` | Zero bitset never matches |
| 13 | `test_bitset_commutative` | Bitset intersection is commutative |
| 14 | `test_opcodes_distinct` | All FUTEX_* opcodes are distinct |
| 15 | `test_key_reflexive` | Key matches itself |
| 16 | `test_key_symmetric` | matches() is symmetric |

### net/route_test (9)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_lookup_longest_prefix` | Longest-prefix match selects correct route |
| 2 | `test_matches_host_route` | Host route (mask = 0xFFFFFFFF) exact match |
| 3 | `test_matches_default_route` | Default route (dst=0, mask=0) matches any |
| 4 | `test_matches_masking` | dst is masked before comparison |
| 5 | `test_add_lookup_count` | Add increments count, lookup finds it |
| 6 | `test_empty_lookup` | Empty table returns None |
| 7 | `test_interleaved_add_remove` | Add/remove interleaving safe |
| 8 | `test_remove_all` | Remove all entries empties table |
| 9 | `test_remove_correct_route` | Remove deletes correct (dst, mask) entry |
| 10 | `test_remove_nonexistent` | Removing non-existent returns false |
| 11 | `test_flags` | RouteFlags bitfield correct |

### net/arp_test (12)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_cache_capacity` | LRU cache bounded to max entries |
| 2 | `test_empty_lookup` | Empty cache returns None |
| 3 | `test_update_existing` | Update refreshes existing entry |
| 4 | `test_update_lookup` | Updated entry findable |
| 5 | `test_remove` | Remove deletes from cache |
| 6 | `test_remove_nonexistent` | Removing non-existent is safe |
| 7 | `test_interleaved_update_remove` | Interleaved update/remove safe |
| 8 | `test_from_bytes_short` | Short packet returns None |
| 9 | `test_packet_size` | ARP packet is 28 bytes |
| 10 | `test_packet_op_detection` | is_request/is_reply detect opcodes |
| 11 | `test_packet_mac_extraction` | MAC addresses extracted correctly |
| 12 | `test_packet_ip_extraction` | IP addresses extracted correctly |
| 13 | `test_clear` | Clear empties cache |

### net/checksum_test (10)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_zero_length` | checksum([]) == 0xFFFF |
| 2 | `test_single_byte` | Single byte checksum formula |
| 3 | `test_all_zeros` | All-zeros checksum is 0xFFFF |
| 4 | `test_complement_identity` | checksum(word) == !word |
| 5 | `test_verify_property` | Appending checksum yields verify == 0 |
| 6 | `test_rfc1071_vector` | RFC 1071 test vector self-consistency |
| 7 | `test_even_length` | Two-byte big-endian formula |
| 8 | `test_pseudo_header` | Pseudo-header checksum construction |
| 9 | `test_carry_fold` | Carry folding correctness |
| 10 | `test_pseudo_header_proto_differs` | TCP vs UDP pseudo-header differ |

### net/tcp_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_rtt_first_measurement` | First RTT: srtt == rtt, rttvar == rtt/2 |
| 2 | `test_rto_clamped` | RTO clamped to [RTO_MIN, RTO_MAX] |
| 3 | `test_rto_backoff` | backoff() doubles RTO, capped |
| 4 | `test_rto_reset` | reset() restores defaults |
| 5 | `test_rto_bounds_sequence` | RTO stays in bounds across sequences |
| 6 | `test_cong_init` | CFS congestion initial state |
| 7 | `test_cong_slow_start` | Slow start: cwnd += MSS per ACK |
| 8 | `test_cong_slow_start_monotone` | cwnd monotonically increases |
| 9 | `test_cong_timeout` | Timeout resets cwnd to MSS |
| 10 | `test_cong_reset` | Reset restores initial state |
| 11 | `test_seq_irreflexive` | seq_before(a, a) == false |
| 12 | `test_seq_antisymmetric` | seq_before(a,b) && seq_before(b,a) == false |
| 13 | `test_flag_roundtrips` | TCP header flag set/get roundtrips |
| 14 | `test_dof_roundtrip` | data offset roundtrip |
| 15 | `test_window_roundtrip` | window field roundtrip |
| 16 | `test_fast_retransmit` | 3 dup ACKs trigger fast retransmit |

### net/ethernet_test (7)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_broadcast_classification` | Broadcast is multicast, never unicast |
| 2 | `test_zero_addr` | All-zeros is no category |
| 3 | `test_multicast_bit` | Multicast iff bit 0 of byte 0 set |
| 4 | `test_unicast_exclusive` | Valid unicast excludes multicast/broadcast |
| 5 | `test_addr_eq` | addr_eq is reflexive and symmetric |
| 6 | `test_addr_not_eq` | Different addresses not equal |
| 7 | `test_classification_exclusive` | At most one category per address |

### net/ipv4_udp_test (7)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_ip_version` | IPv4 header version is 4 |
| 2 | `test_ip_ihl` | IHL >= 5 |
| 3 | `test_ip_header_len_default` | IHL=5 gives 20-byte header |
| 4 | `test_udp_port_roundtrip` | Big-endian port roundtrip |
| 5 | `test_udp_zero_port` | Zero port roundtrip |
| 6 | `test_udp_default` | Default header all zeros |
| 7 | `test_ip_protocol` | Protocol field preserved |
| 8 | `test_ip_addrs` | Source/dest address preserved |
| 9 | `test_ip_ttl` | TTL field preserved |

### fs/cmdline_test (14)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_get_param_basic` | get_param extracts value after '=' |
| 2 | `test_get_param_missing` | get_param returns None for missing key |
| 3 | `test_get_param_multiple` | get_param finds key among multiple params |
| 4 | `test_has_param_present` | has_param detects boolean flags |
| 5 | `test_has_param_absent` | has_param returns false for absent flag |
| 6 | `test_get_all_params` | get_all_params returns all key=value pairs |
| 7 | `test_get_all_params_empty` | Empty cmdline returns no params |
| 8 | `test_get_all_params_skips_flags` | Flags without '=' are skipped |
| 9 | `test_get_root_device_default` | Default root device |
| 10 | `test_get_root_device_present` | Root device extraction |
| 11 | `test_get_init_program_default` | Default init program |
| 12 | `test_debug_mode` | Debug flag detection |
| 13 | `test_real_bootargs` | Real bootargs parsing |
| 14 | `test_root_readonly_default` | Root readonly default |
| 15 | `test_root_readonly_flag` | Root readonly flag |

### fs/stat_test (11)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new` | Default mode is 0 |
| 2 | `test_set_regular_file` | set_regular_file makes is_regular_file true |
| 3 | `test_set_directory` | set_directory makes is_directory true |
| 4 | `test_set_char_device` | set_char_device makes is_char_device true |
| 5 | `test_set_block_device` | set_block_device makes is_block_device true |
| 6 | `test_set_fifo` | set_fifo makes is_fifo true |
| 7 | `test_set_symlink` | set_symlink makes is_symlink true |
| 8 | `test_set_socket` | set_socket makes is_socket true |
| 9 | `test_mutual_exclusivity` | File types mutually exclusive |
| 10 | `test_mode_roundtrip` | set_mode/get_mode roundtrip |
| 11 | `test_get_mode_low_bits` | get_mode returns only low 9 bits |
| 12 | `test_set_mode_preserves_type` | set_mode preserves file type |
| 13 | `test_set_type_preserves_mode` | set_type preserves permissions |
| 14 | `test_type_overwrite` | File type overwritten by new set |
| 15 | `test_file_type_codes_distinct` | All 7 type codes distinct |
| 16 | `test_random_type_mode` | Random type+mode combinations valid |

### fs/path_test (14)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_absolute_stays_absolute` | Absolute paths stay absolute |
| 2 | `test_no_dot_absolute` | No "." in normalized output |
| 3 | `test_no_double_slash` | No consecutive "//" |
| 4 | `test_root_escape` | "/.." normalizes to "/" |
| 5 | `test_is_absolute` | is_absolute correct |
| 6 | `test_components` | Component splitting correct |
| 7 | `test_parent_filename` | parent/file_name consistency |
| 8 | `test_dotdot_cancel` | "a/b/.." normalizes to "a" |
| 9 | `test_relative_dotdot` | Relative ".." accumulates |
| 10 | `test_path_component` | PathComponent classification |
| 11 | `test_normalize_empty` | Empty normalizes to empty |
| 12 | `test_normalize_root` | Root normalizes to "/" |
| 13 | `test_parent_of_root` | parent of "/" returns "/" |
| 14 | `test_normalize_complex` | Complex path normalization |

### fs/inode_test (15)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_is_regular_file` | INV-INODE-1: S_IFREG sets is_regular_file |
| 2 | `test_is_directory` | INV-INODE-2: S_IFDIR sets is_directory |
| 3 | `test_is_char_device` | INV-INODE-3: S_IFCHR sets is_char_device |
| 4 | `test_is_block_device` | INV-INODE-4: S_IFBLK sets is_block_device |
| 5 | `test_is_fifo` | INV-INODE-5: S_IFIFO sets is_fifo |
| 6 | `test_is_symlink` | INV-INODE-6: S_IFLNK sets is_symlink |
| 7 | `test_is_socket` | INV-INODE-7: S_IFSOCK sets is_socket |
| 8 | `test_types_mutually_exclusive` | INV-INODE-8: At most one type true per mode |
| 9 | `test_bits_roundtrip` | INV-INODE-9: bits() roundtrip |
| 10 | `test_ifmt_isolates` | INV-INODE-10: S_IFMT isolates file type bits |
| 11 | `test_type_constants_distinct` | INV-INODE-11: All 7 S_IF* constants distinct |
| 12 | `test_inode_hash_deterministic` | INV-INODE-12: Same input → same hash |
| 13 | `test_inode_hash_different_inputs` | INV-INODE-13: Different inos → different hashes |
| 14 | `test_inode_hash_ino_dominates` | INV-INODE-14: Different fs_id → different hash |
| 15 | `test_ifmt_no_overlap` | INV-INODE-15: Permission bits don't overlap S_IFMT |

### fs/file_test (11)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_readonly_basic` | INV-FILE-1: O_RDONLY is readonly |
| 2 | `test_writeonly_basic` | INV-FILE-2: O_WRONLY is writeonly |
| 3 | `test_rdwr_basic` | INV-FILE-3: O_RDWR is rdwr |
| 4 | `test_access_modes_exclusive` | INV-FILE-4: Access modes mutually exclusive |
| 5 | `test_bits_roundtrip` | INV-FILE-5: bits() roundtrip |
| 6 | `test_accmode_mask` | INV-FILE-6: O_ACCMODE is 2-bit mask |
| 7 | `test_extra_flags_dont_change_access` | INV-FILE-7: Non-access flags don't affect access |
| 8 | `test_add_flags` | INV-FILE-8: add_flags is OR |
| 9 | `test_set_bits` | INV-FILE-9: set_bits replaces flags |
| 10 | `test_accmode_no_overlap` | INV-FILE-10: O_ACCMODE doesn't overlap non-access flags |
| 11 | `test_creat_excl` | INV-FILE-11: O_CREAT + O_EXCL is valid |

### fs/dentry_test (12)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_is_hashed` | INV-DENT-1: DCACHE_HASHED flag sets is_hashed |
| 2 | `test_not_hashed` | INV-DENT-2: No DCACHE_HASHED → not hashed |
| 3 | `test_is_unhashed` | INV-DENT-3: DCACHE_UNHASHED flag sets is_unhashed |
| 4 | `test_both_hashed_unhashed` | INV-DENT-4: Can be both hashed and unhashed |
| 5 | `test_bits_roundtrip` | INV-DENT-5: bits() roundtrip |
| 6 | `test_dentry_hash_deterministic` | INV-DENT-6: Same input → same hash |
| 7 | `test_dentry_hash_empty_name` | INV-DENT-7: Empty name depends on parent_ino |
| 8 | `test_dentry_hash_different_names` | INV-DENT-8: Different names → different hashes |
| 9 | `test_dentry_hash_different_parent` | INV-DENT-9: Different parent → different hashes |
| 10 | `test_dentry_state_distinct` | INV-DENT-10: DentryState variants distinct |
| 11 | `test_dcache_flags_distinct` | INV-DENT-11: DCACHE constants distinct powers-of-2 |
| 12 | `test_dentry_hash_single_char` | INV-DENT-12: Single-char name hash non-zero |

### fs/ext4/indirect_test (10)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_direct_blocks` | Blocks 0–11 → level 0 |
| 2 | `test_single_indirect` | Blocks 12–1035 → level 1 |
| 3 | `test_iteration_count` | next_mapping returns None after total |
| 4 | `test_max_file_size_4k` | max_file_size(4096) > 4TB |
| 5 | `test_indirect_level_direct` | Direct files → level 0 |
| 6 | `test_indirect_level_single` | Single indirect → level 1 |
| 7 | `test_indirect_level_double` | Double indirect → level 2 |
| 8 | `test_boundary_12` | Block 12 boundary: level 0→1 |
| 9 | `test_max_file_size_monotone` | Larger block size → larger max file |
| 10 | `test_indirect_level_monotone` | Larger size → higher level |

### fs/ext4/allocator_test (12)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_all_zeros_find_first` | All-zeros bitmap finds bit 0 |
| 2 | `test_all_ones_find_none` | All-ones bitmap returns None |
| 3 | `test_start_offset` | Respects start offset |
| 4 | `test_max_bits` | Respects max_bits |
| 5 | `test_single_free_bit` | Single free bit found at correct position |
| 6 | `test_empty_bitmap` | Empty bitmap returns None |
| 7 | `test_zero_max_bits` | max_bits=0 returns None |
| 8 | `test_start_beyond_max` | start > max returns None |
| 9 | `test_free_after_occupied_start` | Free bit after occupied prefix |
| 10 | `test_last_bit_free` | Last bit found correctly |
| 11 | `test_alternating_pattern` | Alternating 0xAA/0x55 pattern |
| 12 | `test_byte_boundary_start` | Byte-aligned start offset |

### fs/ext4/namei_test (14)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_find_space_empty_block` | INV-NAMEI-1: Empty block returns None |
| 2 | `test_find_space_single_entry` | INV-NAMEI-2: Single large entry finds space |
| 3 | `test_find_space_no_room` | INV-NAMEI-3: Full block returns None |
| 4 | `test_dot_entry` | INV-NAMEI-4: Dot entry correct structure |
| 5 | `test_dotdot_entry` | INV-NAMEI-5: Dotdot entry correct structure |
| 6 | `test_dot_dotdot_different` | INV-NAMEI-6: Dot and dotdot have different name_len |
| 7 | `test_create_initial_entry` | INV-NAMEI-7: Initial entry writes correct fields |
| 8 | `test_add_entry_split` | INV-NAMEI-8: add_entry splits existing entry |
| 9 | `test_find_prev_entry` | INV-NAMEI-9: find_prev returns previous offset |
| 10 | `test_find_prev_entry_first` | INV-NAMEI-10: First entry has no previous |
| 11 | `test_entry_alignment` | INV-NAMEI-11: Entry length 4-byte aligned |
| 12 | `test_find_prev_nonexistent` | INV-NAMEI-12: Nonexistent target returns target |
| 13 | `test_rec_len_sum` | INV-NAMEI-13: rec_len sum equals block_size |
| 14 | `test_dot_entries_size` | INV-NAMEI-14: Dot entries exactly 8 bytes |

### fs/jbd2/types_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_magic_valid` | Valid header passes is_valid |
| 2 | `test_magic_invalid` | Corrupted magic fails is_valid |
| 3 | `test_block_type_roundtrip` | block_type roundtrip |
| 4 | `test_sequence_roundtrip` | sequence roundtrip |
| 5 | `test_block_types_distinct` | All block type constants distinct |
| 6 | `test_tag_size_v3_larger` | v3 tag size >= v2 >= v1 |
| 7 | `test_tags_per_block_minimum` | tags_per_block >= 1 |
| 8 | `test_tags_per_block_4k` | > 100 tags per 4K block |
| 9 | `test_incompat_flags_pow2` | INCOMPAT flags are power-of-2 |
| 10 | `test_tag_flags_pow2` | Tag flags are power-of-2 |
| 11 | `test_header_size` | Header struct is 12 bytes |
| 12 | `test_tail_size` | Tail struct is 4 bytes |
| 13 | `test_checksum_types_distinct` | Checksum types distinct and non-zero |
| 14 | `test_tags_per_block_monotone` | Larger block → more tags |
| 15 | `test_tags_per_block_tag_size` | Larger tag → fewer tags |
| 16 | `test_default_header_invalid` | Default header has invalid magic |

### security/capability_test (18)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new_masks` | new(mask) masks to valid 41 bits |
| 2 | `test_set_has` | set(x); has(x) for valid caps |
| 3 | `test_set_clear_roundtrip` | set then clear |
| 4 | `test_intersect` | intersect = AND |
| 5 | `test_union` | union = OR |
| 6 | `test_xor` | xor = XOR |
| 7 | `test_complement_empty_full` | complement(EMPTY) == FULL |
| 8 | `test_complement_involution` | complement(complement(c)) == c |
| 9 | `test_de_morgan` | De Morgan's law |
| 10 | `test_is_empty` | is_empty correct |
| 11 | `test_subset_reflexive` | c.is_subset_of(c) |
| 12 | `test_subset_trivial` | EMPTY subset of any |
| 13 | `test_has_invalid_cap` | cap > 40 always false |
| 14 | `test_from_halves_new_roundtrip` | lo/hi/from_halves roundtrip |
| 15 | `test_halves_roundtrip` | lo/hi decomposition |
| 16 | `test_empty_full` | EMPTY/FULL sentinel values |
| 17 | `test_all_caps_distinct` | All 41 CAP_* constants distinct |
| 18 | `test_all_caps_in_range` | All caps in [0, 40] |

### signal/signal_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_add_has` | add(sig); has(sig) |
| 2 | `test_add_idempotent` | Adding same signal twice is idempotent |
| 3 | `test_add_remove_roundtrip` | add then remove |
| 4 | `test_clear` | clear removes all signals |
| 5 | `test_first` | first() returns lowest set bit |
| 6 | `test_first_empty` | first() returns None when empty |
| 7 | `test_first_unmasked_no_mask` | first_unmasked with 0 mask |
| 8 | `test_first_unmasked_all_masked` | first_unmasked with all-ones mask |
| 9 | `test_get_all` | get_all bitmap matches signals |
| 10 | `test_has_out_of_range` | has(0), has(65) return false |
| 11 | `test_random_sequence` | Random add/remove/first_unmasked |
| 12 | `test_sigaction_default` | Default action is Default |
| 13 | `test_sigaction_handler` | Handler action has handler |
| 14 | `test_sigaction_ignore` | Ignore action is Ignore |
| 15 | `test_signal_mask` | add_mask/remove_mask on bitmap |
| 16 | `test_signal_mask_out_of_range` | Out-of-range mask ops are no-op |

### process/pid_test (9)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_pid_reserved` | Allocated PIDs >= RESERVED_PIDS (16) |
| 2 | `test_pid_unique` | Allocated PIDs are unique |
| 3 | `test_pid_free` | Free makes PID available again |
| 4 | `test_pid_free_reserved` | Free reserved PID is no-op |
| 5 | `test_pid_free_oorange` | Free out-of-range PID is no-op |
| 6 | `test_pid_count` | nr_allocated matches actual count |
| 7 | `test_scan_range` | scan_range finds first zero in range |
| 8 | `test_pid_exhaustion` | Exhaustion returns None |
| 9 | `test_pid_double_free` | Double-free is safe |

### sched/fair_test (18)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_weight_monotone` | PRIO_TO_WEIGHT is monotonically decreasing |
| 2 | `test_wmult_monotone` | PRIO_TO_WMULT is monotonically increasing |
| 3 | `test_nice_0_weight` | Nice 0 weight is 1024 |
| 4 | `test_from_nice_valid_index` | from_nice maps to valid table entry |
| 5 | `test_nice_weight_inverse` | Lower nice → higher weight |
| 6 | `test_delta_fair_nice_0` | calc_delta_fair for nice-0 returns delta unchanged |
| 7 | `test_delta_fair_weight_relation` | Higher weight → smaller vruntime delta |
| 8 | `test_delta_fair_linear` | calc_delta_fair linear in delta |
| 9 | `test_update_inv_weight_idempotent` | update_inv_weight is idempotent |
| 10 | `test_inv_weight_nice_0` | inv_weight for nice 0 is 4194304 |
| 11 | `test_sched_slice_empty` | 0 tasks returns min granularity |
| 12 | `test_sched_slice_zero_total_weight` | 0 total weight returns min granularity |
| 13 | `test_sched_slice_minimum` | sched_slice >= min granularity always |
| 14 | `test_sched_slice_proportional` | Double weight → >= slice |
| 15 | `test_check_preempt_no_preempt` | se >= curr → no preempt |
| 16 | `test_check_preempt_threshold` | Small gap → no preempt, large gap → preempt |
| 17 | `test_ms_ns_roundtrip` | Millisecond/nanosecond conversion |
| 18 | `test_weight_inv_product` | weight * inv_weight > 0 |

### sched/deadline_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_default_not_throttled` | Default entity not throttled |
| 2 | `test_bw_zero_period` | Bandwidth 0 when period is 0 |
| 3 | `test_bw_100_percent` | runtime == period → 100% bandwidth |
| 4 | `test_bw_capped` | Bandwidth <= DL_BW_MAX when runtime <= period |
| 5 | `test_bw_zero_runtime` | Bandwidth 0 when runtime is 0 |
| 6 | `test_consume_reduces` | consume_runtime reduces remaining |
| 7 | `test_consume_throttle` | Consuming beyond runtime throttles |
| 8 | `test_replenish` | Replenish restores runtime, clears throttle |
| 9 | `test_update_deadline` | deadline = now + period |
| 10 | `test_deadline_monotone` | Deadlines advance monotonically |
| 11 | `test_bw_monotone_runtime` | Bandwidth monotone in runtime |
| 12 | `test_bw_antitone_period` | Bandwidth antitone in period |
| 13 | `test_consume_zero` | Consume zero is no-op |
| 14 | `test_default_values` | Default period and runtime match constants |
| 15 | `test_runtime_nonnegative` | Runtime never goes negative |
| 16 | `test_repeated_consume` | Repeated consume when throttled doesn't panic |

### sched/rt_test (16)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new_entity` | INV-RT-1: Default time_slice and not on_rq |
| 2 | `test_dec_time_slice` | INV-RT-2: Decrements by 1 |
| 3 | `test_dec_at_zero` | INV-RT-3: At 0, returns 0, stays 0 |
| 4 | `test_reset_time_slice` | INV-RT-4: Reset restores default |
| 5 | `test_on_rq_roundtrip` | INV-RT-5: set_on_rq/is_on_rq roundtrip |
| 6 | `test_exhaust_timeslice` | INV-RT-6: 100 decs from default reaches 0 |
| 7 | `test_set_time_slice` | INV-RT-7: set_time_slice sets exact value |
| 8 | `test_find_highest_prio_word0` | INV-RT-8: Lowest set bit in word0 |
| 9 | `test_find_highest_prio_word1` | INV-RT-9: word1 adds 64 offset |
| 10 | `test_find_highest_prio_lowest_wins` | INV-RT-10: Lowest priority number wins |
| 11 | `test_find_highest_prio_empty` | INV-RT-11: Empty bitmap returns None |
| 12 | `test_bitmap_set_clear_find` | INV-RT-12: Set/clear/find roundtrip |
| 13 | `test_prio_to_bitmap` | INV-RT-13: prio_to_bitmap word/bit mapping |
| 14 | `test_max_rt_prio` | INV-RT-14: MAX_RT_PRIO is 100 |
| 15 | `test_interleaved_bitmap_ops` | INV-RT-15: Interleaved set/clear correctness |
| 16 | `test_no_underflow` | INV-RT-16: Repeated dec never underflows |

### arch/riscv64/mm/pagetable_test (13)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_default` | Default PTE is zero |
| 2 | `test_new_packing` | new_table packs R/W/X bits correctly |
| 3 | `test_new_page_kernel` | Kernel page: valid, readable, writable, not user |
| 4 | `test_new_page_user` | User page: valid, readable, writable, user |
| 5 | `test_new_page_ro` | Read-only page: valid, readable, not writable |
| 6 | `test_from_bits` | from_bits(bits).bits() roundtrip |
| 7 | `test_is_valid` | Valid flag detection |
| 8 | `test_is_readable` | Readable flag detection |
| 9 | `test_is_writable` | Writable flag detection |
| 10 | `test_is_executable` | Executable flag detection |
| 11 | `test_is_user` | User flag detection |
| 12 | `test_is_leaf` | Leaf (R/W/X) detection |
| 13 | `test_ppn_extraction` | PPN extraction from bits |
| 14 | `test_sv39_mode` | Sv39 mode extraction |
| 15 | `test_bare_mode` | Bare mode (mode=0) |
| 16 | `test_asid_extraction` | ASID extraction from Satp |
| 17 | `test_mode_extraction` | Mode extraction from Satp |

### interrupt/irq_test (12)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_irq_return_equality` | INV-IRQ-1: IrqReturn equality works correctly |
| 2 | `test_irq_data_new` | INV-IRQ-2: irq == hwirq, chip=0, chip_data=0 |
| 3 | `test_irq_data_new_zero` | INV-IRQ-3: IrqData::new(0) creates zeroed data |
| 4 | `test_irq_desc_new` | INV-IRQ-4: depth=0, all counts=0 |
| 5 | `test_inc_count` | INV-IRQ-5: inc_count increments specific CPU |
| 6 | `test_inc_count_isolated` | INV-IRQ-6: inc_count on one CPU doesn't affect others |
| 7 | `test_irqf_shared` | INV-IRQ-7: IRQF_SHARED is bit 0 |
| 8 | `test_irq_return_discriminants` | INV-IRQ-8: Discriminants are 0, 1, 2 |
| 9 | `test_irq_data_deterministic` | INV-IRQ-9: IrqData::new is deterministic |
| 10 | `test_get_count_out_of_range` | INV-IRQ-10: Out-of-range returns 0 |
| 11 | `test_depth_field` | INV-IRQ-11: depth field can be read/modified |
| 12 | `test_irq_data_copy` | INV-IRQ-12: Copy preserves all fields |

### mm/swap_test (10)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_make_swap_entry` | INV-SWAP-1: make_swap_entry encodes type and offset |
| 2 | `test_is_swap_entry` | INV-SWAP-2: is_swap_entry detects signature bit |
| 3 | `test_swap_entry_type` | INV-SWAP-3: swap_entry_type extracts type |
| 4 | `test_swap_entry_offset` | INV-SWAP-4: swap_entry_offset extracts offset |
| 5 | `test_roundtrip` | INV-SWAP-5: encode/decode roundtrip |
| 6 | `test_max_offset` | INV-SWAP-6: Max offset fits without signature collision |
| 7 | `test_non_swap_entry` | INV-SWAP-7: Non-swap PTE not detected |
| 8 | `test_type_zero` | INV-SWAP-8: Type 0 swap entry valid |
| 9 | `test_offset_zero` | INV-SWAP-9: Offset 0 swap entry valid |
| 10 | `test_signature_bit` | INV-SWAP-10: SWAP_ENTRY_SIGNATURE is 1<<62 |

### mm/page_addr_test (15)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new` | INV-PADDR-1: new masks to page boundary |
| 2 | `test_is_aligned` | INV-PADDR-2: is_aligned iff page-aligned |
| 3 | `test_floor_le` | INV-PADDR-3: floor(addr) <= addr |
| 4 | `test_ceil_ge` | INV-PADDR-4: ceil(addr) >= addr |
| 5 | `test_floor_aligned` | INV-PADDR-5: floor of aligned addr is itself |
| 6 | `test_ceil_aligned` | INV-PADDR-6: ceil of aligned addr is itself |
| 7 | `test_frame_number_eq_ppn` | INV-PADDR-7: frame_number == ppn |
| 8 | `test_frame_roundtrip` | INV-PADDR-8: PhysFrame start_address roundtrip |
| 9 | `test_frame_range` | INV-PADDR-9: PhysFrame range is PAGE_SIZE wide |
| 10 | `test_virtaddr_new` | INV-PADDR-10: VirtAddr mirrors PhysAddr invariants |
| 11 | `test_virtpage_roundtrip` | INV-PADDR-11: VirtPage start_address roundtrip |
| 12 | `test_virtpage_range` | INV-PADDR-12: VirtPage range is PAGE_SIZE wide |
| 13 | `test_floor_ceil` | INV-PADDR-13: floor then ceil difference <= PAGE_SIZE |
| 14 | `test_frame_times_page_size` | INV-PADDR-14: frame_number * PAGE_SIZE == start_address |
| 15 | `test_page_mask` | INV-PADDR-15: PAGE_MASK == 0xFFF |

### fs/dev_t_test (10)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new` | INV-DEV-1: DevNo::new stores major/minor correctly |
| 2 | `test_roundtrip` | INV-DEV-2: to_u64/from_u64 roundtrip |
| 3 | `test_zero_dev` | INV-DEV-3: DevNo(0,0) roundtrips |
| 4 | `test_max_values` | INV-DEV-4: Max u32 major/minor roundtrips |
| 5 | `test_dev_null` | INV-DEV-5: DEV_NULL is (1, 3) |
| 6 | `test_dev_zero` | INV-DEV-6: DEV_ZERO is (1, 5) |
| 7 | `test_major_ordering` | INV-DEV-7: Major numbers ordered as expected |
| 8 | `test_minor_independence` | INV-DEV-8: Different minors produce different DevNo |
| 9 | `test_multiple_devices` | INV-DEV-9: Standard devices all have valid encoding |
| 10 | `test_from_u64_zero` | INV-DEV-10: from_u64(0) == DevNo(0,0) |

### fs/elf_test (12)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_magic` | INV-ELF-1: ELF magic is correct |
| 2 | `test_elf_type_distinct` | INV-ELF-2: ElfType discriminants distinct |
| 3 | `test_pt_type_distinct` | INV-ELF-3: ElfPtType discriminants distinct |
| 4 | `test_perm_bits` | INV-ELF-4: PF_R \| PF_W \| PF_X == 0o7 |
| 5 | `test_pf_pow2` | INV-ELF-5: PF flags are powers of 2 |
| 6 | `test_is_load` | INV-ELF-6: PT_LOAD is_load |
| 7 | `test_not_load` | INV-ELF-7: PT_NULL is not load |
| 8 | `test_flag_combos` | INV-ELF-8: R/W/X flag combinations correct |
| 9 | `test_is_executable` | INV-ELF-9: ET_EXEC/ET_DYN are executable types |
| 10 | `test_et_none_not_exec` | INV-ELF-10: ET_NONE is not executable |
| 11 | `test_no_flags` | INV-ELF-11: No flags means no permissions |
| 12 | `test_rwx_all` | INV-ELF-12: RWX all set is valid |

### fs/permission_test (12)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_perm_bits` | INV-PERM-1: MAY_READ \| MAY_WRITE \| MAY_EXEC == 0o7 |
| 2 | `test_owner_read_644` | INV-PERM-2: Owner read on 0o644 |
| 3 | `test_other_read_644` | INV-PERM-3: Other read on 0o644 |
| 4 | `test_other_write_denied_644` | INV-PERM-4: Other write denied on 0o644 |
| 5 | `test_group_read_640` | INV-PERM-5: Group read on 0o640 |
| 6 | `test_owner_700` | INV-PERM-6: Owner can do everything on 0o700 |
| 7 | `test_mode_000` | INV-PERM-7: Nobody can do anything on 0o000 |
| 8 | `test_777` | INV-PERM-8: 0o777 allows all for matching category |
| 9 | `test_owner_priority` | INV-PERM-9: Owner takes priority over group/other |
| 10 | `test_exec_111` | INV-PERM-10: Exec on 0o111 for all |
| 11 | `test_no_exec_644` | INV-PERM-11: 0o644 denies exec for non-owners |
| 12 | `test_bit_positions` | INV-PERM-12: Permission bits in correct positions |

### fs/ext4/superblock_test (8)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_new` | INV-SB-1: new() has all features clear and inode_size 256 |
| 2 | `test_has_64bit` | INV-SB-2: has_64bit checks bit 7 |
| 3 | `test_has_extents` | INV-SB-3: has_extents checks bit 6 |
| 4 | `test_has_flex_bg` | INV-SB-4: has_flex_bg checks bit 9 |
| 5 | `test_no_features` | INV-SB-5: No features → all has_* return false |
| 6 | `test_independent` | INV-SB-6: Features are independent |
| 7 | `test_no_overlap` | INV-SB-7: Feature flag bits don't overlap |
| 8 | `test_pow2` | INV-SB-8: Feature bits are powers of 2 |

### arch/pt_regs_test (13)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_exception_codes` | INV-CAUSE-1: Exception codes produce exceptions |
| 2 | `test_interrupt_codes` | INV-CAUSE-2: Interrupt codes produce interrupts |
| 3 | `test_unknown_code` | INV-CAUSE-3: Unknown codes are exceptions |
| 4 | `test_page_fault_codes` | INV-CAUSE-4: Codes 12,13,15 are page faults |
| 5 | `test_ecall_user` | INV-CAUSE-5: Code 8 is exception (ECALL from U) |
| 6 | `test_ecall_supervisor` | INV-CAUSE-6: Code 9 is exception (ECALL from S) |
| 7 | `test_interrupt_exception_exclusive` | INV-CAUSE-7: No code is both interrupt and exception |
| 8 | `test_bit63_interrupt` | INV-CAUSE-8: Bit 63 distinguishes interrupts |
| 9 | `test_csr_spp` | INV-CAUSE-9: SR_SPP = 1 << 8 |
| 10 | `test_csr_pie_sie` | INV-CAUSE-10: SR_PIE/SR_SIE/SR_SUM values correct |
| 11 | `test_csr_fs_vs` | INV-CAUSE-11: SR_FS/SR_VS off/clean/dirty values correct |
| 12 | `test_uxl_distinct` | INV-CAUSE-12: SR_UXL_32 != SR_UXL_64 |
| 13 | `test_page_faults_not_interrupts` | INV-CAUSE-13: Page faults are exceptions not interrupts |

### arch/riscv64/mm/memory_layout_test (14)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_user_sign_extend` | INV-VA-1: User address (bit 38 = 0) clears upper bits |
| 2 | `test_kernel_sign_extend` | INV-VA-2: Kernel address (bit 38 = 1) sets upper bits |
| 3 | `test_vpn_level0` | INV-VA-3: VPN level 0 extracts bits [20:12] |
| 4 | `test_vpn_level1` | INV-VA-4: VPN level 1 extracts bits [29:21] |
| 5 | `test_vpn_level2` | INV-VA-5: VPN level 2 extracts bits [38:30] |
| 6 | `test_vpn_9bit` | INV-VA-6: VPN always returns 9-bit value (0..511) |
| 7 | `test_is_aligned` | INV-VA-7: is_aligned |
| 8 | `test_floor` | INV-VA-8: floor(addr) <= addr |
| 9 | `test_ceil` | INV-VA-9: ceil(addr) >= addr |
| 10 | `test_page_offset` | INV-VA-10: page_offset extracts low 12 bits |
| 11 | `test_ptrs_per_pte` | INV-VA-11: PTRS_PER_PTE == 512 |
| 12 | `test_floor_aligned` | INV-VA-12: floor of page-aligned address is itself |
| 13 | `test_va_mask` | INV-VA-13: VA_MASK covers all 39 bits |
| 14 | `test_zero_vpn` | INV-VA-14: VPN of zero address is 0 at all levels |

### sched/rt_bitmap_test (11)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_empty_bitmap` | INV-RT-BITMAP-1: All-zero bitmap returns None |
| 2 | `test_single_bit_word0` | INV-RT-BITMAP-2: Single bit in word0 found at correct index |
| 3 | `test_single_bit_word1` | INV-RT-BITMAP-3: Single bit in word1 found at index+64 |
| 4 | `test_word0_priority` | INV-RT-BITMAP-4: Word0 bits always take priority over word1 |
| 5 | `test_lowest_bit_wins` | INV-RT-BITMAP-5: Lowest set bit in word0 wins |
| 6 | `test_lowest_bit_w1` | INV-RT-BITMAP-6: Lowest set bit in word1 wins when word0 empty |
| 7 | `test_all_set_word0` | INV-RT-BITMAP-7: All bits set in word0 returns priority 0 |
| 8 | `test_all_set_both` | INV-RT-BITMAP-8: All bits set in both words returns 0 |
| 9 | `test_high_bit_word0` | INV-RT-BITMAP-9: Highest bit (63) in word0 found correctly |
| 10 | `test_high_bit_word1` | INV-RT-BITMAP-10: Highest valid bit (99) in word1 found correctly |
| 11 | `test_random_bitmap` | INV-RT-BITMAP-11: Random bitmap matches trailing_zeros |

### ipc/sysv_msg_test (10)

| # | Test | Invariant ID |
|---|------|-------------|
| 1 | `test_empty_queue` | INV-MSG-1: Empty queue returns None for all msgtyp |
| 2 | `test_receive_first` | INV-MSG-2: msgtyp=0 returns first message |
| 3 | `test_receive_exact_type` | INV-MSG-3: msgtyp>0 finds first message of exact type |
| 4 | `test_no_exact_match` | INV-MSG-4: msgtyp>0 returns None when no match |
| 5 | `test_msg_except` | INV-MSG-5: MSG_EXCEPT finds first non-matching message |
| 6 | `test_negative_msgtyp` | INV-MSG-6: msgtyp<0 finds lowest type <= |msgtyp| |
| 7 | `test_negative_no_match` | INV-MSG-7: msgtyp<0 returns None when all types > |msgtyp| |
| 8 | `test_negative_first_encountered` | INV-MSG-8: Negative msgtyp respects first-encountered for ties |
| 9 | `test_single_message` | INV-MSG-9: Single message queue behavior for all msgtyp |
| 10 | `test_msg_except_all_match` | INV-MSG-10: MSG_EXCEPT with no exceptable messages returns None |

## Maintenance

- **Adding new tests**: Copy relevant types/functions from kernel source, write proptest tests, update `scripts/verify_sync_check.py` mappings, and regenerate this report
- **Sync checking**: Run `python3 scripts/verify_sync_check.py` to detect kernel/verify divergence
- **Regression**: All 1,087 tests must pass before and after changes

## L2: Kani Symbolic Verification

**Tool**: [Kani](https://github.com/model-checking/kani) (CBMC-based, all-input symbolic execution)

| Metric | Value |
|--------|-------|
| **Proof harnesses** | 157 |
| **Modules covered** | 22 |
| **Run command** | `make kani` |
| **Environment** | Host (Kani/CBMC, all-input SAT/SMT) |

**Coverage**: mm (18), sync (2), arch (17), process (16), signal (17), drivers (17), ipc (5), fs (20), net (15), sched (12), interrupt (12), security (9), errno (5)

Kani proves properties hold for ALL possible inputs via SAT/SMT solvers. See [Kani harnesses](../../kernel/verify/src/) for details.

## L3: SPIN Concurrency Models

**Tool**: [SPIN/Promela](https://spinroot.com/) (model checking)

| Metric | Value |
|--------|-------|
| **Models** | 4 |
| **LTL properties** | 8 |
| **Run command** | `make spin` |
| **Environment** | Host (SPIN/Promela, concurrency) |

**Models**:
- `futex_wait_wake.pml` — No lost wakeup, no spurious sleep
- `lock_ordering.pml` — No deadlock cycle across 5 lock levels
- `interrupt_preempt.pml` — preempt_count bounded, no underflow
- `sched_enqueue_dequeue.pml` — nr_running consistency

## L4: Miri UB Detection

**Tool**: [Miri](https://github.com/rust-lang/miri) (undefined behavior detector)

| Metric | Value |
|--------|-------|
| **Run command** | `make miri` |
| **CI** | `.github/workflows/miri.yml` |
| **Environment** | Host (Miri, undefined behavior) |

Miri detects undefined behavior in test code, serving as a CI gate.
