# Formal Verification Test Report

> **Last updated**: 2026-04-08
> **Test command**: `cd kernel/verify && cargo test --target x86_64-unknown-linux-gnu`
> **Sync check**: `python3 scripts/verify_sync_check.py`

## Summary

| Metric | Value |
|--------|-------|
| **Total test cases** | 375 |
| **Test modules** | 32 |
| **Kernel subsystems covered** | 9 (mm, sync, arch, net, fs, security, signal, process, sched) |
| **Test framework** | [proptest](https://crates.io/crates/proptest) 1.5 (property-based, randomized) |
| **Environment** | std, host machine, `x86_64-unknown-linux-gnu` target |
| **Default cases per test** | 256 (configurable via `PROPTEST_CASES`) |
| **Result** | 375 passed, 0 failed |

## Approach

Each test file copies the relevant pure types and functions from `kernel/src/` into `kernel/verify/src/` and verifies invariants using proptest randomized input generation. This avoids a shared-crate dependency chain while keeping kernel source clean. When kernel types change, the copies here must be updated accordingly — the sync check script detects divergences automatically.

## Test Modules

### mm/ (Memory Management) — 102 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `page_flags_test` | 6 | `mm/page_desc.rs` | Bitmap set/test/clear, from_raw, clear_all, test_and_set |
| `buddy_test` | 11 | `mm/buddy_allocator.rs` | Alignment, buddy involution, pair contiguity, size_to_order, get_buddy_idx |
| `vma_test` | 9 | `mm/vma.rs` | Non-overlap, adjacent VMAs, overlap rejection, find, remove, split, contains, overlaps, can_merge |
| `refcount_test` | 6 | `mm/page_desc.rs` | Never negative, get/put symmetry, underflow protection, try_get |
| `list_test` | 8 | `mm/list.rs` | Circular list integrity, add/del, FIFO/LIFO, forward/backward symmetry, for_each |
| `buddy_alloc_test` | 11 | `mm/buddy_allocator.rs` | Order calculation, buddy involution, addr roundtrip, alloc+free conservation, merging |
| `zone_test` | 13 | `mm/zone.rs` | Newton's method int_sqrt, pfn/phys roundtrip, GFP→zone mapping, watermark formula |
| `vmscan_test` | 14 | `mm/vmscan.rs` | nr_to_scan priority-shift formula, ScanControl reclaim target, priority loop termination, LRU index bounds |
| `compact_test` | 16 | `mm/compact.rs` | CompactResult enum, scanner convergence, MAX_SCAN_PAGES limit, migration filter predicate (free/reserved/dirty/refcount) |
| `rmap_test` | 16 | `mm/rmap.rs` | Sv39 VPN extraction/reconstruction roundtrip, addr_to_vpn bounds, page_mapped/mapcount guards |

### sync/ (Synchronization) — 28 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `spinlock_test` | 4 | `sync/spinlock.rs` | try_lock/unlock, lock/unlock, unlock_unlocked, contention |
| `seqlock_test` | 8 | `sync/seqlock.rs` | Initial state, write mutates, locked state, try_write, sequence increments, read consistency, struct atomicity |
| `futex_test` | 16 | `sync/futex.rs` | FutexKey private/shared matching, futex_hash distribution, futex_to_flags, bitset intersection, opcode constants |

### net/ (Networking) — 61 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `route_test` | 9 | `net/ipv4/route.rs` | Longest-prefix match, host route, default route, masking, add/remove, interleaved ops |
| `arp_test` | 12 | `net/arp.rs` | LRU eviction, cache capacity, update/remove, packet parsing, MAC/IP extraction |
| `checksum_test` | 10 | `net/ipv4/checksum.rs` | RFC 1071 ones-complement, zero-length, complement identity, carry fold, pseudo-header |
| `tcp_test` | 16 | `net/tcp.rs` | RFC 6298 RTT estimator, RTO clamping/backoff, RFC 5681 congestion (slow start/CA/timeout), seq_before, TCP header flags |
| `ethernet_test` | 7 | `net/ethernet.rs` | MAC address classification: unicast/multicast/broadcast mutual exclusivity, addr_eq |
| `ipv4_udp_test` | 7 | `net/ipv4/mod.rs`, `net/udp.rs` | IPv4 header version/IHL, big-endian field roundtrips, UDP port/length/protocol accessors |

### fs/ (Filesystem) — 69 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `cmdline_test` | 14 | `cmdline.rs` | get_param, has_param, get_all_params, root device, init program, debug mode |
| `stat_test` | 11 | `fs/stat.rs` | File type mutual exclusivity, set/get mode roundtrip, type overwrite, random type+mode |
| `path_test` | 14 | `fs/path.rs` | Path normalization, dot/dotdot handling, root escape prevention, component splitting, parent/file_name |
| `ext4/indirect_test` | 10 | `fs/ext4/indirect.rs` | Direct/indirect block mapping, block iterator count, max_file_size, indirect level monotonicity |
| `ext4/allocator_test` | 12 | `fs/ext4/allocator.rs` | Bitmap scanner: start offset, max_bits, single free bit, all-ones/all-zeros, byte boundary |
| `jbd2/types_test` | 16 | `fs/jbd2/types.rs` | Journal header magic/block_type/sequence roundtrip, tag size calculation, feature flag power-of-2, tags_per_block |

### security/ (Security) — 18 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `capability_test` | 18 | `security/capability.rs` | POSIX capability bitmask: set/has/clear, boolean algebra (AND/OR/XOR/complement), De Morgan, subset, lo/hi halves |

### signal/ (Signal Handling) — 16 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `signal_test` | 16 | `signal.rs` | Signal bitmap add/has/remove, first/first_unmasked, SigAction classification, signal mask ops |

### process/ (Process Management) — 9 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `pid_test` | 9 | `process/pid.rs` | PID bitmap allocator: reserved range, uniqueness, free+realloc, exhaustion, double-free safety, nr_allocated |

### sched/ (Scheduler) — 34 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `fair_test` | 18 | `sched/fair.rs` | CFS weight/wmult table monotonicity, LoadWeight, calc_delta_fair vruntime arithmetic, sched_slice proportionality, check_preempt |
| `deadline_test` | 16 | `sched/deadline.rs` | DL bandwidth clamped to 100%, consume/replenish runtime, deadline advancement, monotonicity |

### arch/riscv64/mm/ (RISC-V MMU) — 13 tests

| Module | Tests | Kernel Source | Invariants Verified |
|--------|-------|---------------|---------------------|
| `pagetable_test` | 13 | `arch/riscv64/mm/pagetable.rs` | PTE flag bits, user/kernel/ro pages, is_leaf, ppn extraction, Satp fields |

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

## Maintenance

- **Adding new tests**: Copy relevant types/functions from kernel source, write proptest tests, update `scripts/verify_sync_check.py` mappings, and regenerate this report
- **Sync checking**: Run `python3 scripts/verify_sync_check.py` to detect kernel/verify divergence
- **Regression**: All 375 tests must pass before and after changes
