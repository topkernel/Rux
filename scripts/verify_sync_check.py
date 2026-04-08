#!/usr/bin/env python3
"""Verify sync checker — compare kernel/verify copies against kernel source.

Detects when kernel algorithm changes haven't been propagated to the verify
test copies in kernel/verify/src/. Extracts function bodies from both sides,
normalizes platform differences (core/std/alloc, pr_warn, etc.), and reports
divergences.

Usage:
    python3 scripts/verify_sync_check.py           # check all
    python3 scripts/verify_sync_check.py -v        # verbose (show diff context)
    python3 scripts/verify_sync_check.py mm/page   # filter to specific module

Exit code: 0 = all in sync, 1 = divergences found.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


# ============================================================
# Extraction helpers
# ============================================================

def extract_fn(text, name, impl_type=None):
    """Extract a function definition (signature + body) by name.

    If impl_type is given, only match functions within `impl TypeName { ... }`.
    Uses brace counting to find the full extent of the function.
    """
    pattern = re.compile(rf'\bfn\s+{re.escape(name)}\s*(?:<[^>]*>)?\s*\(')
    lines = text.split('\n')

    # If impl_type specified, find impl block boundaries first
    impl_ranges = []  # list of (start_line, end_line)
    if impl_type:
        i = 0
        while i < len(lines):
            stripped = lines[i].strip()
            if re.match(rf'\bimpl\s+{re.escape(impl_type)}\b', stripped):
                block_start = i
                depth = 0
                j = i
                found_open = False
                while j < len(lines):
                    for ch in lines[j]:
                        if ch == '{':
                            depth += 1
                            found_open = True
                        elif ch == '}':
                            depth -= 1
                    if found_open and depth <= 0:
                        impl_ranges.append((block_start, j))
                        break
                    j += 1
            i += 1

    def in_impl_range(line_idx):
        if not impl_ranges:
            return True  # no scoping
        return any(s <= line_idx <= e for s, e in impl_ranges)

    for i, line in enumerate(lines):
        if pattern.search(line) and in_impl_range(i):
            start = i
            depth = 0
            found_open = False
            j = i
            while j < len(lines):
                for ch in lines[j]:
                    if ch == '{':
                        depth += 1
                        found_open = True
                    elif ch == '}':
                        depth -= 1
                if found_open and depth <= 0:
                    return '\n'.join(lines[start:j + 1])
                j += 1
    return None


def extract_impl_pub_methods(text, type_name):
    """Extract all pub fn names from `impl TypeName { ... }` blocks.

    Skips trait impls (e.g. `impl Default for TypeName`).
    Returns a list of method names.
    """
    lines = text.split('\n')
    methods = []
    in_impl = False
    depth = 0

    i = 0
    while i < len(lines):
        stripped = lines[i].strip()
        if not in_impl:
            # Match `impl TypeName {` or `impl TypeName<...> {`
            if re.match(rf'\bimpl\s+{re.escape(type_name)}\b', stripped):
                if '{' in stripped:
                    depth = stripped.count('{') - stripped.count('}')
                    in_impl = depth > 0
                else:
                    # Opening brace on next line(s)
                    i += 1
                    while i < len(lines) and '{' not in lines[i]:
                        i += 1
                    if i < len(lines):
                        depth = lines[i].count('{') - lines[i].count('}')
                        in_impl = depth > 0
        elif in_impl:
            depth += stripped.count('{') - stripped.count('}')
            m = re.match(r'\bpub\s+(const\s+)?fn\s+(\w+)', stripped)
            if m:
                methods.append(m.group(2))
            if depth <= 0:
                in_impl = False
        i += 1

    return methods


# ============================================================
# Normalization
# ============================================================

def normalize(text):
    """Normalize platform differences between kernel (no_std) and verify (std)."""
    # core:: / std:: / alloc:: -> canonical __NS__
    text = re.sub(r'\bcore::', '__NS__', text)
    text = re.sub(r'\bstd::', '__NS__', text)
    text = re.sub(r'\balloc::', '__NS__', text)

    # Remove crate::pr_warn!(...) calls (kernel-only logging)
    text = re.sub(r'\s*crate::pr_warn!.*?;\s*\n?', '\n', text, flags=re.DOTALL)

    # Remove #[inline], #[inline(never)], #[inline(always)]
    text = re.sub(r'#\[inline[^\]]*\]\s*\n?', '', text)

    # Remove all comments (doc comments /// and regular //)
    text = re.sub(r'\s*//[^\n]*\n', '\n', text)

    # pub(crate) -> pub
    text = re.sub(r'pub\(crate\)\s+', 'pub ', text)

    # Collapse multiple blank lines
    text = re.sub(r'\n{2,}', '\n', text)

    # Strip trailing whitespace
    text = re.sub(r'[ \t]+$', '', text, flags=re.MULTILINE)

    return text.strip()


# ============================================================
# Mappings: verify file <-> kernel source + functions to compare
# ============================================================

MAPPINGS = [
    {
        "name": "mm/page_flags",
        "verify": "kernel/verify/src/mm/page_flags_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": "PageFlags",
        "compare": [
            "new", "from_raw", "raw", "test", "set", "clear",
            "test_and_set", "test_and_clear", "clear_all",
        ],
        "skip": [],
    },
    {
        "name": "mm/refcount",
        "verify": "kernel/verify/src/mm/refcount_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": "Page",
        "compare": ["get_page", "put_page", "try_get_page"],
        "skip": ["new"],  # struct layout intentionally simplified
        "skip_diff": ["put_page"],  # kernel has pr_warn + extra brace nesting
        "check_new": False,  # many intentionally uncopied methods
    },
    {
        "name": "mm/vma",
        "verify": "kernel/verify/src/mm/vma_test.rs",
        "kernel": "kernel/src/mm/vma.rs",
        "type": "Vma",
        "compare": [
            "contains", "overlaps", "split", "can_merge",
        ],
        "skip": ["new"],  # struct fields differ
        "check_new": False,  # many intentionally uncopied methods
    },
    {
        "name": "mm/vma_manager",
        "verify": "kernel/verify/src/mm/vma_test.rs",
        "kernel": "kernel/src/mm/vma.rs",
        "type": "VmaManager",
        "compare": ["add", "find", "remove"],
        "skip": ["new"],  # struct fields differ
        "check_new": False,  # many intentionally uncopied methods
    },
    {
        "name": "sync/spinlock",
        "verify": "kernel/verify/src/sync/spinlock_test.rs",
        "kernel": "kernel/src/sync/spinlock.rs",
        "type": "RawSpinlock",
        "compare": ["try_lock", "unlock", "is_locked"],
        "skip": ["lock", "new"],  # lock has deadlock warn stripped by design
    },
    {
        "name": "arch/riscv64/pagetable",
        "verify": "kernel/verify/src/arch/riscv64/mm/pagetable_test.rs",
        "kernel": "kernel/src/arch/riscv64/mm/pagetable.rs",
        "type": "PageTableEntry",
        "compare": [
            "from_bits", "bits", "is_valid", "is_readable", "is_writable",
            "is_executable", "is_user", "is_leaf", "ppn",
            "new_table", "new_page_kernel", "new_page_user", "new_page_ro",
        ],
        "skip": [],  # new() is const fn, same body
    },
    {
        "name": "arch/riscv64/satp",
        "verify": "kernel/verify/src/arch/riscv64/mm/pagetable_test.rs",
        "kernel": "kernel/src/arch/riscv64/mm/pagetable.rs",
        "type": "Satp",
        "compare": [
            "bits", "mode", "asid", "ppn", "is_bare", "is_sv39",
        ],
        "skip": ["new", "sv39"],  # const fn, same body but skip for noise
    },
    {
        "name": "list",
        "verify": "kernel/verify/src/mm/list_test.rs",
        "kernel": "kernel/src/list.rs",
        "type": "ListHead",
        "compare": [
            "new", "init", "is_empty", "add", "add_tail", "del", "for_each",
        ],
        "skip": [],
        "skip_diff": ["for_each"],  # kernel has crate::console::putchar debug output
        "check_new": False,  # entry/first_entry use kernel-specific types
    },
    {
        "name": "net/route_entry",
        "verify": "kernel/verify/src/net/route_test.rs",
        "kernel": "kernel/src/net/ipv4/route.rs",
        "type": "RouteEntry",
        "compare": ["new", "is_gateway", "is_host", "is_network", "matches"],
        "skip": [],
        "check_new": False,  # RouteTable uses Vec vs kernel's fixed array
    },
    {
        "name": "net/route_flags",
        "verify": "kernel/verify/src/net/route_test.rs",
        "kernel": "kernel/src/net/ipv4/route.rs",
        "type": "RouteFlags",
        "compare": [],
        "skip": [],
        "check_new": False,  # constants-only, no methods to compare
    },
    {
        "name": "net/arp_entry",
        "verify": "kernel/verify/src/net/arp_test.rs",
        "kernel": "kernel/src/net/arp.rs",
        "type": "ArpEntry",
        "compare": ["new", "is_expired"],
        "skip": [],
        "check_new": False,  # ArpCache uses Vec vs kernel's fixed array
    },
    {
        "name": "net/arp_packet",
        "verify": "kernel/verify/src/net/arp_test.rs",
        "kernel": "kernel/src/net/arp.rs",
        "type": "ArpPacket",
        "compare": ["is_request", "is_reply", "sender_mac", "sender_ip", "target_mac", "target_ip"],
        "skip": ["from_bytes"],  # kernel uses &'static, verify uses &
        "check_new": False,  # LEN is const, already covered
    },
    {
        "name": "net/checksum",
        "verify": "kernel/verify/src/net/checksum_test.rs",
        "kernel": "kernel/src/net/ipv4/checksum.rs",
        "type": None,  # free functions, not impl methods
        "compare": [
            "ip_checksum", "verify_ip_checksum", "pseudo_header_checksum",
        ],
        "skip": [],
    },
    {
        "name": "sync/seqlock",
        "verify": "kernel/verify/src/sync/seqlock_test.rs",
        "kernel": "kernel/src/sync/seqlock.rs",
        "type": "RawSeqLock",
        "compare": [
            "new", "try_write_lock", "write_unlock",
            "read_begin", "read_retry", "is_locked",
        ],
        "skip": [],
        "check_new": False,  # preempt hooks replaced with no-ops in verify
    },
    {
        "name": "fs/cmdline",
        "verify": "kernel/verify/src/fs/cmdline_test.rs",
        "kernel": "kernel/src/cmdline.rs",
        "type": None,  # free functions with modified signatures (cmdline param added)
        "compare": [],  # signatures differ (verify takes &str param, kernel uses global)
        "skip": [],
    },
    {
        "name": "mm/buddy_alloc",
        "verify": "kernel/verify/src/mm/buddy_alloc_test.rs",
        "kernel": "kernel/src/mm/buddy_allocator.rs",
        "type": "BuddyAllocator",
        "compare": [
            "init", "add_to_free_list", "remove_from_free_list",
            "alloc_blocks", "free_blocks",
        ],
        "skip": ["new"],  # verify uses Vec, kernel uses fixed-size array
        "skip_diff": [
            "init", "add_to_free_list", "remove_from_free_list",
            "alloc_blocks", "free_blocks",
        ],  # verify uses pub fn + Vec, kernel uses unsafe fn + fixed arrays
        "check_new": False,  # many kernel-only methods not copied
    },
    {
        "name": "mm/buddy_funcs",
        "verify": "kernel/verify/src/mm/buddy_alloc_test.rs",
        "kernel": "kernel/src/mm/buddy_allocator.rs",
        "type": None,  # free functions
        "compare": [
            "heap_size_to_order", "size_to_order", "get_buddy_idx",
            "page_idx_to_addr", "addr_to_page_idx",
        ],
        "skip_diff": [
            "heap_size_to_order", "size_to_order", "get_buddy_idx",
            "page_idx_to_addr", "addr_to_page_idx",
        ],  # verify uses pub const/const, kernel uses different visibility
    },
    # ---- Phase 2 mappings ----
    {
        "name": "security/capability",
        "verify": "kernel/verify/src/security/capability_test.rs",
        "kernel": "kernel/src/security/capability.rs",
        "type": "Cap",
        "compare": [
            "new", "has", "set", "clear",
            "intersect", "union", "xor", "complement",
            "is_subset_of", "is_empty", "bits", "lo", "hi", "from_halves",
        ],
        "skip": [],
        "skip_diff": ["new"],  # verify uses fn, kernel uses const fn
        "check_new": False,  # EMPTY/FULL are const, not methods
    },
    {
        "name": "signal/sig_pending",
        "verify": "kernel/verify/src/signal/signal_test.rs",
        "kernel": "kernel/src/signal.rs",
        "type": "SigPending",
        "compare": [
            "add", "remove", "has", "first", "first_unmasked", "get_all",
        ],
        "skip": ["new", "add_info", "first_info", "clear"],  # new uses AtomicU64; info methods use kernel-only types
        "skip_diff": ["add", "remove", "has", "first", "first_unmasked", "get_all"],
        # verify uses plain u64, kernel uses AtomicU64 with Ordering params
        "check_new": False,
    },
    {
        "name": "signal/sig_action",
        "verify": "kernel/verify/src/signal/signal_test.rs",
        "kernel": "kernel/src/signal.rs",
        "type": "SigAction",
        "compare": ["new", "ignore", "handler", "action", "has_handler"],
        "skip": [],
        "skip_diff": ["handler", "new", "ignore", "action"],
        # verify uses Self:: + plain 0, kernel uses SigAction:: + SigFlags::new(0)
        "check_new": False,
    },
    {
        "name": "fs/stat",
        "verify": "kernel/verify/src/fs/stat_test.rs",
        "kernel": "kernel/src/fs/stat.rs",
        "type": "Stat",
        "compare": [
            "new",
            "set_regular_file", "set_directory", "set_char_device", "set_block_device",
            "set_fifo", "set_symlink", "set_socket",
            "is_regular_file", "is_directory", "is_char_device", "is_block_device",
            "is_fifo", "is_symlink", "is_socket",
            "set_mode", "get_mode",
        ],
        "skip": [],
        "skip_diff": ["new"],  # verify uses simplified struct (fewer fields)
        "check_new": False,
    },
    {
        "name": "fs/path_normalize",
        "verify": "kernel/verify/src/fs/path_test.rs",
        "kernel": "kernel/src/fs/path.rs",
        "type": None,  # free function
        "compare": ["path_normalize"],
        "skip": [],
        "skip_diff": ["path_normalize"],  # verify uses alloc::string::String vs core:: in kernel
    },
    {
        "name": "fs/path",
        "verify": "kernel/verify/src/fs/path_test.rs",
        "kernel": "kernel/src/fs/path.rs",
        "type": "Path",
        "compare": [
            "new", "is_absolute", "is_empty", "as_str",
            "parent", "file_name",
        ],
        "skip": ["components", "join"],  # PathComponents uses lifetime params differently; join not copied
        "skip_diff": ["parent", "file_name", "new"],  # verify uses alloc::string::String vs core::; struct formatting
        "check_new": False,
    },
    {
        "name": "fs/path_component",
        "verify": "kernel/verify/src/fs/path_test.rs",
        "kernel": "kernel/src/fs/path.rs",
        "type": "PathComponent",
        "compare": ["new", "name", "is_current", "is_parent", "is_root", "is_empty"],
        "skip": [],
        "skip_diff": ["new"],  # struct field formatting only
        "check_new": False,
    },
    {
        "name": "net/tcp_rtt",
        "verify": "kernel/verify/src/net/tcp_test.rs",
        "kernel": "kernel/src/net/tcp.rs",
        "type": "TcpRttEstimator",
        "compare": ["new", "update", "backoff", "reset"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "net/tcp_congestion",
        "verify": "kernel/verify/src/net/tcp_test.rs",
        "kernel": "kernel/src/net/tcp.rs",
        "type": "TcpCongestion",
        "compare": ["new", "on_ack", "on_dup_ack", "on_timeout", "reset", "seq_before"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "net/tcp_hdr",
        "verify": "kernel/verify/src/net/tcp_test.rs",
        "kernel": "kernel/src/net/tcp.rs",
        "type": "TcpHdr",
        "compare": [
            "dof", "header_len", "syn", "ack", "fin", "rst", "psh", "window",
            "set_dof", "set_syn", "set_ack", "set_fin", "set_rst", "set_psh",
        ],
        "skip": ["from_bytes"],  # kernel uses &'static Self, verify uses &Self
        "check_new": False,
    },
    {
        "name": "net/ethernet",
        "verify": "kernel/verify/src/net/ethernet_test.rs",
        "kernel": "kernel/src/net/ethernet.rs",
        "type": None,  # free functions
        "compare": [
            "eth_is_valid_unicast_addr", "eth_is_multicast_addr",
            "eth_is_broadcast_addr", "eth_addr_eq",
        ],
        "skip": [],
        "skip_diff": ["eth_is_broadcast_addr"],  # verify uses *addr ==, kernel uses addr == &
    },
    {
        "name": "mm/zone_funcs",
        "verify": "kernel/verify/src/mm/zone_test.rs",
        "kernel": "kernel/src/mm/zone.rs",
        "type": None,  # free functions
        "compare": ["int_sqrt", "pfn_to_phys", "phys_to_pfn"],
        "skip_diff": ["int_sqrt"],  # verify has pub fn, kernel has private fn
    },
    {
        "name": "mm/zone_gfp",
        "verify": "kernel/verify/src/mm/zone_test.rs",
        "kernel": "kernel/src/mm/zone.rs",
        "type": "GfpFlags",
        "compare": ["zone_type"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/zone_watermark",
        "verify": "kernel/verify/src/mm/zone_test.rs",
        "kernel": "kernel/src/mm/zone.rs",
        "type": "Zone",
        "compare": ["watermark_ok"],
        "skip": [],
        "skip_diff": ["watermark_ok"],  # verify uses simplified struct fields
        "check_new": False,  # many kernel-only methods not copied
    },
    {
        "name": "fs/ext4/indirect",
        "verify": "kernel/verify/src/fs/ext4/indirect_test.rs",
        "kernel": "kernel/src/fs/ext4/indirect.rs",
        "type": "Ext4BlockIterator",
        "compare": ["new", "next_mapping"],
        "skip": [],
        "skip_diff": ["next_mapping"],  # kernel has extra local variables for debug extraction
        "check_new": False,
    },
    {
        "name": "fs/ext4/indirect_funcs",
        "verify": "kernel/verify/src/fs/ext4/indirect_test.rs",
        "kernel": "kernel/src/fs/ext4/indirect.rs",
        "type": None,  # free functions
        "compare": ["max_file_size", "get_indirect_level"],
        "skip": [],
    },
    {
        "name": "net/ipv4_hdr",
        "verify": "kernel/verify/src/net/ipv4_udp_test.rs",
        "kernel": "kernel/src/net/ipv4/mod.rs",
        "type": "IpHdr",
        "compare": [],  # verify defines its own accessor methods; kernel IpHdr only has from_bytes/compute_checksum/is_valid_checksum
        "skip": ["from_bytes", "compute_checksum", "is_valid_checksum"],  # use &'static and kernel types
        "check_new": False,
    },
    {
        "name": "net/udp_hdr",
        "verify": "kernel/verify/src/net/ipv4_udp_test.rs",
        "kernel": "kernel/src/net/udp.rs",
        "type": "UdpHdr",
        "compare": ["source", "dest", "len", "check"],
        "skip": ["from_bytes"],  # kernel uses &'static Self
        "skip_diff": ["source", "dest", "len", "check"],  # verify uses plain u16, kernel uses u16::from_be_bytes
        "check_new": False,
    },
    {
        "name": "process/pid_allocator",
        "verify": "kernel/verify/src/process/pid_test.rs",
        "kernel": "kernel/src/process/pid.rs",
        "type": "PidAllocator",
        "compare": [
            "scan_range", "find_next_zero",
        ],
        "skip": ["new"],  # verify uses [u64; N], kernel uses different internal state
        "skip_diff": [
            "scan_range", "find_next_zero",
        ],  # verify copies private methods as pub for testing; field access differs
        "check_new": False,
    },
    {
        "name": "process/pid_funcs",
        "verify": "kernel/verify/src/process/pid_test.rs",
        "kernel": "kernel/src/process/pid.rs",
        "type": None,  # standalone functions in kernel; methods on PidAllocator in verify
        "compare": [],  # alloc_pid/free_pid are methods in verify but standalone fns in kernel
        "skip": [],
    },
    # ---- Phase 3 mappings ----
    {
        "name": "sched/load_weight",
        "verify": "kernel/verify/src/sched/fair_test.rs",
        "kernel": "kernel/src/sched/fair.rs",
        "type": "LoadWeight",
        "compare": ["new", "from_nice", "update_inv_weight"],
        "skip": [],
        "skip_diff": ["new"],  # verify uses single-line struct init, kernel uses multi-line
        "check_new": False,
    },
    {
        "name": "sched/dl_entity",
        "verify": "kernel/verify/src/sched/deadline_test.rs",
        "kernel": "kernel/src/sched/deadline.rs",
        "type": "SchedDlEntity",
        "compare": ["new", "get_bw", "update_deadline", "replenish_runtime", "consume_runtime"],
        "skip": ["is_on_rq"],  # uses AtomicBool
        "skip_diff": ["new", "get_bw", "update_deadline", "replenish_runtime", "consume_runtime"],
        # verify uses plain u64/i64, kernel uses AtomicU64/AtomicI64/AtomicBool
        "check_new": False,
    },
    {
        "name": "sync/futex_key",
        "verify": "kernel/verify/src/sync/futex_test.rs",
        "kernel": "kernel/src/sync/futex.rs",
        "type": "FutexKey",
        "compare": ["new", "matches"],
        "skip": [],
        "skip_diff": ["matches"],  # verify uses FLAGS_SHARED constant directly
        "check_new": False,
    },
    {
        "name": "sync/futex_flags",
        "verify": "kernel/verify/src/sync/futex_test.rs",
        "kernel": "kernel/src/sync/futex.rs",
        "type": None,  # free function
        "compare": ["futex_to_flags"],
        "skip": [],
        "skip_diff": ["futex_to_flags"],  # verify uses copy of constants
    },
    {
        "name": "fs/jbd2/header",
        "verify": "kernel/verify/src/fs/jbd2/types_test.rs",
        "kernel": "kernel/src/fs/jbd2/types.rs",
        "type": "journal_header_t",
        "compare": ["new", "is_valid", "block_type", "sequence"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/jbd2/tag_size",
        "verify": "kernel/verify/src/fs/jbd2/types_test.rs",
        "kernel": "kernel/src/fs/jbd2/types.rs",
        "type": None,  # const fn
        "compare": [],
        "skip": [],  # journal_tag_size and journal_tags_per_block are const fns
    },
    {
        "name": "fs/ext4/allocator",
        "verify": "kernel/verify/src/fs/ext4/allocator_test.rs",
        "kernel": "kernel/src/fs/ext4/allocator.rs",
        "type": None,  # free function
        "compare": ["find_free_bit"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/vmscan",
        "verify": "kernel/verify/src/mm/vmscan_test.rs",
        "kernel": "kernel/src/mm/vmscan.rs",
        "type": None,  # free function
        "compare": [],
        "skip": [],  # nr_to_scan is private in kernel; verify copies the pure arithmetic
    },
    {
        "name": "mm/compact",
        "verify": "kernel/verify/src/mm/compact_test.rs",
        "kernel": "kernel/src/mm/compact.rs",
        "type": None,  # struct + enum
        "compare": [],
        "skip": [],  # CompactControl is private; verify extracts the termination logic
    },
    {
        "name": "mm/rmap",
        "verify": "kernel/verify/src/mm/rmap_test.rs",
        "kernel": "kernel/src/mm/rmap.rs",
        "type": None,  # free functions + predicates
        "compare": [],
        "skip": [],  # verify extracts pure arithmetic (addr_to_vpn, sv39_vpn_indices, page_mapped)
    },
    # ---- Phase 4 mappings ----
    {
        "name": "fs/inode_mode",
        "verify": "kernel/verify/src/fs/inode_test.rs",
        "kernel": "kernel/src/fs/inode.rs",
        "type": "InodeMode",
        "compare": [
            "new", "is_regular_file", "is_directory", "is_char_device",
            "is_block_device", "is_fifo", "is_symlink", "is_socket", "bits",
        ],
        "skip": [],
        "check_new": False,  # constants-only, no new methods expected
    },
    {
        "name": "fs/inode_hash",
        "verify": "kernel/verify/src/fs/inode_test.rs",
        "kernel": "kernel/src/fs/inode.rs",
        "type": None,  # free function
        "compare": ["inode_hash"],
        "skip": [],
        "skip_diff": ["inode_hash"],  # verify uses pub fn, kernel uses fn
    },
    {
        "name": "fs/file_flags",
        "verify": "kernel/verify/src/fs/file_test.rs",
        "kernel": "kernel/src/fs/file.rs",
        "type": "FileFlags",
        "compare": ["new", "is_readonly", "is_writeonly", "is_rdwr", "bits", "set_bits", "add_flags"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/page_flags_ops",
        "verify": "kernel/verify/src/mm/page_flags_ops_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": "PageFlags",
        "compare": ["new", "from_raw", "raw", "test", "set", "clear", "test_and_set", "test_and_clear", "clear_all"],
        "skip": [],
        "skip_diff": ["new", "from_raw", "raw", "test", "set", "clear", "test_and_set", "test_and_clear", "clear_all"],
        # verify uses plain u32 + &mut self, kernel uses AtomicU32 + &self with Ordering
        "check_new": False,
    },
    {
        "name": "sched/rt_entity",
        "verify": "kernel/verify/src/sched/rt_test.rs",
        "kernel": "kernel/src/sched/rt.rs",
        "type": "SchedRtEntity",
        "compare": ["new", "is_on_rq", "set_on_rq", "get_time_slice", "set_time_slice", "dec_time_slice", "reset_time_slice"],
        "skip": [],
        "skip_diff": ["new", "is_on_rq", "set_on_rq", "get_time_slice", "set_time_slice", "dec_time_slice", "reset_time_slice"],
        # verify uses plain u32/bool, kernel uses AtomicU32/AtomicBool with Ordering params
        "check_new": False,
    },
    {
        "name": "sched/rt_bitmap",
        "verify": "kernel/verify/src/sched/rt_test.rs",
        "kernel": "kernel/src/sched/rt.rs",
        "type": "RtRunQueue",
        "compare": [],
        "skip": [],  # find_highest_prio is private in kernel; verify extracts pure bitmap logic
        "check_new": False,
    },
    {
        "name": "fs/ext4/namei",
        "verify": "kernel/verify/src/fs/ext4/namei_test.rs",
        "kernel": "kernel/src/fs/ext4/namei.rs",
        "type": None,  # free functions
        "compare": ["find_entry_space", "add_entry_to_block", "create_initial_entry", "create_dot_entry", "create_dotdot_entry", "find_prev_entry"],
        "skip": [],
        "skip_diff": ["create_initial_entry", "create_dot_entry", "create_dotdot_entry"],
        # verify uses local EXT4_FT_DIR constant, kernel uses file_type::EXT4_FT_DIR; kernel has unused _entry_len
    },
    {
        "name": "fs/dentry_flags",
        "verify": "kernel/verify/src/fs/dentry_test.rs",
        "kernel": "kernel/src/fs/dentry.rs",
        "type": "DentryFlags",
        "compare": ["new", "is_hashed", "is_unhashed", "bits"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/dentry_hash",
        "verify": "kernel/verify/src/fs/dentry_test.rs",
        "kernel": "kernel/src/fs/dentry.rs",
        "type": None,  # free function
        "compare": ["dentry_hash"],
        "skip": [],
    },
    {
        "name": "interrupt/irq_data",
        "verify": "kernel/verify/src/interrupt/irq_test.rs",
        "kernel": "kernel/src/interrupt/irqdesc.rs",
        "type": "IrqData",
        "compare": ["new"],
        "skip": [],
        "skip_diff": ["new"],  # verify uses plain usize for chip/chip_data, kernel uses Option<&IrqChip>/usize
        "check_new": False,
    },
    # ---- Phase 5 mappings ----
    {
        "name": "mm/swap_entry",
        "verify": "kernel/verify/src/mm/swap_test.rs",
        "kernel": "kernel/src/mm/swap.rs",
        "type": None,  # free functions
        "compare": ["make_swap_entry", "is_swap_entry", "swap_entry_type", "swap_entry_offset"],
        "skip": [],
        "skip_diff": ["make_swap_entry", "is_swap_entry", "swap_entry_type", "swap_entry_offset"],
        # verify uses pub const fn, kernel uses pub const fn; but verify body uses direct bit ops vs kernel's
        "check_new": False,
    },
    {
        "name": "fs/dev_no",
        "verify": "kernel/verify/src/fs/dev_t_test.rs",
        "kernel": "kernel/src/fs/dev_t.rs",
        "type": "DevNo",
        "compare": ["new", "from_u64", "to_u64"],
        "skip": [],
        "check_new": False,  # constants-only, no new methods expected
    },
    {
        "name": "arch/pt_regs_cause",
        "verify": "kernel/verify/src/arch/pt_regs_test.rs",
        "kernel": "kernel/src/arch/riscv64/pt_regs.rs",
        "type": "Cause",
        "compare": ["from_cause", "is_interrupt", "is_exception", "is_page_fault"],
        "skip": ["code"],  # kernel has no code() method
        "skip_diff": ["from_cause"],  # verify uses Self::Variant(x), kernel may format differently
        "check_new": False,
    },
    {
        "name": "mm/page_physaddr",
        "verify": "kernel/verify/src/mm/page_addr_test.rs",
        "kernel": "kernel/src/mm/page.rs",
        "type": "PhysAddr",
        "compare": ["new", "as_usize", "is_aligned", "floor", "ceil", "frame_number", "ppn"],
        "skip": [],
        "skip_diff": ["frame_number"],  # verify returns usize, kernel returns PhysFrameNr type alias
        "check_new": False,
    },
    {
        "name": "mm/page_virtaddr",
        "verify": "kernel/verify/src/mm/page_addr_test.rs",
        "kernel": "kernel/src/mm/page.rs",
        "type": "VirtAddr",
        "compare": ["new", "as_usize", "is_aligned", "floor", "ceil", "page_number"],
        "skip": [],
        "skip_diff": ["page_number"],  # verify returns usize, kernel returns VirtPageNr type alias
        "check_new": False,
    },
    {
        "name": "mm/phys_frame",
        "verify": "kernel/verify/src/mm/page_addr_test.rs",
        "kernel": "kernel/src/mm/page.rs",
        "type": "PhysFrame",
        "compare": ["new", "containing_address", "start_address", "range"],
        "skip": [],
        "skip_diff": ["new", "range"],  # verify uses usize param, kernel uses PhysFrameNr alias; range same normalization
        "check_new": False,
    },
    {
        "name": "mm/virt_page",
        "verify": "kernel/verify/src/mm/page_addr_test.rs",
        "kernel": "kernel/src/mm/page.rs",
        "type": "VirtPage",
        "compare": ["new", "containing_address", "start_address", "range"],
        "skip": [],
        "skip_diff": ["new", "range"],  # verify uses usize param, kernel uses VirtPageNr alias
        "check_new": False,
    },
    {
        "name": "fs/ext4/superblock",
        "verify": "kernel/verify/src/fs/ext4/superblock_test.rs",
        "kernel": "kernel/src/fs/ext4/superblock.rs",
        "type": "Ext4FsState",
        "compare": ["new", "has_64bit", "has_extents", "has_flex_bg"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/elf_phdr",
        "verify": "kernel/verify/src/fs/elf_test.rs",
        "kernel": "kernel/src/fs/elf.rs",
        "type": "Elf64Phdr",
        "compare": ["is_load", "is_readable", "is_writable", "is_executable"],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/permission",
        "verify": "kernel/verify/src/fs/permission_test.rs",
        "kernel": "kernel/src/fs/permission.rs",
        "type": None,  # free function
        "compare": ["generic_permission"],
        "skip": [],
        "skip_diff": ["generic_permission"],
        # verify uses simplified Cred struct (euid/egid only) and no CAP_DAC_OVERRIDE check
        "check_new": False,
    },
    {
        "name": "arch/riscv64/mm/memory_layout",
        "verify": "kernel/verify/src/arch/riscv64/mm/memory_layout_test.rs",
        "kernel": "kernel/src/arch/riscv64/mm/memory_layout.rs",
        "type": "VirtAddr",
        "compare": ["new", "bits", "is_aligned", "floor", "ceil", "page_offset", "vpn", "as_u64"],
        "skip": ["as_usize"],  # not in verify copy (u64-only)
        "check_new": False,
    },
    # ---- Phase 6 mappings ----
    {
        "name": "mm/slab_find_cache_index",
        "verify": "kernel/verify/src/mm/slab_test.rs",
        "kernel": "kernel/src/mm/slab.rs",
        "type": None,  # free function (private in kernel, extracted in verify)
        "compare": ["find_cache_index"],
        "skip": [],
        "skip_diff": ["find_cache_index"],  # verify copies as standalone fn, kernel has it in impl SlabAllocator
        "check_new": False,
    },
    {
        "name": "arch/riscv64/mm/asid_satp",
        "verify": "kernel/verify/src/arch/riscv64/mm/asid_test.rs",
        "kernel": "kernel/src/arch/riscv64/mm/asid.rs",
        "type": None,  # free functions
        "compare": ["build_satp", "satp_to_asid", "satp_to_ppn"],
        "skip": [],
        "skip_diff": ["build_satp", "satp_to_asid", "satp_to_ppn"],  # verify copies as standalone fn with #[inline(always)]
        "check_new": False,
    },
    {
        "name": "net/eth_protocol",
        "verify": "kernel/verify/src/net/buffer_test.rs",
        "kernel": "kernel/src/net/buffer.rs",
        "type": "EthProtocol",
        "compare": ["from_u16", "to_u16"],
        "skip": [],
        "skip_diff": ["to_u16"],  # verify: `self as u16`, kernel: same but extracted from impl context
        "check_new": False,
    },
    {
        "name": "net/ip_protocol",
        "verify": "kernel/verify/src/net/buffer_test.rs",
        "kernel": "kernel/src/net/buffer.rs",
        "type": "IpProtocol",
        "compare": ["from_u8", "to_u8"],
        "skip": [],
        "skip_diff": ["to_u8"],  # verify: `self as u8`, kernel: same but extracted from impl context
        "check_new": False,
    },
    {
        "name": "signal/sig_flags",
        "verify": "kernel/verify/src/signal/sigpending_test.rs",
        "kernel": "kernel/src/signal.rs",
        "type": "SigFlags",
        "compare": ["new", "bits"],
        "skip": [],
        "skip_diff": ["new", "bits"],  # verify copies struct with Self(flags), kernel uses same; verify has #[derive(PartialEq)]
        "check_new": False,
    },
    {
        "name": "errno/errno_enum",
        "verify": "kernel/verify/src/errno_test.rs",
        "kernel": "kernel/src/errno.rs",
        "type": "Errno",
        "compare": ["as_i32", "as_neg_i32", "as_neg_u64"],
        "skip": [],
        "skip_diff": ["as_i32", "as_neg_i32", "as_neg_u64"],  # verify copies methods, kernel has #[inline]
        "check_new": False,
    },
    {
        "name": "sched/sched_class_id",
        "verify": "kernel/verify/src/sched/class_test.rs",
        "kernel": "kernel/src/sched/class.rs",
        "type": "SchedClassId",
        "compare": [],
        "skip": [],  # enum-only, no methods to compare
        "check_new": False,
    },
    {
        "name": "interrupt/softirq_index",
        "verify": "kernel/verify/src/interrupt/softirq_test.rs",
        "kernel": "kernel/src/interrupt/softirq.rs",
        "type": "SoftirqIndex",
        "compare": [],
        "skip": [],  # enum-only, no methods to compare
        "check_new": False,
    },
    {
        "name": "net/icmp_hdr",
        "verify": "kernel/verify/src/net/icmp_test.rs",
        "kernel": "kernel/src/net/icmp.rs",
        "type": "IcmpHdr",
        "compare": [],
        "skip": [],  # struct-only, methods not copied
        "check_new": False,
    },
    {
        "name": "sync/rwlock_constants",
        "verify": "kernel/verify/src/sync/rwlock_test.rs",
        "kernel": "kernel/src/sync/rwlock.rs",
        "type": None,  # constants only, no struct methods copied
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    # ---- Phase 7 mappings ----
    {
        "name": "mm/hugepage_align",
        "verify": "kernel/verify/src/mm/hugepage_test.rs",
        "kernel": "kernel/src/mm/hugepage.rs",
        "type": None,  # alignment helper free functions
        "compare": [
            "is_pmd_aligned", "is_pgd_aligned",
            "pmd_align_down", "pmd_align_up",
            "pgd_align_down", "pgd_align_up",
        ],
        "skip": [],
        "skip_diff": ["is_pmd_aligned", "is_pgd_aligned", "pmd_align_down", "pmd_align_up", "pgd_align_down", "pgd_align_up"],
        # verify uses pub const + plain constants; kernel uses pub fn + super:: imports
        "check_new": False,
    },
    {
        "name": "fs/superblock_flags",
        "verify": "kernel/verify/src/fs/superblock_test.rs",
        "kernel": "kernel/src/fs/superblock.rs",
        "type": "SuperBlockFlags",
        "compare": ["new", "is_readonly", "is_active", "bits"],
        "skip": [],
        "skip_diff": ["new", "is_readonly", "is_active", "bits"],
        # verify: Self(flags); kernel: Self(flags) — but verify has #[derive(PartialEq)], kernel has #[repr(C)]
        "check_new": False,
    },
    {
        "name": "fs/mount_flags",
        "verify": "kernel/verify/src/fs/mount_test.rs",
        "kernel": "kernel/src/fs/mount.rs",
        "type": "MntFlags",
        "compare": ["new", "is_readonly", "is_noexec", "is_nosuid", "bits"],
        "skip": [],
        "skip_diff": ["new", "is_readonly", "is_noexec", "is_nosuid", "bits"],
        # verify has #[derive(Debug, Copy, Clone, PartialEq)], kernel has #[repr(C)] + same derives
        "check_new": False,
    },
    {
        "name": "fs/file_flags_ext",
        "verify": "kernel/verify/src/fs/file_flags_test.rs",
        "kernel": "kernel/src/fs/file.rs",
        "type": "FileFlags",
        "compare": ["new", "is_readonly", "is_writeonly", "is_rdwr", "bits"],
        "skip": [],
        "skip_diff": ["new", "is_readonly", "is_writeonly", "is_rdwr", "bits"],
        # verify has simplified derives; kernel has #[repr(C)]
        "check_new": False,
    },
    {
        "name": "mm/vmemmap_constants",
        "verify": "kernel/verify/src/mm/vmemmap_test.rs",
        "kernel": "kernel/src/mm/vmemmap.rs",
        "type": None,  # constants-only, no fn copies
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/config_constants",
        "verify": "kernel/verify/src/mm/config_test.rs",
        "kernel": "kernel/src/config.rs",
        "type": None,  # constants-only, auto-generated from Kernel.toml
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/page_flag_constants",
        "verify": "kernel/verify/src/mm/page_flag_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": None,  # PageFlag enum constants, no methods copied
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/hugepage_constants",
        "verify": "kernel/verify/src/mm/hugepage_test.rs",
        "kernel": "kernel/src/mm/hugepage.rs",
        "type": None,  # constants-only for size/shift/mask
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    # ---- Phase 8 mappings ----
    {
        "name": "ipc/ipc_id",
        "verify": "kernel/verify/src/ipc/ipc_id_test.rs",
        "kernel": "kernel/src/ipc/util.rs",
        "type": None,  # free functions
        "compare": ["ipc_build_id", "ipc_id_to_index", "ipc_id_seq", "ipc_update_mode", "owner_bits", "group_bits", "other_bits"],
        "skip": [],
        "skip_diff": ["ipc_build_id", "ipc_id_to_index", "ipc_id_seq", "ipc_update_mode", "owner_bits", "group_bits", "other_bits"],
        # verify copies standalone fns with simplified signatures; kernel uses private methods
        "check_new": False,
    },
    {
        "name": "drivers/virtio_offset",
        "verify": "kernel/verify/src/drivers/virtio_offset_test.rs",
        "kernel": "kernel/src/drivers/virtio/offset.rs",
        "type": None,  # constants-only, no fn copies
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "drivers/virtio_queue",
        "verify": "kernel/verify/src/drivers/virtio_queue_test.rs",
        "kernel": "kernel/src/drivers/virtio/queue.rs",
        "type": None,  # struct layout constants, no fn copies
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "drivers/pci_offset",
        "verify": "kernel/verify/src/drivers/pci_offset_test.rs",
        "kernel": "kernel/src/drivers/pci/mod.rs",
        "type": None,  # constants + free functions
        "compare": ["is_io_bar", "is_memory_bar", "is_64bit_memory_bar"],
        "skip": [],
        "skip_diff": ["is_io_bar", "is_memory_bar", "is_64bit_memory_bar"],
        # verify copies as standalone fns; kernel has in impl block
        "check_new": False,
    },
    {
        "name": "net/tcp_state",
        "verify": "kernel/verify/src/net/tcp_state_test.rs",
        "kernel": "kernel/src/net/tcp.rs",
        "type": None,  # enum constants + free functions
        "compare": ["tcp_dof", "tcp_header_len", "tcp_syn", "tcp_ack", "tcp_fin", "tcp_rst", "tcp_psh"],
        "skip": [],
        "skip_diff": ["tcp_dof", "tcp_header_len", "tcp_syn", "tcp_ack", "tcp_fin", "tcp_rst", "tcp_psh"],
        # verify copies as standalone fns; kernel methods are on TcpHdr impl
        "check_new": False,
    },
    {
        "name": "net/socket",
        "verify": "kernel/verify/src/net/socket_test.rs",
        "kernel": "kernel/src/net/socket.rs",
        "type": "SockAddrIn",
        "compare": ["port", "addr"],
        "skip": [],
        "skip_diff": ["port", "addr"],
        # verify copies as standalone impl; kernel has additional derives/lifetime
        "check_new": False,
    },
    {
        "name": "mm/memblock",
        "verify": "kernel/verify/src/mm/memblock_test.rs",
        "kernel": "kernel/src/mm/memblock.rs",
        "type": "MemBlockRegion",
        "compare": ["new", "end", "contains", "base_pfn", "end_pfn", "page_count"],
        "skip": [],
        "skip_diff": ["new", "end", "contains", "base_pfn", "end_pfn", "page_count"],
        # verify uses simplified struct (fewer fields); uses pub const fn vs kernel fn
        "check_new": False,
    },
    # ---- Phase 9 mappings ----
    {
        "name": "fs/readahead",
        "verify": "kernel/verify/src/fs/readahead_test.rs",
        "kernel": "kernel/src/fs/readahead.rs",
        "type": "ReadAheadState",
        "compare": ["new", "on_read"],
        "skip": [],
        "skip_diff": ["new", "on_read"],
        # verify uses plain types; kernel uses same logic
        "check_new": False,
    },
    {
        "name": "fs/pipe_buffer",
        "verify": "kernel/verify/src/fs/pipe_test.rs",
        "kernel": "kernel/src/fs/pipe.rs",
        "type": "PipeBuffer",
        "compare": ["new", "read", "write", "available_read", "available_write"],
        "skip": [],
        "skip_diff": ["new", "read", "write", "available_read", "available_write"],
        # verify uses Vec + plain usize; kernel uses unsafe Vec + AtomicUsize
        "check_new": False,
    },
    {
        "name": "interrupt/preempt",
        "verify": "kernel/verify/src/interrupt/preempt_test.rs",
        "kernel": "kernel/src/interrupt/preempt.rs",
        "type": None,  # constants + free functions
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/layout",
        "verify": "kernel/verify/src/mm/layout_test.rs",
        "kernel": "kernel/src/mm/layout.rs",
        "type": "KernelMemoryLayout",
        "compare": ["init_from_memblock"],
        "skip": [],
        "skip_diff": ["init_from_memblock"],
        # verify copies struct + fn directly; kernel has additional global functions
        "check_new": False,
    },
    {
        "name": "fs/ext4/extent",
        "verify": "kernel/verify/src/fs/ext4/extent_test.rs",
        "kernel": "kernel/src/fs/ext4/extent.rs",
        "type": "Ext4Extent",
        "compare": ["start_block", "length"],
        "skip": [],
        "skip_diff": ["start_block", "length"],
        # verify copies methods directly; kernel has additional logical_end/physical_end
        "check_new": False,
    },
    {
        "name": "fs/ext4/extent_idx",
        "verify": "kernel/verify/src/fs/ext4/extent_test.rs",
        "kernel": "kernel/src/fs/ext4/extent.rs",
        "type": "Ext4ExtentIdx",
        "compare": ["leaf_block"],
        "skip": [],
        "skip_diff": ["leaf_block"],
        "check_new": False,
    },
    {
        "name": "fs/umask",
        "verify": "kernel/verify/src/fs/umask_test.rs",
        "kernel": "kernel/src/fs/fs_struct.rs",
        "type": None,  # free functions extracted from apply_umask/set_umask
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/jbd2/wrap",
        "verify": "kernel/verify/src/fs/jbd2/wrap_test.rs",
        "kernel": "kernel/src/fs/jbd2/recovery.rs",
        "type": None,  # private fn wrap_block extracted as standalone
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/jbd2/journal_space",
        "verify": "kernel/verify/src/fs/jbd2/wrap_test.rs",
        "kernel": "kernel/src/fs/jbd2/checkpoint.rs",
        "type": None,  # free functions extracted
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "drivers/netdev_flags",
        "verify": "kernel/verify/src/drivers/netdev_test.rs",
        "kernel": "kernel/src/drivers/net/space.rs",
        "type": None,  # constants + flag operations
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    # ---- Phase 10 mappings ----
    {
        "name": "mm/oom_kill",
        "verify": "kernel/verify/src/mm/oom_kill_test.rs",
        "kernel": "kernel/src/mm/oom_kill.rs",
        "type": None,  # scoring formula extracted as standalone fn
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/meminfo",
        "verify": "kernel/verify/src/mm/meminfo_test.rs",
        "kernel": "kernel/src/mm/meminfo.rs",
        "type": None,  # threshold functions extracted as standalone fns
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "security/lsm",
        "verify": "kernel/verify/src/security/lsm_test.rs",
        "kernel": "kernel/src/security/lsm.rs",
        "type": None,  # HookId enum + sorted insertion logic
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "security/cap_lsm",
        "verify": "kernel/verify/src/security/cap_lsm_test.rs",
        "kernel": "kernel/src/security/cap_lsm.rs",
        "type": "CapLsm",
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "sync/semaphore",
        "verify": "kernel/verify/src/sync/semaphore_test.rs",
        "kernel": "kernel/src/sync/semaphore.rs",
        "type": "Semaphore",
        "compare": ["down", "down_trylock", "up", "count"],
        "skip": ["new", "down_interruptible", "init"],
        "skip_diff": ["down", "down_trylock", "up", "count"],
        # verify uses plain i32, kernel uses AtomicI32 with Ordering params
        "check_new": False,
    },
    {
        "name": "fs/io_completion",
        "verify": "kernel/verify/src/fs/io_completion_test.rs",
        "kernel": "kernel/src/fs/io_completion.rs",
        "type": "IoCompletion",
        "compare": ["new", "complete", "is_done", "try_wait", "reset"],
        "skip": ["wait"],
        "skip_diff": ["new", "complete", "is_done", "try_wait", "reset"],
        # verify uses plain bool/i32, kernel uses AtomicBool/AtomicI32 with Ordering params
        "check_new": False,
    },
    {
        "name": "fs/wait_for_all",
        "verify": "kernel/verify/src/fs/io_completion_test.rs",
        "kernel": "kernel/src/fs/io_completion.rs",
        "type": None,  # free function
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "interrupt/domain",
        "verify": "kernel/verify/src/interrupt/domain_test.rs",
        "kernel": "kernel/src/interrupt/domain.rs",
        "type": None,  # IrqDomain identity mapping logic
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    # ---- Phase 11 mappings ----
    {
        "name": "process/exit_status",
        "verify": "kernel/verify/src/process/exit_status_test.rs",
        "kernel": "kernel/src/process/exit.rs",
        "type": None,  # POSIX wait status encoding extracted as standalone fns
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "process/task_state",
        "verify": "kernel/verify/src/process/task_state_test.rs",
        "kernel": "kernel/src/process/task.rs",
        "type": "TaskState",
        "compare": ["new", "bits", "contains", "is_running", "is_sleeping", "is_dead", "is_interruptible"],
        "skip": [],
        "skip_diff": ["new", "bits", "contains", "is_running", "is_sleeping", "is_dead", "is_interruptible"],
        # verify uses plain u32 newtype; kernel has same logic
        "check_new": False,
    },
    {
        "name": "process/cred",
        "verify": "kernel/verify/src/process/cred_test.rs",
        "kernel": "kernel/src/process/task.rs",
        "type": "Cred",
        "compare": ["new_init", "new_user"],
        "skip": [],
        "skip_diff": ["new_init", "new_user"],
        # verify uses simplified Cap type instead of importing capability module
        "check_new": False,
    },
    {
        "name": "drivers/input/event",
        "verify": "kernel/verify/src/drivers/input/event_test.rs",
        "kernel": "kernel/src/drivers/input/event.rs",
        "type": None,  # constants + InputEvent struct
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/page_offset",
        "verify": "kernel/verify/src/fs/page_offset_test.rs",
        "kernel": "kernel/src/fs/buffer.rs",
        "type": None,  # page offset/index arithmetic extracted as standalone fns
        "compare": [],
        "skip": [],
        "check_new": False,
    },

    # ---- Phase 12 mappings ----
    {
        "name": "fs/ext4/dir",
        "verify": "kernel/verify/src/fs/ext4/dir_test.rs",
        "kernel": "kernel/src/fs/ext4/dir.rs",
        "type": None,  # Ext4DirEntry + iterator + find_entry extracted
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/ext4/inode",
        "verify": "kernel/verify/src/fs/ext4/inode_test.rs",
        "kernel": "kernel/src/fs/ext4/inode.rs",
        "type": None,  # Ext4InodeOnDisk/Ext4Inode + get_block_nr extracted
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "net/transport_checksum",
        "verify": "kernel/verify/src/net/transport_checksum_test.rs",
        "kernel": "kernel/src/net/udp.rs",
        "type": None,  # UDP/ICMP/TCP checksum functions extracted (uses ip_checksum for ICMP)
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "net/checksum_verify",
        "verify": "kernel/verify/src/net/checksum_verify_test.rs",
        "kernel": "kernel/src/net/ipv4/checksum.rs",
        "type": None,  # ip_checksum extracted
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "fs/bio",
        "verify": "kernel/verify/src/fs/bio_test.rs",
        "kernel": "kernel/src/fs/bio.rs",
        "type": None,  # BufferState + hash_index extracted
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    {
        "name": "mm/pfn_valid",
        "verify": "kernel/verify/src/mm/pfn_valid_test.rs",
        "kernel": "kernel/src/mm/page_desc.rs",
        "type": None,  # pfn_valid/phys_valid + constants extracted
        "compare": [],
        "skip": [],
        "check_new": False,
    },
    # ---- Phase 13 mappings ----
    {
        "name": "sched/rt_bitmap",
        "verify": "kernel/verify/src/sched/rt_bitmap_test.rs",
        "kernel": "kernel/src/sched/rt.rs",
        "type": "RtRunQueue",
        "compare": [],
        "skip": [],  # find_highest_prio is private; verify extracts pure bitmap logic
        "check_new": False,
    },
    {
        "name": "ipc/sysv_msg",
        "verify": "kernel/verify/src/ipc/sysv_msg_test.rs",
        "kernel": "kernel/src/ipc/sysv_msg.rs",
        "type": None,  # find_msg_match extracted (Msg simplified to mtype-only)
        "compare": [],
        "skip": [],
        "check_new": False,
    },
]


# ============================================================
# Check logic
# ============================================================

def check_mapping(m, verbose=False):
    """Check one mapping. Returns list of result tuples."""
    name = m["name"]
    vpath = ROOT / m["verify"]
    kpath = ROOT / m["kernel"]
    type_name = m.get("type", "")

    if not vpath.exists():
        return [("ERROR", f"verify file not found: {m['verify']}")]
    if not kpath.exists():
        return [("ERROR", f"kernel file not found: {m['kernel']}")]

    vtext = vpath.read_text()
    ktext = kpath.read_text()
    skip = set(m.get("skip", []))
    skip_diff = set(m.get("skip_diff", []))
    results = []

    # 1. Compare listed functions (scoped to impl type)
    for fn_name in m["compare"]:
        vfn = extract_fn(vtext, fn_name, impl_type=type_name)
        kfn = extract_fn(ktext, fn_name, impl_type=type_name)

        if vfn is None:
            results.append(("ERROR", f"'{fn_name}' not found in verify"))
            continue
        if kfn is None:
            results.append(("WARN", f"'{fn_name}' not found in kernel (removed?)"))
            continue

        vnorm = normalize(vfn)
        knorm = normalize(kfn)

        if vnorm == knorm:
            if verbose:
                results.append(("OK", fn_name))
        elif fn_name in skip_diff:
            if verbose:
                results.append(("SKIP", fn_name))
        else:
            # Find first differing line
            vlines = vnorm.split('\n')
            klines = knorm.split('\n')
            diff_line = 0
            for idx, (a, b) in enumerate(zip(vlines, klines)):
                if a != b:
                    diff_line = idx
                    break
            else:
                diff_line = min(len(vlines), len(klines))

            results.append(("DIFF", fn_name, diff_line, vlines, klines))

    # 2. Check for new pub methods in kernel not covered by verify
    if type_name and m.get("check_new", True):
        k_methods = set(extract_impl_pub_methods(ktext, type_name))
        v_methods = set(extract_impl_pub_methods(vtext, type_name))
        covered = set(m["compare"]) | skip

        for method in sorted(k_methods):
            if method not in v_methods and method not in covered:
                results.append(("NEW", f"kernel has new method '{method}' on {type_name} not in verify"))

    return results


def format_diff(fn_name, diff_line, vlines, klines):
    """Format a diff result for display."""
    ctx = 3
    start = max(0, diff_line - ctx)
    end = min(max(len(vlines), len(klines)), diff_line + ctx + 1)
    lines = []
    for i in range(start, end):
        vl = vlines[i] if i < len(vlines) else "<EOF>"
        kl = klines[i] if i < len(klines) else "<EOF>"
        marker = ">>>" if i == diff_line else "   "
        if vl == kl:
            lines.append(f"    {marker} {i + 1:3d}| {vl}")
        else:
            lines.append(f"    {marker} {i + 1:3d}- verify: {vl}")
            lines.append(f"    {marker} {i + 1:3d}+ kernel: {kl}")
    return '\n'.join(lines)


# ============================================================
# Main
# ============================================================

def main():
    args = sys.argv[1:]
    verbose = '-v' in args or '--verbose' in args
    filter_mod = None
    for arg in args:
        if not arg.startswith('-'):
            filter_mod = arg

    all_diffs = 0
    all_errors = 0

    for m in MAPPINGS:
        if filter_mod and not m["name"].startswith(filter_mod):
            continue

        print(f"Checking {m['name']}...")
        issues = check_mapping(m, verbose)

        for item in issues:
            severity = item[0]
            if severity == "OK":
                print(f"  [OK] {item[1]}")
            elif severity == "SKIP":
                print(f"  [SKIP] {item[1]} (known intentional diff)")
            elif severity == "DIFF":
                all_diffs += 1
                fn_name = item[1]
                diff_line = item[2]
                vlines = item[3]
                klines = item[4]
                print(f"  [DIFF] {fn_name} (first diff at line {diff_line + 1})")
                if verbose:
                    print(format_diff(fn_name, diff_line, vlines, klines))
            elif severity == "ERROR":
                all_errors += 1
                print(f"  [ERROR] {item[1]}")
            elif severity == "WARN":
                print(f"  [WARN] {item[1]}")
            elif severity == "NEW":
                print(f"  [NEW]  {item[1]}")

    print()
    if all_diffs == 0 and all_errors == 0:
        if filter_mod:
            print(f"Module '{filter_mod}' is in sync.")
        else:
            print("All verify copies are in sync with kernel source.")
        return 0
    else:
        parts = []
        if all_diffs:
            parts.append(f"{all_diffs} divergence(s)")
        if all_errors:
            parts.append(f"{all_errors} error(s)")
        print(f"Found {', '.join(parts)}. Review and update verify copies.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
