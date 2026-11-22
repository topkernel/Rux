//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 file operations

use crate::errno;
use crate::fs::bio;
use crate::fs::ext4::indirect;
use crate::fs::file::{File, FileOps};
use crate::fs::inode::Inode;
use crate::fs::io_completion::IoCompletion;
use crate::fs::page_cache;
use crate::fs::readahead::ReadAheadState;

// ============================================================================
// sc105: demand-fault read attribution counters
// ============================================================================
//
// The OH launcher child burns its whole 3 s life in kernel time with the
// prime suspect being per-fault synchronous ext4 fills (no fault-around).
// Count page-shaped positional reads (the mmap demand-fault signature),
// their total service time, and cache-miss fills — KILLDUMP prints these
// alongside utime/stime so the burn is attributed, not guessed.

static FAULT_READS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FAULT_MISSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FAULT_CYCLES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Incremented in the miss branch of ext4_file_read_cached_dst (all fills).
pub(crate) static FAULT_FILL_MISSES: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Snapshot (reads, misses, cycles) for KILLDUMP.
pub fn demand_read_stats() -> (u64, u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (
        FAULT_READS.load(Relaxed),
        FAULT_MISSES.load(Relaxed),
        FAULT_CYCLES.load(Relaxed),
        FAULT_FILL_MISSES.load(Relaxed),
    )
}

pub fn ext4_file_read(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &mut [u8],
) -> Result<usize, i32> {
    let file_size = inode.get_size();

    if offset >= file_size {
        return Ok(0);  // EOF
    }

    let available = file_size - offset;
    let to_read = core::cmp::min(buf.len() as u64, available) as usize;

    let block_size = fs.block_size as usize;

    // BATCHED O_DIRECT (paired-test item 1, 2026-11-17): the per-block
    // body below served every 4 KiB block with its own synchronous
    // bread round trip — a 1 MiB direct read was 256 serialized device
    // waits (226x off the reference's single 1 MiB request). Resolve the
    // request's blocks into physically-consecutive runs, submit each run
    // as ONE multi-block request (bio::bread_async_multi — the batching
    // the page-cache fill has used since the readahead window), keep a
    // window of runs in flight, and wait ONCE per window: requests
    // behind the first complete while the first is waited on. Fall back
    // to the per-block body when the fs block size differs from the
    // block cache's fixed 4096 (multi-block sector math is cache-block
    // based) or per-run on any async failure.
    if block_size == bio::get_block_cache().cache_block_size() as usize && inode.has_extent() {
        let mut dst = DioReadDst::Staging(buf);
        if let Some(n) = ext4_dio_read_batched(fs, inode, offset, &mut dst, to_read, block_size) {
            return Ok(n);
        }
    }

    ext4_file_read_perblock(fs, inode, offset, buf, to_read, block_size)
}

/// Destination of a batched O_DIRECT read (GUP-DIO, 2026-11-18): the
/// syscall layer's staging slice, or the caller's PINNED user frames
/// (the reference kernel's bio_iov_iter_get_pages shape — the device
/// DMAs straight into the user pages, no staging copy).
enum DioReadDst<'a> {
    /// Physically-contiguous kernel staging (buddy-backed), alive for
    /// the whole call.
    Staging(&'a mut [u8]),
    /// Pinned user frames backing `[va, va+len)`. DMA runs build their
    /// SG segments from the frames' direct-map aliases; per-block
    /// serves and hole fills copy through the USER mapping
    /// (`copy_to_user` — the same exception-table discipline as the
    /// buffered fast path), so unpinned pages (pin holes) keep the
    /// staged-path semantics byte-for-byte.
    User {
        pins: &'a crate::mm::gup::PinnedUserPages,
        va: usize,
    },
}

impl<'a> DioReadDst<'a> {
    /// Serve `src` (a whole block's window from bread or a hole's
    /// zero block) at request offset `off`. Returns bytes delivered;
    /// None on a user fault before ANY byte (the ReadDst::User put
    /// contract: first-byte fault = EFAULT-class failure).
    fn put(&mut self, off: usize, src: &[u8]) -> Option<usize> {
        match self {
            DioReadDst::Staging(buf) => {
                debug_assert!(off + src.len() <= buf.len());
                (**buf)[off..off + src.len()].copy_from_slice(src);
                Some(src.len())
            }
            DioReadDst::User { va, .. } => {
                // SAFETY: exception-table copy into the access_ok'd
                // user window; uncopied tail truncates (short read).
                let uncopied = crate::arch::uaccess_safe::copy_out(
                    (*va + off) as usize,
                    src,
                );
                if uncopied >= src.len() {
                    None
                } else {
                    Some(src.len() - uncopied)
                }
            }
        }
    }

    /// Zero-fill `n` bytes at request offset `off` (holes / unwritten).
    /// Same failure contract as `put`.
    fn put_zeroes(&mut self, off: usize, n: usize) -> Option<usize> {
        static ZEROS: [u8; 4096] = [0u8; 4096];
        let mut done = 0usize;
        while done < n {
            let k = core::cmp::min(4096, n - done);
            match self.put(off + done, &ZEROS[..k]) {
                Some(c) if c == k => done += c,
                Some(c) => return Some(done + c),
                None => return if done == 0 { None } else { Some(done) },
            }
        }
        Some(done)
    }

    /// True when the whole `[off, off+len)` span can be DMA'd (staging
    /// is always contiguous; user frames must all be pinned).
    fn span_ok(&self, off: usize, len: usize) -> bool {
        match self {
            DioReadDst::Staging(_) => true,
            DioReadDst::User { pins, .. } => pins.range_pinned(off, len),
        }
    }

    /// First byte pointer of the span at `off` — the completion token
    /// (the zero-serve guard expects the chain's first segment's
    /// pointer).
    fn first_ptr(&mut self, off: usize) -> *mut u8 {
        match self {
            DioReadDst::Staging(buf) => {
                // off <= buf.len() (callers pass in-range dst
                // offsets); the pointer is used within this borrow
                // (wrapping: the edge's in-range contract).
                (**buf).as_mut_ptr().wrapping_add(off)
            }
            DioReadDst::User { pins, .. } => pins.first_ptr(off) as *mut u8,
        }
    }
}

/// Batched O_DIRECT read body (paired-test item 1, 2026-11-17; GUP-DIO
/// destination parameterization 2026-11-18).
///
/// DMAs straight into the CALLER's buffer: the syscall layer's staging
/// slice (buddy-backed, physically contiguous), alive for the whole
/// call — or, in the GUP arm, the caller's PINNED user frames (SG
/// segments built from the frames, the reference's zero-copy shape).
/// Every request drains before this returns, so the destination is
/// never a live DMA target after return. Whole 4 KiB-aligned runs of
/// physically-consecutive, block-cache-absent blocks ride ONE
/// multi-block device request each (a 1 MiB sequential direct read is
/// four 256 KiB requests in flight, waited once); head/tail partial
/// blocks, holes, pin-hole spans and any async failure fall back to
/// per-block bread serves. Returns None when the device has no async
/// read path at all (first submission fails) — the caller re-runs the
/// per-block body.
fn ext4_dio_read_batched(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    dst: &mut DioReadDst<'_>,
    to_read: usize,
    block_size: usize,
) -> Option<usize> {
    use crate::fs::io_completion::IoCompletion;

    const MAX_BATCH: usize = 64;
    const MAX_BLOCKS_PER_WINDOW: u64 = 512;
    const MAX_MERGE_BLOCKS: u64 = 256;
    let sectors_per_block = (block_size / 512) as u64;

    let mut completions: [IoCompletion; MAX_BATCH] = [const { IoCompletion::new() }; MAX_BATCH];
    // Per in-flight request: (dst offset in buf, file offset, bytes).
    let mut req_dst = [0usize; MAX_BATCH];
    let mut req_off = [0u64; MAX_BATCH];
    let mut req_len = [0usize; MAX_BATCH];
    // Device-side start sector of each run (the fusion key).
    let mut req_sector = [0u64; MAX_BATCH];
    let mut count = 0usize;
    let mut blocks_in_window: u64 = 0;

    let mut total_read = 0usize;
    let mut current_offset = offset;
    let mut buf_offset = 0usize;

    // Serve exactly one block's window (from `file_off`, `dst_off`)
    // through the synchronous per-block path: hole -> zeroes, else
    // bread + copy. Returns bytes served, or None on I/O error.
    fn serve_one_block(
        fs: &crate::fs::ext4::Ext4FileSystem,
        inode: &crate::fs::ext4::inode::Ext4Inode,
        file_off: u64,
        dst_off: usize,
        dst: &mut DioReadDst<'_>,
        take: usize,
        block_size: usize,
    ) -> Option<usize> {
        let block_index = file_off / block_size as u64;
        let block_offset = (file_off % block_size as u64) as usize;
        let phys = match inode.get_data_block(fs, block_index) {
            Ok(b) => b,
            Err(_) => return None,
        };
        let avail = block_size - block_offset;
        let len = core::cmp::min(take, avail);
        if phys == 0 {
            crate::fs::ext4::extent::exthunt_read_hole(
                inode.ino,
                &inode.block,
                block_index,
            );
            return match dst.put_zeroes(dst_off, len) {
                Some(n) => Some(n),
                None => None,
            };
        }
        // INV-BIO-BHREF: bread pins the BufferHead until brelse; the
        // data view rides the safe accessor.
        {
            let bh = match bio::bread(fs.device, phys) {
                Some(b) => b,
                None => return None,
            };
            let data = bio::bh_data(&bh);
            crate::fs::ext4::extent::exthunt_rd_od(
                inode.ino as u64,
                block_index,
                phys,
                data[0],
                data[1],
                data.as_ptr(),
            );
            let served = dst.put(dst_off, &data[block_offset..block_offset + len]);
            bio::brelse(bh);
            served
        }
    }

    // Drain the window (FUSED, the blk-merge front/back-merge shape):
    // runs were only RECORDED until now — the plug list. Phase 1 groups
    // device-adjacent runs (sector-contiguous AND destination-contiguous,
    // two integer compares — the reference's back-merge test) and submits
    // ONE scatter-gather request per group (per-run chains as fallback);
    // in the GUP arm each run's segments are the PINNED user frames
    // (one per page piece, chains capped at MAX_SEGS). Phase 2 kicks
    // once and waits in order. A failed request is re-served per-block
    // synchronously (a second failure cuts the read short at that
    // request's start — POSIX short read).
    macro_rules! drain {
        () => {{
            if count > 0 {
                const MAX_SEGS: usize =
                    crate::drivers::virtio::MAX_SG_SEGS_PER_CHAIN;
                // GUP packing cap: bounded stack segment array (2 KiB),
                // clipped by the negotiated seg_max at drain time.
                const GUP_SEG_CAP: usize = 128;
                // Flat chain list: chain c covers [chain_dst[c],
                // +chain_len[c]) of the request inside run chain_run[c];
                // chain_armed[c] = false marks a range that could NOT be
                // submitted (SG refused / pin hole / table full) — the
                // settle phase re-serves it per-block instead of waiting.
                let mut chain_run = [0usize; MAX_BATCH];
                let mut chain_dst = [0usize; MAX_BATCH];
                let mut chain_len = [0usize; MAX_BATCH];
                let mut chain_armed = [false; MAX_BATCH];
                let mut chains = 0usize;
                let mut cut_at = usize::MAX;

                let mut segs: [crate::drivers::blkdev::DmaSeg; MAX_SEGS] =
                    [crate::drivers::blkdev::DmaSeg {
                        ptr: core::ptr::null_mut(),
                        len: 0,
                    }; MAX_SEGS];
                let mut gup_segs: [crate::drivers::blkdev::DmaSeg; GUP_SEG_CAP] =
                    [crate::drivers::blkdev::DmaSeg {
                        ptr: core::ptr::null_mut(),
                        len: 0,
                    }; GUP_SEG_CAP];
                // GUP chain geometry from the negotiated ring depth.
                let gup_slot = (*fs.device).private_data
                    .map(|p| p as usize)
                    .unwrap_or(0);
                let (seg_max, desc_budget) =
                    crate::drivers::virtio::pci_blk_sg_limits(gup_slot);
                // Two full-cap chains fit one wave by construction.
                let pack_cap = seg_max.min(GUP_SEG_CAP).min(desc_budget / 2);
                let mut window_descs = 0usize;

                // ---- Phase 1: build + submit chains quietly ----
                // Staging: fuse device+dst-adjacent runs into ONE chain
                // of <= MAX_SEGS segments (the pre-GUP behavior,
                // preserved); on SG refusal each run takes its own plain
                // async read (also the pre-GUP fallback). GUP: chains of
                // the run's PINNED page pieces (up to pack_cap segments,
                // physically-adjacent pages pre-merged by the pin), never
                // spanning runs. When the descriptor budget binds, the
                // packing STOPS: Phase 2 settles what was submitted and
                // resets the window; the caller's main loop re-records
                // the un-served runs into the next window (the ring
                // depth — not this drain — is the real wave bound; the
                // first cut armed only ~2/3 of the chains and serialized
                // the rest through per-block breads, the 941 MiB/s
                // regression shape). A run with a pin hole goes un-armed
                // — per-block re-serve.
                let mut i = 0usize;
                'pack: while i < count {
                    let staging_fuses = matches!(dst, DioReadDst::Staging(_));
                    let mut e = i + 1;
                    if staging_fuses {
                        while e < count
                            && (e - i) < MAX_SEGS
                            && req_sector[e - 1] + (req_len[e - 1] / 512) as u64
                                == req_sector[e]
                            && req_dst[e - 1] + req_len[e - 1] == req_dst[e]
                        {
                            e += 1;
                        }
                    }

                    if staging_fuses {
                        // One chain over the fused group.
                        let k = e - i;
                        let group_bytes: usize = (i..e).map(|r| req_len[r]).sum();
                        for (j, sg) in segs.iter_mut().take(k).enumerate() {
                            sg.len = req_len[i + j];
                        }
                        {
                            let base = match dst {
                                DioReadDst::Staging(buf) => {
                                    (**buf)[req_dst[i]..].as_mut_ptr()
                                }
                                _ => unreachable!(),
                            };
                            let mut p = base as usize;
                            for sg in segs.iter_mut().take(k) {
                                sg.ptr = p as *mut u8;
                                p += sg.len;
                            }
                        }
                        let armed = chains < MAX_BATCH
                            && crate::drivers::blkdev::blkdev_read_async_sg(
                                fs.device,
                                req_sector[i],
                                &segs[..k],
                                &completions[chains],
                            )
                            .is_ok();
                        if armed {
                            chain_run[chains] = i;
                            chain_dst[chains] = req_dst[i];
                            chain_len[chains] = group_bytes;
                            chain_armed[chains] = true;
                            chains += 1;
                        } else {
                            // SG refused (window lag / no SG): one plain
                            // chain per run — the pre-GUP fallback. A run
                            // whose own submit also fails stays un-armed
                            // (per-block re-serve — the standing fallback).
                            for r in i..e {
                                if chains >= MAX_BATCH {
                                    break;
                                }
                                // INV-EXT4-REQRUN: recorded runs partition
                                // [0, to_read) — each span is clamped to the
                                // request's remaining full blocks at record
                                // time (n <= remaining / block_size), so
                                // req_dst[r] + req_len[r] <= to_read <= the
                                // staging buffer's length. The bounds-checked
                                // index below therefore never fires.
                                let slice = match dst {
                                    DioReadDst::Staging(buf) => {
                                        &mut (**buf)[req_dst[r]..req_dst[r] + req_len[r]]
                                    }
                                    _ => unreachable!(),
                                };
                                let ok = crate::drivers::blkdev::blkdev_read_async(
                                    fs.device,
                                    req_sector[r],
                                    slice,
                                    &completions[chains],
                                )
                                .is_ok();
                                chain_run[chains] = r;
                                chain_dst[chains] = req_dst[r];
                                chain_len[chains] = req_len[r];
                                chain_armed[chains] = ok;
                                chains += 1;
                                if !ok {
                                    break; // device out of async slots: stop arming
                                }
                            }
                        }
                    } else {
                        // GUP arm: chains of pinned pieces per run.
                        for r in i..e {
                            if !dst.span_ok(req_dst[r], req_len[r]) {
                                // Pin hole: record the whole run un-armed.
                                if chains < MAX_BATCH {
                                    chain_run[chains] = r;
                                    chain_dst[chains] = req_dst[r];
                                    chain_len[chains] = req_len[r];
                                    chain_armed[chains] = false;
                                    chains += 1;
                                }
                                continue;
                            }
                            let pins = match dst {
                                DioReadDst::User { pins, .. } => *pins,
                                _ => unreachable!(),
                            };
                            let run_end = req_dst[r] + req_len[r];
                            let mut o = req_dst[r];
                            let mut table_full = false;
                            while o < run_end && !table_full {
                                let chain_dst_o = o;
                                let mut k = 0usize;
                                let mut bytes = 0usize;
                                while k < pack_cap {
                                    match pins.next_piece(&mut o, run_end) {
                                        Some((pp, l)) => {
                                            gup_segs[k].ptr = pp;
                                            gup_segs[k].len = l;
                                            k += 1;
                                            bytes += l;
                                        }
                                        None => break,
                                    }
                                }
                                if bytes == 0 {
                                    break; // no pinned piece left (pin hole tail)
                                }
                                // Descriptor-budget wave gate (actual
                                // footprint: merged pieces shrink chains).
                                if chains > 0
                                    && window_descs + k + 2 > desc_budget
                                {
                                    // Settle the live chains and hand the
                                    // remainder back to the main loop
                                    // (re-recorded into the next window).
                                    break 'pack;
                                }
                                let base_sector = req_sector[r]
                                    + ((chain_dst_o - req_dst[r]) / 512) as u64;
                                let armed = chains < MAX_BATCH
                                    && crate::drivers::blkdev::blkdev_read_async_sg(
                                        fs.device,
                                        base_sector,
                                        &gup_segs[..k],
                                        &completions[chains],
                                    )
                                    .is_ok();
                                if chains < MAX_BATCH {
                                    chain_run[chains] = r;
                                    chain_dst[chains] = chain_dst_o;
                                    chain_len[chains] = bytes;
                                    chain_armed[chains] = armed;
                                    if armed {
                                        window_descs += k + 2;
                                    }
                                    chains += 1;
                                    table_full = chains == MAX_BATCH;
                                } else {
                                    table_full = true;
                                }
                            }
                            if table_full {
                                crate::pr_err!(
                                    "ext4 dio gup: chain table full at dst {}",
                                    o
                                );
                                break 'pack;
                            }
                        }
                    }
                    i = e;
                }

                // ---- Phase 2: one kick, then wait in order ----
                crate::drivers::blkdev::kick_all_pending_disks();
                for c in 0..chains {
                    let mut ok = false;
                    if chain_armed[c] {
                        let rc = completions[c].wait();
                        if rc == 0 {
                            // Zero-serve/attribution guard against the
                            // chain's first segment pointer.
                            let token = completions[c].delivered_token();
                            let expect = dst.first_ptr(chain_dst[c]) as usize;
                            if token == 0 || token == expect {
                                ok = true;
                            } else {
                                crate::pr_err!(
                                    "ext4 dio: misattributed read completion (token {:x} != {:x})",
                                    token, expect
                                );
                            }
                        }
                    }
                    if ok {
                        let d = chain_dst[c];
                        let l = chain_len[c];
                        if d + l > total_read {
                            total_read = d + l;
                        }
                    } else {
                        // Per-block re-serve of the failed chain's range.
                        let mut off = req_off[chain_run[c]]
                            + (chain_dst[c] - req_dst[chain_run[c]]) as u64;
                        let mut d = chain_dst[c];
                        let mut left = chain_len[c];
                        while left > 0 {
                            match serve_one_block(fs, inode, off, d, dst, left, block_size) {
                                Some(n) => {
                                    off += n as u64;
                                    d += n;
                                    left -= n;
                                }
                                None => {
                                    cut_at = cut_at.min(d);
                                    break;
                                }
                            }
                        }
                        if left == 0 && d > total_read {
                            total_read = d;
                        } else if left > 0 {
                            cut_at = cut_at.min(d);
                        }
                    }
                    completions[c].reset();
                }
                count = 0;
                blocks_in_window = 0;
                if cut_at != usize::MAX && cut_at < total_read {
                    total_read = cut_at;
                }
                buf_offset = total_read;
                current_offset = offset + total_read as u64;
            }
        }};
    }

    // Outer resume loop (GUP-DIO): the drain's descriptor-budget wave
    // may settle a window with un-served runs left (the ring depth is
    // the wave bound); this loop re-records them into the next window
    // instead of surfacing a short read.
    loop {
    while buf_offset < to_read {
        let block_offset = (current_offset % block_size as u64) as usize;
        let remaining = to_read - buf_offset;

        // Head/tail partial block: per-block serve (DMA must stay
        // sector-aligned inside the destination).
        if block_offset != 0 || remaining < block_size {
            match serve_one_block(fs, inode, current_offset, buf_offset, dst, remaining, block_size)
            {
                Some(n) => {
                    total_read += n;
                    buf_offset += n;
                    current_offset += n as u64;
                    continue;
                }
                None => {
                    drain!();
                    return Some(total_read.min(buf_offset));
                }
            }
        }

        let block_index = current_offset / block_size as u64;
        let full_blocks_available = remaining / block_size;

        // ONE extent walk resolves the maximal run (paired-test item 1):
        // per-block get_data_block paid 1-2 cached breads per 4 KiB on
        // depth>=1 files (512 per 1 MiB read — the fsw br=25k signature).
        let (run_phys, run_avail, run_unwritten) = match crate::fs::ext4::extent::ext4_ext_get_run(
            fs,
            inode.ino,
            &inode.block,
            block_index,
        ) {
            Ok(v) => v,
            Err(_) => {
                drain!();
                if total_read > 0 {
                    return Some(total_read);
                }
                return None;
            }
        };

        // Unwritten extent or hole: zero-serve the known stretch (the
        // resolver reports the distance to the next extent; 0 = unknown
        // ahead — one block at a time).
        if run_phys == 0 {
            let zero_blocks = if run_unwritten {
                run_avail.max(1)
            } else {
                run_avail
            };
            let zero_len = if zero_blocks > 0 {
                core::cmp::min(remaining, zero_blocks as usize * block_size)
            } else {
                core::cmp::min(remaining, block_size)
            };
            crate::fs::ext4::extent::exthunt_read_hole(
                inode.ino,
                &inode.block,
                block_index,
            );
            match dst.put_zeroes(buf_offset, zero_len) {
                Some(n) => {
                    total_read += n;
                    buf_offset += n;
                    current_offset += n as u64;
                }
                None => {
                    drain!();
                    return Some(total_read);
                }
            }
            continue;
        }

        // Trim the run: request bounds, window budget, merge cap, and the
        // conservative block-cache split (a cached member may hold dirty
        // bytes the disk does not have).
        let mut n: u64 = core::cmp::min(
            core::cmp::min(run_avail, full_blocks_available as u64),
            MAX_MERGE_BLOCKS,
        );
        if blocks_in_window + n > MAX_BLOCKS_PER_WINDOW {
            n = MAX_BLOCKS_PER_WINDOW.saturating_sub(blocks_in_window).max(1);
        }
        if n > 1 {
            let mut k: u64 = 1;
            while k < n {
                if bio::block_cached(fs.device, run_phys + k) {
                    break;
                }
                k += 1;
            }
            n = k;
        }
        // A single block that the cache holds: per-block serve (bread
        // serves the cached, possibly dirty, truth).
        if n == 1 && bio::block_cached(fs.device, run_phys) {
            match serve_one_block(fs, inode, current_offset, buf_offset, dst, remaining, block_size)
            {
                Some(n2) => {
                    total_read += n2;
                    buf_offset += n2;
                    current_offset += n2 as u64;
                    continue;
                }
                None => {
                    drain!();
                    return Some(total_read.min(buf_offset));
                }
            }
        }

        if count == MAX_BATCH || blocks_in_window + n > MAX_BLOCKS_PER_WINDOW {
            drain!();
        }

        let span_bytes = n as usize * block_size;
        // RECORD the run only — the plug list. Submission (fused with any
        // device-adjacent neighbors) happens at the window drain; the
        // device sees no work before the drain's kick either way (chains
        // publish quietly), so deferral costs nothing and lets the drain
        // fuse this run with its predecessor when both are contiguous.
        crate::fs::ext4::extent::exthunt_rd_batch(
            inode.ino as u64,
            block_index,
            run_phys,
            n as u32,
        );
        req_dst[count] = buf_offset;
        req_off[count] = current_offset;
        req_len[count] = span_bytes;
        req_sector[count] = run_phys * sectors_per_block;
        count += 1;
        blocks_in_window += n;
        buf_offset += span_bytes;
        current_offset += span_bytes as u64;
    }

    drain!();
    if buf_offset >= to_read {
        break;
    }
    }
    Some(total_read)
}

/// The pre-batching per-block O_DIRECT read body (2026-11-15 dio-data
/// shape): one extent lookup + one synchronous bread round trip per 4 KiB
/// block. Kept as the fallback for fs block sizes the multi-block path
/// cannot express (block-cache block size is fixed 4096) and for devices
/// without an async read path.
fn ext4_file_read_perblock(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &mut [u8],
    to_read: usize,
    block_size: usize,
) -> Result<usize, i32> {
    let file_size = inode.get_size();

    let mut total_read = 0;
    let mut current_offset = offset as usize;
    let mut buf_offset = 0;

    while total_read < to_read {
        let block_index = current_offset / block_size;
        let block_offset = current_offset % block_size;

        if block_index * block_size >= file_size as usize {
            break;
        }

        let this_block = inode.get_data_block(fs, block_index as u64)?;

        // Sparse hole (block 0 = unallocated): serve ZEROES, never physical
        // block 0 (superblock-backup garbage) — review 5.5 (稀疏洞读盘 0,
        // 非缓存路径).
        if this_block == 0 {
            // rux:exthunt (retired dfx=exthunt): O_DIRECT hole verdict — see exthunt_read_hole.
            crate::fs::ext4::extent::exthunt_read_hole(
                inode.ino,
                &inode.block,
                block_index as u64,
            );
            let remaining = to_read - total_read;
            let available_in_block = block_size - block_offset;
            let zero_len = core::cmp::min(remaining, available_in_block);
            for i in 0..zero_len {
                buf[buf_offset + i] = 0;
            }
            total_read += zero_len;
            buf_offset += zero_len;
            current_offset += zero_len;
            continue;
        }

        // INV-BIO-BHREF: bread pins the BufferHead until brelse; the
        // data view rides the safe accessor (block-sized, aligned).
        {
            let bh = bio::bread(fs.device, this_block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = bio::bh_data(&bh);
            // rux:exthunt (retired dfx=exthunt): O_DIRECT serve truth (see exthunt_rd_od).
            crate::fs::ext4::extent::exthunt_rd_od(
                inode.ino as u64,
                block_index as u64,
                this_block,
                data[0],
                data[1],
                data.as_ptr(),
            );
            let remaining = to_read - total_read;
            let available_in_block = block_size - block_offset;
            let read_in_block = core::cmp::min(remaining, available_in_block);

            buf[buf_offset..buf_offset + read_in_block]
                .copy_from_slice(&data[block_offset..block_offset + read_in_block]);

            total_read += read_in_block;
            buf_offset += read_in_block;
            current_offset += read_in_block;

            bio::brelse(bh);
        }
    }

    Ok(total_read)
}

/// Destination of a file read: kernel slice (FileOps path) or user memory
/// (read(2) fast path — skips the syscall layer's staging buffer and its
/// extra 4 KiB alloc+copy per call).
pub enum ReadDst {
    /// Kernel buffer pointer + length.
    Kernel(*mut u8, usize),
    /// access_ok-validated user pointer + length.
    User(*mut u8, usize),
    /// Throw-away destination (mm filemap_fault cache warm-up): the read
    /// runs for its SIDE EFFECT — populating the page cache through the
    /// readahead/fault-around machinery — and the bytes are never copied.
    /// The caller pins the frames with `page_cache::get_phys` afterwards.
    Discard(usize),
}

impl ReadDst {
    #[inline]
    fn len(&self) -> usize {
        match self {
            ReadDst::Kernel(_, n) | ReadDst::User(_, n) => *n,
            ReadDst::Discard(n) => *n,
        }
    }

    /// Copy `bytes` from a cached page into this destination at `off`.
    /// Returns bytes actually copied; `Err(EFAULT)` when a USER destination
    /// faulted before ANY byte of the request could be delivered (Linux
    /// semantics: a fault at the first byte is EFAULT, a fault mid-copy is
    /// a short read).
    ///
    /// Contract (INV-EXT4-DIO-1): the Kernel variant pointer must have
    /// `off + src.len()` writable bytes, the User variant is
    /// access_ok-validated; `src` proves its own bytes.
    fn put(&self, off: usize, src: &[u8]) -> Result<usize, i32> {
        let bytes = src.len();
        match self {
            ReadDst::Kernel(dst, _) => {
                // SAFETY: INV-EXT4-DIO-1 — dst+off+bytes is in bounds
                // (the staging buffer's caller-checked span).
                unsafe {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), dst.add(off), bytes)
                };
                Ok(bytes)
            }
            ReadDst::Discard(_) => Ok(bytes), // warm-up read: data lands in the page cache
            ReadDst::User(dst, _) => {
                // Exception-table copy; uncopied bytes truncate.
                let uncopied = crate::arch::uaccess_safe::copy_out(
                    (*dst as *const u8 as usize) + off,
                    src,
                );
                if uncopied >= bytes {
                    // Nothing delivered: the page is unmapped — report the
                    // fault instead of a silent 0-byte "EOF" (the r8
                    // read(2) fast path turned read() on a bad buffer into
                    // an apparent success; LTP read02/readv02 EFAULT).
                    return Err(errno::Errno::BadAddress.as_neg_i32());
                }
                Ok(bytes - uncopied)
            }
        }
    }

    /// Zero-fill `bytes` at `off` (sparse holes). Returns bytes filled;
    /// `Err(EFAULT)` on an immediate full user fault.
    ///
    /// Destination validity contract as `put` (INV-EXT4-DIO-1).
    fn put_zeroes(&self, off: usize, bytes: usize) -> Result<usize, i32> {
        // The zero page: ext4 block_size-sized static is overkill — copy in
        // 4 KiB chunks from a small const zero block.
        static ZEROS: [u8; 4096] = [0u8; 4096];
        let mut done = 0usize;
        while done < bytes {
            let n = core::cmp::min(4096, bytes - done);
            let c = self.put(off + done, &ZEROS[..n])?;
            if c < n {
                break; // user fault: short read
            }
            done += c;
        }
        Ok(done)
    }
}

/// Source of a buffered write (BUFW-2X): a kernel slice (the FileOps path
/// — the syscall layer's staging buffer) or USER memory (the write(2)
/// fast path: bytes go user→page-cache-frame in ONE copy, the reference
/// kernel's copy_page_from_iter shape — the staging buffer and its second
/// copy are skipped).
#[derive(Clone, Copy)]
pub enum WriteSrc {
    /// Kernel buffer pointer + length.
    Kernel(*const u8, usize),
    /// access_ok-validated user pointer + length.
    User(*const u8, usize),
}

impl WriteSrc {
    #[inline]
    fn len(&self) -> usize {
        match self {
            WriteSrc::Kernel(_, n) | WriteSrc::User(_, n) => *n,
        }
    }

    /// Byte view of the KERNEL source at `[off, off + n)`.
    ///
    /// INV-EXT4-WRITESRC-K: every `WriteSrc::Kernel` is built from a real
    /// slice's `(as_ptr(), len())` (the three construction sites: the
    /// FileOps staging buffer, the write_at boundary edge, the staged
    /// fallback), so the whole `len()`-byte span is readable for the
    /// source value's life. `off + n <= self.len()` is the caller's
    /// bound (each caller derives `n` from the remaining source length).
    /// `None` for a User source (those copy through the exception table).
    #[inline]
    fn kernel_bytes(&self, off: usize, n: usize) -> Option<&[u8]> {
        debug_assert!(off.checked_add(n).is_some_and(|end| end <= self.len()));
        match self {
            // SAFETY: INV-EXT4-WRITESRC-K — a real slice backs the span.
            WriteSrc::Kernel(p, _) => Some(unsafe { core::slice::from_raw_parts(p.add(off), n) }),
            WriteSrc::User(..) => None,
        }
    }

    /// Clip the source to `n` bytes (RLIMIT_FSIZE truncation).
    #[inline]
    fn truncate(&self, n: usize) -> WriteSrc {
        match *self {
            WriteSrc::Kernel(p, _) => WriteSrc::Kernel(p, n),
            WriteSrc::User(p, _) => WriteSrc::User(p, n),
        }
    }

    /// Copy `n` bytes at source offset `off` straight into a page-cache
    /// frame — the ONE user→frame copy on the fast path. Returns the
    /// uncopied tail (0 = complete; >0 = user fault, and the frame holds
    /// a valid prefix of the bytes at `dst` plus zero-fill beyond, per
    /// the uaccess primitive's contract).
    ///
    /// # Safety
    /// `dst` must be a valid writable kernel destination for `n` bytes
    /// (a page-cache frame span). Never call this with a spinlock held:
    /// the User arm's fault path can sleep (mapped-but-not-resident user
    /// pages are demand-filled by the trap handler).
    fn copy_to_frame(&self, off: usize, dst: &mut [u8]) -> usize {
        match self {
            // INV-EXT4-WRITESRC-K: the kernel source span `off..off+n`.
            WriteSrc::Kernel(..) => {
                dst.copy_from_slice(self.kernel_bytes(off, dst.len()).expect("Kernel source"));
                0
            }
            // Exception-table copy from user memory — any address is
            // sound to pass; uncopied bytes are returned (and the frame
            // holds a valid prefix, per the uaccess primitive's contract).
            WriteSrc::User(p, _) => {
                crate::arch::uaccess_safe::copy_in(dst, (*p as usize) + off)
            }
        }
    }
}

/// Read file data with page cache and batched async I/O.
///
/// Uses `get_data_block(index)` for single-block resolution (instead of
/// resolving the entire block map) and caches pages in the global page cache.
///
/// Performance design: a cache miss no longer pays one synchronous virtio
/// round trip per 4 KiB page. The miss triggers a BATCH fill — all missing
/// pages of the current request plus a read-ahead window (when the access
/// looks sequential) are submitted back-to-back with `bio::bread_async` and
/// completed with ONE sleep/wake cycle, then served from the page cache.
/// Before this, every page miss was a synchronous submit/sleep/wake
/// (~1.8 ms under TCG) and the old read-ahead never fired at all on the PCI
/// root disk (no async_read_fn registered — fixed in the virtio driver).
fn ext4_file_read_cached_dst(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    dst: ReadDst,
    ra_state: &mut ReadAheadState,
) -> Result<usize, i32> {
    let file_size = inode.get_size();
    if offset >= file_size {
        return Ok(0);
    }

    let available = file_size - offset;
    let to_read = core::cmp::min(dst.len() as u64, available) as usize;
    let block_size = fs.block_size as u64;
    let block_size_usize = fs.block_size as usize;
    let cache = page_cache::get_page_cache();
    let ino = inode.ino as u64;
    // Filesystem identity for the page-cache key (review 5.3: cross-FS key
    // collisions on small inode numbers).
    let fs_id = fs as *const crate::fs::ext4::Ext4FileSystem as u64;
    let file_pages = (file_size + block_size - 1) / block_size;
    // Pages the request itself touches.
    let demand_pages = ((to_read as u64 + block_size - 1) / block_size) as u64;

    let mut total_read = 0;
    let mut current_offset = offset;
    let mut buf_offset = 0;

    while total_read < to_read {
        let page_index = current_offset / block_size;
        let page_offset = (current_offset % block_size) as usize;
        let remaining = to_read - total_read;
        // Consecutive pages this request still needs from this page on.
        let pages_wanted = remaining.div_ceil(block_size_usize);

        // Serve a run of cached pages with ONE pin round trip. Fixed-size
        // stack array: a heap-allocated run here measurably regressed the
        // 4 KiB-read hot path under TCG.
        let mut run: [(u64, *const u8); 16] = [(0, core::ptr::null()); 16];
        let want = core::cmp::min(pages_wanted, run.len());
        // rux:filerd (retired dfx=filerd): phase anatomy of the warm path (bw_file_rd 50x
        // attribution). The probe's copy bracket is skipped on the
        // user-fault early returns below — a diagnostic undercount on
        // the EFAULT path only, never on the benchmark's fast path.
        let probe = crate::dfx::filerd::on();
        let t_pin = if probe { crate::dfx::filerd::now() } else { 0 };
        // oc-rc: claim the entry ONCE for the pin AND the unpin — the
        // second bank-claim + probe + strong-count pair the old
        // put_range re-paid per run was ~half the pin+unpin cost of a
        // 1-page hit. The entry Arc keeps the pins' map alive across
        // the copy window; the release paths below all hand it back.
        let (run_len, entry) =
            cache.get_range_with_entry(fs_id, ino, page_index, &mut run[..want]);
        if probe {
            crate::dfx::filerd::charge(&crate::dfx::filerd::PIN_CYCLES, t_pin);
        }
        if run_len > 0 {
            let t_copy = if probe { crate::dfx::filerd::now() } else { 0 };
            // Copy the run in MERGED groups: physically-contiguous pinned
            // pages (direct-map neighbors, run[j].1 == run[j-1].1 + block)
            // are served by ONE dst.put — one copy_to_user (one AC bracket,
            // one rep-movsb startup) instead of one per 4 KiB page. The
            // bw_file_rd anatomy probe showed the per-page copies were 81%
            // of the warm read(2) kernel time; batch-filled runs (buddy
            // allocations) are largely contiguous, so the common 16-page
            // sequential read collapses to a few copies. Byte-for-byte
            // identical to the per-page walk: a group spans [src, src+span)
            // contiguously and is truncated by the same `remaining` cap.
            let mut i = 0usize;
            while i < run_len {
                let in_page_off = if i == 0 { page_offset } else { 0 };
                let remaining_now = to_read - total_read;
                // rux:exthunt (retired dfx=exthunt): frame-hit truth for the run's first page
                // (a poisoned frame shows zeros here) — see exthunt_rd_hit.
                if i == 0 {
                    // First two bytes of the run's pinned frame
                    // (INV-CBYTES-4) — frame-hit truth for exthunt.
                    let head = crate::safety::bytes::pinned_frame_bytes(&run[0].1, 0, 2);
                    crate::fs::ext4::extent::exthunt_rd_hit(
                        ino,
                        page_index,
                        head[0],
                        head[1],
                    );
                }
                // page_data pointers are valid page-aligned pointers of
                // block_size bytes pinned by get_range_into; the copy
                // length is bounded by the contiguous span and the
                // remaining request bytes. Pointer values are only
                // COMPARED here (physical adjacency); every byte access
                // goes through the pinned-frame view below.
                {
                    let avail = block_size_usize - in_page_off;
                    let mut span = avail;
                    let mut k = 1usize;
                    while i + k < run_len
                        && span < remaining_now
                        && run[i + k].1 as usize == run[i + k - 1].1 as usize + block_size_usize
                    {
                        span += block_size_usize;
                        k += 1;
                    }
                    let copy_len = core::cmp::min(remaining_now, span);
                    // run[i].1 is a pinned page-cache frame (released via
                    // put_range_with_entry below) — INV-CBYTES-4.
                    let copied = match dst.put(
                        buf_offset,
                        crate::safety::bytes::pinned_frame_bytes(
                            &run[i].1,
                            in_page_off,
                            copy_len,
                        ),
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            // Immediate full user fault: EFAULT when nothing
                            // has been delivered yet, short read otherwise.
                            cache.put_range_with_entry(
                                fs_id,
                                ino,
                                entry.as_ref().unwrap(),
                                &run[..run_len],
                            );
                            return if total_read == 0 { Err(e) } else { Ok(total_read) };
                        }
                    };
                    total_read += copied;
                    buf_offset += copied;
                    current_offset += copied as u64;
                    if copied < copy_len {
                        // Partial user fault: short read, stop.
                        cache.put_range_with_entry(
                            fs_id,
                            ino,
                            entry.as_ref().unwrap(),
                            &run[..run_len],
                        );
                        return Ok(total_read);
                    }
                    i += k;
                }
            }
            if probe {
                crate::dfx::filerd::charge(&crate::dfx::filerd::COPY_CYCLES, t_copy);
            }
            let t_unpin = if probe { crate::dfx::filerd::now() } else { 0 };
            // run_len > 0 implies get_range_with_entry pinned this run
            // inside `entry` — the Option is always Some on this path
            // (unwrap is the loud failure if that invariant breaks).
            cache.put_range_with_entry(
                fs_id,
                ino,
                entry.as_ref().unwrap(),
                &run[..run_len],
            );
            if probe {
                crate::dfx::filerd::charge(&crate::dfx::filerd::UNPIN_CYCLES, t_unpin);
            }
            continue;
        }

        // rux:filerd (retired dfx=filerd): this read call fell off the warm path.
        if probe {
            crate::dfx::filerd::note_miss();
        }

        // Cache miss on the first page: batch-fill from here. The window
        // covers the demand range plus (on sequential access) the read-ahead
        // window ahead of it; pages land in the cache and the loop serves
        // them through the fast path above.
        //
        // READ/WRITE COHERENCE (the ftest01/ftest05 bad-verify fix): this
        // whole miss path — mapping resolution (get_data_block), the
        // block-cache coherence guard, the multi-block DMA and the
        // page-cache inserts — runs under the ext4 big lock, exactly like
        // the write/truncate paths. Unlocked, it raced writers on OTHER
        // files (writes serialize on the same global lock): between the
        // `block_cached` guard and the DMA, a writer could reallocate,
        // zero and rewrite the very blocks being DMA'd (freed by this
        // file's truncate, recycled by the allocator to a concurrent
        // writer), so the read-back spliced a stale disk image into the
        // page cache — f05rep2's "read this file's other chunk / previous
        // round's data / zeros for written data" corruption family, and
        // ftest01/05's bad-verify TFAILs. Cached-page serving (the fast
        // path above) stays lock-free: cached pages are immutable and the
        // write path's invalidate already fences stale entries.
        let _ext4_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock();
        let seq_contd = offset == ra_state.last_read_end || ra_state.active;
        if demand_pages == 1 && offset % block_size == 0 {
            // Page-shaped miss (mmap demand-fault signature) — see the
            // fault-around window below; counted for KILLDUMP attribution.
            FAULT_MISSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        let window = if seq_contd || offset == 0 {
            core::cmp::max(demand_pages, crate::fs::readahead::MAX_READAHEAD_BLOCKS as u64)
        } else if demand_pages == 1 && offset % block_size == 0 {
            // sc105 fault-around: a single page-aligned one-page read is
            // the mmap demand-fault signature (the fault path preads
            // exactly PAGE_SIZE at a page boundary). Dynamic-linker fault
            // order interleaves segments (.text/.rodata/.got/.data), which
            // reads as random to the sequential detector above — every
            // miss then filled ONE page synchronously (sleep/wake + big
            // lock + run of ~25k faults on ~100 MB of ArkUI libs burned
            // the launcher's whole 3 s appspawn watchdog budget in kernel
            // time). Linux clusters mmap faults: fault-around maps 16
            // pages around the faulting address. Fill the same 16-page
            // cluster here — subsequent faults (interleaved or ascending)
            // hit the cache; plain random preads only pay one extra merged
            // device request.
            FAULT_FILL_MISSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            core::cmp::max(demand_pages, crate::fs::readahead::FAULT_AROUND_BLOCKS as u64)
        } else {
            FAULT_FILL_MISSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Random seek: read only what the request needs.
            demand_pages
        };
        let fill_upto = core::cmp::min(page_index + window, file_pages);
        fill_page_cache_batch(fs, inode, cache, fs_id, ino, page_index, fill_upto);

        // The fill skips unallocated pages (holes) but may also have bailed
        // early (I/O or metadata error, OOM). Verify against the block map:
        // an ALLOCATED-but-uncached page gets a synchronous single-block
        // read here; only true holes (block 0) zero-fill.
        // `resident`, not `get`: this is a pure membership check — a `get()`
        // handle dropped on the hit side leaks one pin per filled page, and
        // leaked pins keep entries alive past inode free as invalidated
        // shadows that poison the inode-number successor (the
        // msync01-after-mincore04 order-dependent corruption).
        if !cache.resident(fs_id, ino, page_index) {
            // Distinguish a HOLE (Ok(0) — zero-fill, the sparse contract)
            // from a metadata ERROR (Err — the extent/bitmap read failed):
            // the old unwrap_or(0) folded both into the hole arm and served
            // ZEROS for real data on a failing extent read (Linux returns
            // the ext4_map_blocks error as EIO — ext4_file_read_iter's
            // error-is-error discipline).
            let block_nr = match inode.get_data_block(fs, page_index) {
                Ok(b) => b,
                Err(_) => return Err(errno::Errno::IOError.as_neg_i32()),
            };
            if block_nr != 0 {
                if let Some(bh) = bio::bread(fs.device, block_nr) {
                    let data = bio::bh_data(&bh);
                    // rux:exthunt (retired dfx=exthunt): sync-fallback serve truth (see
                    // exthunt_rd_serve).
                    crate::fs::ext4::extent::exthunt_rd_serve(
                        ino,
                        page_index,
                        block_nr,
                        data[0],
                        data[1],
                    );
                    let avail = block_size_usize - page_offset;
                    let copy_len = core::cmp::min(remaining, avail);
                    // The audited ReadDst::put span — the raw window
                    // add rides its in-range contract (page_offset <
                    // block_size <= data.len()).
                    match dst.put(buf_offset, &data[page_offset..page_offset + copy_len]) {
                        Ok(copied) => {
                            total_read += copied;
                            buf_offset += copied;
                            current_offset += copied as u64;
                        }
                        Err(e) => {
                            cache.insert(fs_id, ino, page_index, block_nr, data);
                            bio::brelse(bh);
                            return if total_read == 0 { Err(e) } else { Ok(total_read) };
                        }
                    }
                    cache.insert(fs_id, ino, page_index, block_nr, data);
                    bio::brelse(bh);
                    continue;
                }
                // Synchronous read failed: report I/O error rather than
                // silently returning zeros for real data.
                return Err(errno::Errno::IOError.as_neg_i32());
            }
            // True hole: zero-fill in place.
            crate::fs::ext4::extent::exthunt_read_hole(
                inode.ino as u32,
                &inode.block,
                page_index,
            );
            let avail = block_size_usize - page_offset;
            let zero_len = core::cmp::min(remaining, avail);
            // SAFETY: dst validity documented on ReadDst::put_zeroes.
            let copied = match dst.put_zeroes(buf_offset, zero_len) {
                Ok(c) => c,
                Err(e) => return if total_read == 0 { Err(e) } else { Ok(total_read) },
            };
            total_read += copied;
            buf_offset += copied;
            current_offset += copied as u64;
        } else {
            cache.put(fs_id, ino, page_index);
        }
    }

    // Update read-ahead bookkeeping (sequential detection state only — the
    // actual prefetch is the batch fill above).
    let _ = ra_state.on_read(offset, total_read as u64);

    Ok(total_read)
}

/// Kernel-slice wrapper (FileOps path).
fn ext4_file_read_cached(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &mut [u8],
    ra_state: &mut ReadAheadState,
) -> Result<usize, i32> {
    ext4_file_read_cached_dst(fs, inode, offset, ReadDst::Kernel(buf.as_mut_ptr(), buf.len()), ra_state)
}

/// Batch size bounds shared by fill_page_cache_batch/drain_batch.
/// MAX_BATCH_DRAIN bounds REQUESTS per sleep/wake cycle; MAX_BLOCKS_PER_BATCH
/// bounds the heap bytes alive per cycle (128 blocks x 4 KiB = 512 KiB).
const MAX_BATCH_DRAIN: usize = 128;
const MAX_BLOCKS_PER_BATCH: u64 = 512;

/// Max blocks coalesced into ONE device request (128 KiB). Under TCG the
/// per-request fixed cost (chain setup, header/resp alloc, device kick
/// amortization, IRQ) dwarfs the per-byte DMA cost.
const MAX_MERGE_BLOCKS: u64 = 64;

/// Batch-fill the page cache with pages [start, end) of a file.
///
/// Resolves each uncached page's physical block; runs of physically
/// consecutive blocks are COALESCED into single multi-block requests
/// (`bio::bread_async_multi`). The whole batch is submitted back-to-back,
/// the device is kicked ONCE, and a single sleep/wake cycle drains it.
/// Requests that cannot be submitted asynchronously (device without async
/// support, or the virtqueue is momentarily full) fall back to a synchronous
/// `bio::bread` AFTER draining what is already in flight — forward progress
/// is always guaranteed.
#[allow(clippy::too_many_arguments)]
fn fill_page_cache_batch(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    cache: &page_cache::PageCache,
    fs_id: u64,
    ino: u64,
    start: u64,
    end: u64,
) {
    const MAX_BATCH: usize = MAX_BATCH_DRAIN; // requests per sleep/wake cycle

    // Stack-depth fix (leg-16 KSTACK-HW attribution, resize2fs chain):
    // `core::array::from_fn` in the DEBUG build materialises the array
    // through try_from_fn's NeverShortCircuit/erased wrapper layers —
    // several full copies of the [IoCompletion; 128] batch alive at once,
    // a measured ~80KB kernel-stack span (the deepest chain in the boot,
    // the reason KERNEL_STACK_SIZE is 128KB, and ~358MB of heap at the
    // launcher floor). The inline-const repeat constructs each element
    // in place: no intermediate array copies. Same values, one codegen
    // change; verified by the KSTACK-HW probe on the next roll.
    let mut completions: [IoCompletion; MAX_BATCH] =
        [const { IoCompletion::new() }; MAX_BATCH];
    let mut bh_ptrs = [core::ptr::null_mut::<bio::BufferHead>(); MAX_BATCH];
    // Per submitted request: first file page index, first physical block,
    // and block count (skipped/sparse pages must not shift insert indices —
    // review EXT4-H6 discipline).
    let mut ra_idx: [u64; MAX_BATCH] = [0; MAX_BATCH];
    let mut ra_blk: [u64; MAX_BATCH] = [0; MAX_BATCH];
    let mut ra_len: [u32; MAX_BATCH] = [0; MAX_BATCH];
    let mut count = 0usize;
    let mut blocks_in_batch: u64 = 0;

    let mut idx = start;
    while idx < end {
        if count >= MAX_BATCH || blocks_in_batch >= MAX_BLOCKS_PER_BATCH {
            break;
        }
        // Already cached: nothing to do.
        if cache.get(fs_id, ino, idx).is_some() {
            cache.put(fs_id, ino, idx);
            idx += 1;
            continue;
        }

        // Resolve this page's physical block (0 = hole/unwritten).
        let block_nr = match inode.get_data_block(fs, idx) {
            Ok(b) => b,
            Err(_) => break, // metadata failure: stop extending the batch
        };
        if block_nr == 0 {
            // rux:exthunt (retired dfx=exthunt): the reader just verdict "hole" from the cached
            // root — see exthunt_read_hole.
            crate::fs::ext4::extent::exthunt_read_hole(
                inode.ino as u32,
                &inode.block,
                idx,
            );
            // STOP the read-ahead window at the first hole — the
            // reference kernel's readahead discipline. Holes are never
            // page-cache entries, so walking past one only re-resolves
            // extents (and cache probes) for pages that can never be
            // filled: a fully-sparse scratch file paid 512 lookups per
            // read(2) here (the lmbench bw_file_rd "50x" was exactly
            // this — 611 MB/s hole reads vs a 33.6 GB/s reference).
            // Demand pages after a hole still get served one at a time
            // by the caller's sync fallback, which handles holes.
            break;
        }

        // Coalesce the following pages while their physical blocks stay
        // consecutive (holes and fragmentation break the run naturally).
        let run_first = idx;
        let mut n: u64 = 1;
        while n < MAX_MERGE_BLOCKS
            && idx + n < end
            && blocks_in_batch + n < MAX_BLOCKS_PER_BATCH
        {
            match inode.get_data_block(fs, idx + n) {
                Ok(b) if b == block_nr + n => n += 1,
                _ => break,
            }
        }
        let nblocks = n as u32;

        // Coherence guard (bisect: 3bf0a33 broke Xorg's xkbcomp flow):
        // bread_async_multi DMA-reads the whole run into a private buffer
        // WITHOUT consulting the block cache. Any member block with a live
        // entry there — dirty ones hold data the disk does not have yet
        // (sync_dirty_buffer defers while a journal handle is active) —
        // would come back as stale disk bytes and poison the page cache
        // (write-then-read-back returned garbage; Xorg read back a
        // corrupt .xkm and died at "XKB: Failed to compile keymap").
        // Split the run at the first block-cached block: the clean prefix
        // still rides one multi-block request; the cached block (and
        // everything after) is revisited by the loop and fetched with
        // per-block bread_async, which returns the cached uptodate data.
        let submit_blocks = if nblocks > 1 {
            let mut k = 0u64;
            while k < nblocks as u64 {
                if bio::block_cached(fs.device, block_nr + k) {
                    break;
                }
                k += 1;
            }
            if k == 0 {
                1 // block 0 itself cached: single-block bread_async hits it
            } else {
                k as u32
            }
        } else {
            nblocks
        };

        match bio::bread_async_multi(fs.device, block_nr, submit_blocks, &completions[count]) {
            Some(bh) => {
                // rux:exthunt (retired dfx=exthunt): batch submit truth (see exthunt_rd_batch).
                crate::fs::ext4::extent::exthunt_rd_batch(
                    ino,
                    run_first,
                    block_nr,
                    submit_blocks,
                );
                bh_ptrs[count] = bh;
                ra_idx[count] = run_first;
                ra_blk[count] = block_nr;
                ra_len[count] = submit_blocks;
                count += 1;
                blocks_in_batch += submit_blocks as u64;
                idx += submit_blocks as u64;
            }
            None => {
                // Queue full (or async unsupported): drain what is in
                // flight, then continue — the next iteration retries this
                // run (async if a slot freed up, sync as last resort via
                // the count==0 branch below).
                if count > 0 {
                    drain_batch(cache, fs_id, ino, &bh_ptrs, &ra_idx, &ra_blk, &ra_len, &completions, count);
                    // The IoCompletions are single-shot: reset before the
                    // array is reused for the next sub-batch, or bread_wait
                    // would return the PREVIOUS batch's status immediately.
                    for c in completions.iter().take(count) {
                        c.reset();
                    }
                    count = 0;
                    blocks_in_batch = 0;
                } else {
                    // No async path at all: synchronous per-block reads
                    // (correctness fallback, e.g. loop devices).
                    // SAFETY: bio::bread returns a valid pinned BufferHead
                    // on success; b_data points to a block-sized buffer.
                    let mut ok = true;
                    for k in 0..n {
                        {
                            if let Some(bh) = bio::bread(fs.device, block_nr + k) {
                                let data = bio::bh_data(&bh);
                                cache.insert(fs_id, ino, run_first + k, block_nr + k, &data);
                                bio::brelse(bh);
                            } else {
                                ok = false; // I/O error: stop filling
                                break;
                            }
                        }
                    }
                    if !ok {
                        break;
                    }
                    idx += n;
                }
            }
        }
    }

    if count > 0 {
        drain_batch(cache, fs_id, ino, &bh_ptrs, &ra_idx, &ra_blk, &ra_len, &completions, count);
    }
}

/// Wait for a submitted batch and insert the completed pages into the cache.
///
/// R20-FS8 discipline: a failed read's (garbage) buffer must never be cached
/// as a valid page.
#[allow(clippy::too_many_arguments)]
fn drain_batch(
    cache: &page_cache::PageCache,
    fs_id: u64,
    ino: u64,
    bh_ptrs: &[*mut bio::BufferHead],
    ra_idx: &[u64],
    ra_blk: &[u64],
    ra_len: &[u32],
    completions: &[IoCompletion],
    count: usize,
) {
    let mut status = [0i32; MAX_BATCH_DRAIN];
    // One device kick for the whole quietly-submitted batch (see
    // pci_submit_read_async): publish N chains, notify once, then wait.
    crate::drivers::blkdev::kick_all_pending_disks();
    for i in 0..count {
        status[i] = bio::bread_wait(bh_ptrs[i], &completions[i]);
    }
    // Insert completed pages into the page cache under their own indices.
    for i in 0..count {
        // bread_wait has completed, so bh_ptrs[i] points to a valid
        // BufferHead with fully populated b_data (status 0 only); the
        // data view rides the INV-BIO-BHREF accessor.
        if status[i] == 0 {
            let data = bio::bh_data(&bh_ptrs[i]);
            let blocks = ra_len[i] as usize;
            if blocks <= 1 {
                cache.insert(fs_id, ino, ra_idx[i], ra_blk[i], data);
            } else {
                // Multi-block request: one bulk insert for the whole
                // contiguous buffer (one lock, buddy-alloc chunks, one
                // BTreeMap append — see insert_batch).
                cache.insert_batch(fs_id, ino, ra_idx[i], data.len() / blocks, data);
            }
        }
        if ra_len[i] <= 1 {
            // Single-block requests went through bread_async: they live
            // in the block cache, release with brelse.
            //
            // Abandon discipline (10s-deadline): on WAIT_TIMED_OUT the
            // device may still DMA into b_data, so the buffer must NOT
            // re-enter the block cache for reuse — the pin is
            // intentionally held forever (bounded by the rarity of 10s
            // stalls; freeing it corrupted unrelated pages). A real
            // device EIO COMPLETED its chain — brelse is safe there.
            if status[i] == crate::fs::io_completion::WAIT_TIMED_OUT {
                crate::pr_err!(
                    "ext4: leaked timed-out read-ahead bh (block {}) — device may still DMA",
                    ra_blk[i]
                );
            } else {
                bio::brelse(bh_ptrs[i]);
            }
        } else if status[i] == crate::fs::io_completion::WAIT_TIMED_OUT {
            // Multi-block private buffer, timed out (the waiter already
            // abandoned the pending so no late completion fires into
            // its stack): the device may still DMA into it.
            // NEVER free a buffer the device may still write — leak it
            // (R21-N2 discipline: integrity over a bounded leak).
            crate::pr_err!(
                "ext4: leaked timed-out multi-block read-ahead ({} blocks from {}) — device may still DMA",
                ra_len[i], ra_blk[i]
            );
        } else {
            // Multi-block buffers are private: free directly. A real
            // device EIO completed its chain — the DMA is done.
            // bfree_multi's contract — private (non-cached) buffer,
            // DMA quiesced (status != WAIT_TIMED_OUT above).
            bio::bfree_multi(bh_ptrs[i]);
        }
    }
}

/// Apply a SIGNED extent-tree metadata-sector delta to i_blocks.
///
/// `ext4_ext_insert_written`/`ext4_ext_materialize_block` report the
/// rebuild delta as i64 (a compact rebuild frees more metadata than it
/// allocates). The old u64 protocol folded a negative delta into
/// ~u64::MAX and the plain `+=` then overflow-panicked in release
/// builds; saturate at both ends instead (i_blocks accounting is
/// advisory — a clamp under-counts a tree, never corrupts one).
#[inline]
fn apply_meta_delta(blocks: u64, delta: i64) -> u64 {
    if delta >= 0 {
        blocks.saturating_add(delta as u64)
    } else {
        blocks.saturating_sub(delta.unsigned_abs())
    }
}

/// Source of a write through the ext4 core (GUP-DIO): the syscall
/// staging slice, or the caller's PINNED user frames (zero-copy DMA
/// source — the reference's bio_iov_iter_get_pages write shape).
#[derive(Clone, Copy)]
pub(crate) enum DioWriteSrc<'a> {
    Staging(&'a [u8]),
    /// Pinned frames backing `[va, va+len)`. The GUP write entry
    /// guarantees EVERY page pinned (any pin hole falls back to the
    /// staged path at request granularity — see
    /// ext4_file_write_dio_user_vfs), so gathers never touch user
    /// memory and never fault.
    User {
        pins: &'a crate::mm::gup::PinnedUserPages,
        va: usize,
    },
}

impl<'a> DioWriteSrc<'a> {
    #[inline]
    fn len(&self) -> usize {
        match self {
            DioWriteSrc::Staging(b) => b.len(),
            DioWriteSrc::User { pins, .. } => pins.len(),
        }
    }

    /// Copy `dst.len()` bytes of the source at request offset `off`
    /// into `dst` (the per-block body's buffer-cache write).
    fn gather(&self, off: usize, dst: &mut [u8]) {
        match self {
            DioWriteSrc::Staging(b) => dst.copy_from_slice(&b[off..off + dst.len()]),
            DioWriteSrc::User { pins, .. } => {
                let mut d = 0usize;
                pins.for_each_piece(off, dst.len(), |p, l| {
                    // The pin keeps the frame alive; the alias is l
                    // readable bytes (INV-MM-6 pinned-alias arm).
                    crate::mm::edges::copy_kva_from(&mut dst[d..][..l], p);
                    d += l;
                    true
                });
                debug_assert_eq!(d, dst.len(), "GUP write gather crossed a pin hole");
            }
        }
    }

    /// One probe byte at request offset `off` (exthunt truth).
    fn byte_at(&self, off: usize) -> u8 {
        match self {
            DioWriteSrc::Staging(b) => b[off],
            DioWriteSrc::User { pins, .. } => {
                // A one-byte gather: the piece walk owns the alias
                // reasoning, so the probe byte needs no raw deref.
                let mut b = [0u8; 1];
                self.gather(off, &mut b);
                b[0]
            }
        }
    }
}

/// In-flight cap of the GUP write window — one chain per span, so the
/// bound matches bio::MAX_WRITE_BATCH's device-pressure shape.
const MAX_GUP_WRITE_SPANS: usize = 32;

/// Write-side GUP chain window (GUP-DIO): bio::WriteBatch's contract
/// (quiet span submit / one-kick drain / per-span status) where every
/// span is ONE scatter-gather chain built from the span's PINNED user
/// frames. Spans are formed by the caller at <= MAX_SG_SEGS_PER_CHAIN
/// pieces, so each chain's device range is contiguous by construction.
///
/// Timeout discipline (the torn-write edge): a timed-out chain's frames
/// stay pinned until the request ends; wait() tombstones the software
/// pending so no late delivery is observed, and the span reports EIO —
/// the same post-timeout acceptance the WriteBatch's Borrowed arm
/// documents (a 10 s virtio deadline means the machine is wedged far
/// beyond it). The frames are unpinned after the caller's fail-cut
/// accounting, exactly like a staging buffer would be released.
struct GupWriteChains {
    device: &'static crate::drivers::blkdev::GenDisk,
    completions: [IoCompletion; MAX_GUP_WRITE_SPANS],
    /// Per span: the chain's first-segment pointer (the completion
    /// token the virtio walker stamps).
    token: [usize; MAX_GUP_WRITE_SPANS],
    /// Per-span outcome of the LAST flush.
    status: [i32; MAX_GUP_WRITE_SPANS],
    n: usize,
}

impl GupWriteChains {
    fn new(device: &'static crate::drivers::blkdev::GenDisk) -> Self {
        Self {
            device,
            completions: [const { IoCompletion::new() }; MAX_GUP_WRITE_SPANS],
            token: [0; MAX_GUP_WRITE_SPANS],
            status: [0; MAX_GUP_WRITE_SPANS],
            n: 0,
        }
    }

    /// Record + quietly submit one span (one SG chain over
    /// `segs[..k]`, device range starting at `base_sector`). Returns
    /// false when the window is full (caller drains, retries) or the
    /// device refused (caller keeps the per-block body).
    fn submit_span(
        &mut self,
        segs: &[crate::drivers::blkdev::DmaSeg],
        k: usize,
        base_sector: u64,
    ) -> bool {
        if self.n >= MAX_GUP_WRITE_SPANS {
            return false;
        }
        let i = self.n;
        // The GUP write contract: the caller guarantees the pinned
        // frames stable (and the user bytes unmodified) until the drain.
        let ok = crate::drivers::blkdev::blkdev_write_async_sg(
            self.device,
            base_sector,
            &segs[..k],
            &self.completions[i],
        )
        .is_ok();
        if ok {
            self.token[i] = segs[0].ptr as usize;
            self.n += 1;
        }
        ok
    }

    fn in_flight(&self) -> usize {
        self.n
    }

    /// Kick once, wait every chain in order, fill status[]. Returns
    /// the first nonzero status.
    fn flush(&mut self) -> i32 {
        if self.n == 0 {
            return 0;
        }
        crate::drivers::blkdev::kick_all_pending_disks();
        let mut first_err = 0i32;
        for i in 0..self.n {
            let rc = self.completions[i].wait();
            let st = if rc == 0 {
                // Attribution guard: the walker stamps the chain's first
                // data pointer into the completion.
                let tok = self.completions[i].delivered_token();
                if tok != 0 && tok != self.token[i] {
                    crate::pr_err!(
                        "bio: misattributed gup write completion (token {:x} != {:x}) — refusing to ack",
                        tok,
                        self.token[i]
                    );
                    -5
                } else {
                    0
                }
            } else {
                rc
            };
            self.status[i] = st;
            if st != 0 && first_err == 0 {
                first_err = st;
            }
            self.completions[i].reset();
        }
        self.n = 0;
        first_err
    }

    fn status(&self, i: usize) -> i32 {
        if i < MAX_GUP_WRITE_SPANS {
            self.status[i]
        } else {
            -5
        }
    }
}

/// The direct-write window: the staging WriteBatch or the GUP chain
/// window (same submit/flush/status surface — the dio_drain! macro is
/// written against it).
enum DioWin {
    Batch(bio::WriteBatch),
    Gup(GupWriteChains),
}

impl DioWin {
    #[inline]
    fn in_flight(&self) -> usize {
        match self {
            DioWin::Batch(b) => b.in_flight(),
            DioWin::Gup(g) => g.in_flight(),
        }
    }

    #[inline]
    fn flush(&mut self) -> i32 {
        match self {
            DioWin::Batch(b) => b.flush(),
            DioWin::Gup(g) => g.flush(),
        }
    }

    #[inline]
    fn status(&self, i: usize) -> i32 {
        match self {
            DioWin::Batch(b) => b.status(i),
            DioWin::Gup(g) => g.status(i),
        }
    }
}

/// Staged write wrapper — the pre-GUP signature (see ext4_file_write_src).
pub fn ext4_file_write(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &[u8],
    dio_fast: bool,
) -> Result<usize, i32> {
    ext4_file_write_src(fs, inode, offset, DioWriteSrc::Staging(buf), dio_fast)
}

/// The write body (source-parameterized GUP-DIO form): staging slice or
/// pinned user frames; everything else identical.
fn ext4_file_write_src(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    wsrc: DioWriteSrc<'_>,
    dio_fast: bool,
) -> Result<usize, i32> {
    let block_size = fs.block_size as u64;
    let to_write = wsrc.len() as u64;

    // Calculate required block count
    let end_offset = offset + to_write;
    let needed_blocks = (end_offset + block_size - 1) / block_size;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let sectors_per_block = (fs.block_size / 512) as u64;

    // DIO UNWRITTEN ALLOCATION (fio-delalloc lane, 2026-11-18): an
    // O_DIRECT write that grows the file allocates UNWRITTEN extents —
    // metadata only (one bitmap run pass + extent append): NO per-block
    // bread/zero/dirty populate of the fresh blocks, NO block-cache
    // entries, NO dirty notes. The direct fast arm below then DMAs the
    // caller's bytes straight over the unwritten run
    // (ext4_ext_get_run_write resolves the real physical run) and the
    // unwritten->written conversion runs at the write's COMPLETION, after
    // the final device drain — the reference kernel's shape
    // (iomap_dio_complete -> ext4_convert_unwritten_io_end_vec; bios over
    // unwritten extents, state conversion at end_io). The old shape
    // (written extents + per-block populate) breaded every fresh block
    // (synchronous reads of garbage), left 256 dirty cache victims per
    // 1 MiB for the write-return durability drain, and — because every
    // target block became cache-resident — forced the data through the
    // per-block buffer-cache body instead of the direct fast arm: the
    // fresh-vs-overwrite 0.33x vs 0.83x split (hotspot-diff's +282 us/op
    // fresh-allocation share). Gated on the same condition that arms the
    // direct batch below (dio_fast && 4096-byte blocks && extent file):
    // if the fast arm cannot run, the per-block body expects WRITTEN
    // extents to splice into (materialize handles the unwritten edges).
    let dio_unwritten = dio_fast && fs.block_size == 4096 && inode.has_extent();
    if needed_blocks > current_blocks {
        let first_touched = offset / block_size;
        let r = if dio_unwritten {
            allocate_blocks_unwritten_dio(
                fs,
                inode,
                needed_blocks,
                core::cmp::max(current_blocks, first_touched),
            )
        } else {
            allocate_blocks_for_file(fs, inode, needed_blocks,
                                     core::cmp::max(current_blocks, first_touched))
        };
        r?;
        // Delalloc reservations: every index below the freshly allocated
        // frontier is a real block now (or a hole whose own materialize
        // owns it) — release the pending-writeback reservations held for
        // those indices (fio-delalloc; see DELALLOC_RESERVATIONS). This
        // is the writeback twin of ext4_buffered_write's reserve: the
        // da_write_begin reserve and the mballoc claim, one allocation
        // apart.
        da_release_below(fs, inode.ino, needed_blocks);
    }

    // BULK UNWRITTEN CONVERSION (wrfsync lane, 2026-11-17): convert the
    // write's full-block interior from unwritten to WRITTEN in ONE tree
    // edit with neighbor merging — the reference kernel's shape (Linux
    // writes over unwritten extents and converts the extent state in
    // bulk, ext4_ext_convert_unwritten). The per-block materialize this
    // replaces split the covering extent per 4 KiB block: a 1 MiB direct
    // write into a fallocated file paid 256 tree edits + rebuilds, and a
    // random 4 KiB overwrite stream fragmented the tree into thousands of
    // single-block entries whose every later lookup walked a deep leaf.
    // Partial head/tail blocks keep the per-block materialize below
    // (unwritten halves must not convert past the bytes actually being
    // written).
    //
    // fio-delalloc: the DIO path (dio_unwritten) SKIPS the conversion
    // here and runs it at the write's END, after the data drain — the
    // reference's end_io ordering (data on the device before the extent
    // state flips; a crash between the two leaves unwritten = zeros, the
    // strictly safer exposure). The non-DIO paths keep the top position:
    // the per-block body below expects full blocks already WRITTEN (it
    // would otherwise re-fragment the interior one materialize per
    // block).
    if inode.has_extent() && !dio_unwritten {
        let first_full = (offset + block_size - 1) / block_size;
        let last_full = (offset + to_write) / block_size; // exclusive
        if last_full > first_full {
            let mut ext_meta: i64 = 0;
            match crate::fs::ext4::extent::ext4_ext_convert_unwritten(
                fs,
                &mut inode.block,
                first_full,
                last_full - first_full,
                &mut ext_meta,
                crate::fs::ext4::csum::dir_csum_seed(
                    fs.csum_seed,
                    inode.ino,
                    inode.generation,
                ),
            ) {
                Ok(()) => {
                    inode.blocks = apply_meta_delta(inode.blocks, ext_meta);
                }
                Err(e) => return Err(e),
            }
        }
    }

    // DIRECT FAST RUNS (paired-test item 1, 2026-11-17): for O_DIRECT
    // writes, full-block spans whose physical blocks are consecutive and
    // absent from the block cache go to the device DIRECTLY from the
    // caller's buffer (WriteBatch::submit_ref — the syscall staging
    // slice; no block-cache entry is created, no private copy made, the
    // batch drains before this function returns so the caller's bytes
    // stay the DMA source exactly as long as the device needs them).
    // GUP-DIO (2026-11-18): with a PINNED-frames source the same spans
    // ride SG chains built from the user frames — no staging copy at
    // all (the reference's bio_iov_iter_get_pages shape). Head/tail
    // partial blocks, holes, unwritten extents and cached blocks keep
    // the per-block body below (buffer-cache RMW truth).
    // Bound: 64-block spans (256 KiB — the proven contiguous-span
    // shape; the GUP arm packs the run into <=16-piece chains).
    const DIO_MAX_MERGE: u64 = 64;
    let mut dio_window: Option<DioWin> = if dio_fast && fs.block_size == 4096 {
        match &wsrc {
            DioWriteSrc::Staging(_) => Some(DioWin::Batch(bio::WriteBatch::new())),
            DioWriteSrc::User { .. } => {
                if (*fs.device).has_async_write_sg() {
                    Some(DioWin::Gup(GupWriteChains::new(fs.device)))
                } else {
                    None // no SG chains: per-block body (correct, staged)
                }
            }
        }
    } else {
        None
    };
    // GUP pieces cursor source (the pins), pre-resolved for the arm.
    let gup_pins = match &wsrc {
        DioWriteSrc::User { pins, .. } => Some(*pins),
        DioWriteSrc::Staging(_) => None,
    };
    // Per in-flight span: (dst offset in buf, byte len), submission order.
    let mut dio_spans: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::new();
    // Byte offset (in buf) of the first failed direct span — a short
    // write cuts total_written back to it even if later spans landed.
    let mut dio_fail_at: Option<usize> = None;
    // Set when a direct span failed: later iterations keep the per-block
    // body (ordering after a lost span is not worth re-plumbing).
    let mut dio_dead = false;

    // Drain the current direct window. Must run where `batch` (the
    // as_mut binding) is in scope. Records the first failed span's dst.
    macro_rules! dio_drain {
        ($batch:expr) => {{
            let n = $batch.in_flight();
            let rc = $batch.flush();
            let mut any_failed = rc != 0;
            if any_failed {
                let mut cut = dio_fail_at.unwrap_or(usize::MAX);
                for si in 0..n {
                    if $batch.status(si) != 0 {
                        if let Some(&(off, _)) = dio_spans.get(si) {
                            cut = cut.min(off);
                        }
                        break;
                    }
                }
                dio_fail_at = Some(cut);
            }
            dio_spans.clear();
            any_failed
        }};
    }

    // Write data
    let mut total_written = 0;
    let mut current_offset = offset;
    let mut buf_offset = 0;

    // POSIX write(2) / reference ext4 shape: once at least one byte of the
    // request has landed, a failure on a LATER block (ENOSPC on a
    // mid-request hole allocation, EIO on a block fetch) must report the
    // partial count, not the error — only a request that wrote nothing
    // fails outright. Appends cannot hit this (their blocks are
    // pre-allocated above, so allocation failure precedes all data), but
    // a write into a HOLE of a sparse file allocates mid-loop.
    macro_rules! partial_or_err {
        ($label:lifetime, $e:expr) => {
            if total_written > 0 {
                break $label;
            } else {
                return Err($e);
            }
        };
    }

    'block_loop: while total_written < to_write as usize {
        let block_index = current_offset / block_size;
        let block_offset = (current_offset % block_size) as usize;

        // Direct fast run: block-aligned, >= 1 full block left, extent
        // resolves, no block-cache entry inside the run.
        if !dio_dead {
            if let Some(win) = dio_window.as_mut() {
                let remaining_full = (to_write as usize - total_written) / block_size as usize;
                if block_offset == 0 && remaining_full >= 1 {
                    // ONE extent walk for the maximal run. WRITE-side
                    // resolver (fio-delalloc): unwritten extents report
                    // their REAL physical run so the direct arm targets
                    // them (reads keep zero-filling unwritten via
                    // ext4_ext_get_run) — per-block lookups paid 1-2
                    // cached breads per 4 KiB on depth>=1 files.
                    let (mut first_phys, run_avail, _uw) = if inode.has_extent() {
                        match crate::fs::ext4::extent::ext4_ext_get_run_write(
                            fs,
                            inode.ino,
                            &inode.block,
                            block_index,
                        ) {
                            Ok(v) => v,
                            Err(_) => (0, 0, false),
                        }
                    } else {
                        (0, 0, false)
                    };
                    if first_phys != 0 && run_avail > 0 && remaining_full >= 1 {
                        let mut n: u64 = core::cmp::min(
                            core::cmp::min(run_avail, remaining_full as u64),
                            DIO_MAX_MERGE,
                        );
                        if n > 1 {
                            let mut k: u64 = 1;
                            while k < n {
                                if bio::block_cached(fs.device, first_phys + k) {
                                    break;
                                }
                                k += 1;
                            }
                            n = k;
                        }
                        // Cached single block: the buffer cache holds the
                        // truth (possibly dirty) — the per-block body
                        // below serves it; no direct submission.
                        if n == 1 && bio::block_cached(fs.device, first_phys) {
                            n = 0;
                        }
                        let span_bytes = n as usize * block_size as usize;
                        match win {
                            DioWin::Batch(batch) => {
                                let span = match &wsrc {
                                    DioWriteSrc::Staging(b) => {
                                        &b[buf_offset..buf_offset + span_bytes]
                                    }
                                    _ => unreachable!(),
                                };
                                let mut submitted = batch.submit_ref(
                                    fs.device,
                                    first_phys * sectors_per_block,
                                    span,
                                );
                                if !submitted {
                                    // Window full: drain, retry once. A failed
                                    // drain kills the fast path for this call.
                                    if dio_drain!(batch) {
                                        dio_dead = true;
                                    } else {
                                        submitted = batch.submit_ref(
                                            fs.device,
                                            first_phys * sectors_per_block,
                                            span,
                                        );
                                    }
                                }
                                if submitted && !dio_dead {
                                    dio_spans.push((buf_offset, span_bytes));
                                    total_written += span_bytes;
                                    buf_offset += span_bytes;
                                    current_offset += span_bytes as u64;
                                    continue;
                                }
                            }
                            DioWin::Gup(g) => {
                                // GUP-DIO: pack the run's pinned pieces
                                // into chains of <= MAX_SEGS pieces; one
                                // span per chain. Bookkeeping advances
                                // per span so a mid-run abort (window
                                // full / device refusal) leaves the loop
                                // at the first un-submitted byte for the
                                // per-block body.
                                let pins = gup_pins.expect("gup window without pins");
                                // Chain packing cap from the negotiated
                                // ring depth (see the read twin): a
                                // fully-scattered 1 MiB run rides 2-3 big
                                // chains instead of 16 small ones.
                                const W_SEG_CAP: usize = 128;
                                let gup_slot = (*fs.device).private_data
                                    .map(|p| p as usize)
                                    .unwrap_or(0);
                                let (seg_max, _w_budget) =
                                    crate::drivers::virtio::pci_blk_sg_limits(gup_slot);
                                let pack_cap = seg_max.min(W_SEG_CAP);
                                let mut segs: [crate::drivers::blkdev::DmaSeg; W_SEG_CAP] =
                                    [crate::drivers::blkdev::DmaSeg {
                                        ptr: core::ptr::null_mut(),
                                        len: 0,
                                    }; W_SEG_CAP];
                                let run_dst = buf_offset;
                                let run_end = buf_offset + span_bytes;
                                let mut o = run_dst;
                                while o < run_end {
                                    let chain_dst = o;
                                    let mut k = 0usize;
                                    let mut bytes = 0usize;
                                    while k < pack_cap {
                                        match pins.next_piece(&mut o, run_end) {
                                            Some((p, l)) => {
                                                segs[k].ptr = p;
                                                segs[k].len = l;
                                                k += 1;
                                                bytes += l;
                                            }
                                            None => break,
                                        }
                                    }
                                    if bytes == 0 {
                                        break; // pin hole mid-run: per-block body
                                    }
                                    let base_sector = first_phys * sectors_per_block
                                        + ((chain_dst - run_dst) / 512) as u64;
                                    let mut submitted =
                                        g.submit_span(&segs, k, base_sector);
                                    if !submitted {
                                        if dio_drain!(g) {
                                            dio_dead = true;
                                            break;
                                        }
                                        submitted = g.submit_span(&segs, k, base_sector);
                                        if !submitted {
                                            break;
                                        }
                                    }
                                    dio_spans.push((chain_dst, bytes));
                                    total_written += bytes;
                                    buf_offset += bytes;
                                    current_offset += bytes as u64;
                                }
                                if o >= run_end && !dio_dead {
                                    continue;
                                }
                                // Fall through: the per-block body serves
                                // from the current buf_offset (in-flight
                                // spans settle at the next drain; their
                                // blocks are cache-absent by the split
                                // above, so the body never aliases them).
                            }
                        }
                        // Device cannot take the async write (or the
                        // window drain failed): per-block body below
                        // handles this block (correctness unchanged).
                    }
                }
            }
        }

        // Get data block number (supports indirect blocks)
        let block_num = match inode.get_data_block(fs, block_index) {
            Ok(0) => {
                // Extent-mapped file: 0 here is either a hole or an
                // UNWRITTEN fallocate-preallocated block. Unwritten blocks
                // convert to written on first write (split the extent,
                // materialize the one block — bounded by real data);
                // genuine holes still fail honestly below (extent insertion
                // is not implemented — review EXT4-C4).
                if inode.has_extent() {
                    // SIGNED metadata delta (a rebuild can free more tree
                    // blocks than it adds) — applied with saturation below.
                    let mut ext_meta: i64 = 0;
                    match crate::fs::ext4::extent::ext4_ext_materialize_block(
                        fs,
                        &mut inode.block,
                        block_index,
                        &mut ext_meta,
                        crate::fs::ext4::csum::dir_csum_seed(
                            fs.csum_seed,
                            inode.ino,
                            inode.generation,
                        ),
                    ) {
                        Ok(Some(phys)) => {
                            inode.blocks = apply_meta_delta(inode.blocks, ext_meta);
                            // Materialize the block contents now: the
                            // read-modify-write below breads it; an
                            // unzeroed block would splice garbage around
                            // the written span.
                            // SAFETY: fs.device is a valid GenDisk; phys is
                            // a freshly allocated block whose disk contents
                            // are irrelevant — getblk_zero creates a zeroed
                            // dirty buffer without the disk read.
                            // rux:exthunt (retired dfx=exthunt): unwritten-materialization zero.
                            crate::fs::ext4::extent::exthunt_zzero(
                                "mat", inode.ino, block_index, phys,
                            );
                            {
                                if let Some(bh) = bio::getblk_zero(fs.device, phys) {
                                    bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                                    bio::brelse(bh);
                                } else {
                                    partial_or_err!('block_loop, errno::Errno::IOError.as_neg_i32());
                                }
                            }
                            phys
                        }
                        Ok(None) => {
                            // Hole in an extent file (e.g. ftruncate-grow
                            // then write): allocate one block, map it with
                            // a written extent entry, zero it.
                            let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                            let goal_group =
                                (inode.ino / fs.inodes_per_group).min(fs.group_count - 1);
                            let new_block = match allocator.alloc_block(goal_group) {
                                Ok(b) => b,
                                Err(e) => partial_or_err!('block_loop, e),
                            };
                            {
                                // SIGNED delta: a shrinking rebuild returns
                                // hm < 0. The old u64 fold turned that into
                                // ~u64::MAX and `inode.blocks += hm`
                                // overflow-panicked (dio_read's 128 MB
                                // buffered write on a fragmented ext4).
                                let mut hm: i64 = 0;
                                match crate::fs::ext4::extent::ext4_ext_insert_written(
                                    fs,
                                    &mut inode.block,
                                    block_index,
                                    new_block,
                                    1,
                                    &mut hm,
                                    crate::fs::ext4::csum::dir_csum_seed(
                                        fs.csum_seed,
                                        inode.ino,
                                        inode.generation,
                                    ),
                                ) {
                                    Ok(()) => {
                                        inode.blocks = apply_meta_delta(inode.blocks, hm);
                                    }
                                    Err(e) => partial_or_err!('block_loop, e),
                                }
                            }
                            // SAFETY: freshly allocated block; getblk_zero
                            // fabricates the zeroed dirty buffer without a
                            // disk read (full-block overwrite follows).
                            // rux:exthunt (retired dfx=exthunt): hole-fill allocation zero.
                            crate::fs::ext4::extent::exthunt_zzero(
                                "hole", inode.ino, block_index, new_block,
                            );
                            {
                                if let Some(bh) = bio::getblk_zero(fs.device, new_block) {
                                    bio::brelse(bh);
                                } else {
                                    partial_or_err!('block_loop, errno::Errno::IOError.as_neg_i32());
                                }
                            }
                            inode.blocks += sectors_per_block;
                            new_block
                        }
                        Err(e) => partial_or_err!('block_loop, e),
                    }
                } else {
                    // Block not allocated, need to allocate a new one for writing
                    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                    let goal_group = (inode.ino / fs.inodes_per_group).min(fs.group_count - 1);
                    let new_block = match allocator.alloc_block(goal_group) {
                        Ok(b) => b,
                        Err(e) => partial_or_err!('block_loop, e),
                    };

                    // Zero the new block (deferred-dirty: the write loop
                    // below leaves the data buffer BH_Dirty; the block
                    // cache write-back persists it — see ext4_file_write).
                    // SAFETY: bio::bread returns a valid BufferHead;
                    // b_data is a block-sized writable buffer.
                    {
                        let bh = match bio::bread(fs.device, new_block) {
                            Some(b) => b,
                            None => partial_or_err!('block_loop, errno::Errno::IOError.as_neg_i32()),
                        };

                        for byte in bio::bh_data_mut(&bh).iter_mut() {
                            *byte = 0;
                        }
                        bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                        bio::brelse(bh);
                    }

                    // Update inode block pointer
                    if block_index < 12 {
                        inode.block[block_index as usize] = new_block as u32;
                    } else {
                        // Handle indirect blocks
                        match allocate_indirect_block(fs, inode, block_index, new_block, &allocator, goal_group) {
                            Ok(()) => {}
                            Err(e) => partial_or_err!('block_loop, e),
                        }
                    }
                    inode.blocks += sectors_per_block;

                    new_block
                }
            }
            Ok(b) => b,
            Err(e) => partial_or_err!('block_loop, e),
        };

        // SAFETY: bio::bread returns a valid BufferHead; b_data is a writable
        // block-sized buffer. The slice ranges are bounded by block_size.
        {
            // FULL-BLOCK OVERWRITE (flush-wall fix, 2026-11): when this
            // loop step covers the whole buffer-cache block (4096 — the
            // cache's fixed granularity), fetch it with getblk_overwrite
            // instead of bread: no disk READ on a miss (the buffer is
            // fabricated zeroed+dirty and every byte is overwritten
            // before the buffer becomes observable — the same contract
            // namei's dir-block writer already relies on). This is the
            // writeback shape: a flush of an N-page file re-writes N
            // allocated blocks in full; with bread each buffer the
            // cache evicted since the last pass cost one synchronous
            // 4 KiB read (measured: the msync pass of the mtest06
            // shape, 8192 blocks vs an 8192-entry cache — the read
            // wall). Partial blocks still take the read path: they
            // splice into existing content.
            let bh = match {
                if block_offset == 0 {
                    let remaining = to_write as usize - total_written;
                    let write_in_block =
                        core::cmp::min(remaining, block_size as usize);
                    if write_in_block >= 4096 {
                        bio::getblk_overwrite(fs.device, block_num)
                    } else {
                        bio::bread(fs.device, block_num)
                    }
                } else {
                    bio::bread(fs.device, block_num)
                }
            } {
                Some(b) => b,
                None => partial_or_err!('block_loop, errno::Errno::IOError.as_neg_i32()),
            };

            let data = bio::bh_data_mut(&bh);
            let remaining = to_write as usize - total_written;
            let available_in_block = block_size as usize - block_offset;
            let write_in_block = core::cmp::min(remaining, available_in_block);

            // Write data to block (GUP-DIO: gather from the pinned
            // frames — no staging slice on the user-source path).
            wsrc.gather(
                buf_offset,
                &mut data[block_offset..block_offset + write_in_block],
            );

            // rux:exthunt (retired dfx=exthunt): the writer's (logical, phys, pattern) truth.
            crate::fs::ext4::extent::exthunt_wr_put(
                inode.ino,
                block_index,
                block_num,
                wsrc.byte_at(buf_offset),
                data.as_ptr(),
            );

            // Deferred write-back: leave the buffer BH_Dirty in the block
            // cache instead of syncing it here. A synchronous write per
            // write(2) (~10-20ms of virtio wait under TCG) made any
            // small-write workload — every LTP test's setup — blow the
            // 30s wall clock. Durability now comes from fsync
            // (ext4_sync_file syncs the file's dirty data blocks), the
            // buffer-cache eviction path (dirty victims are synced before
            // reuse), and sync(2).
            bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
            bio::brelse(bh);
            // Dirty-note (see DIRTY_DATA_BLOCKS): the fsync walk is
            // note-driven for regular files — every data block left
            // BH_Dirty here must be in the set.
            note_dirty_data_block(fs as *const _ as u64, inode.ino as u64, block_num);

            total_written += write_in_block;
            buf_offset += write_in_block;
            current_offset += write_in_block as u64;
        }
    }

    // Drain the direct fast runs (device-ack every span before the
    // caller's buffer goes back to userspace). A failed span truncates the
    // reported count to its start — bytes after it may or may not have
    // landed, and POSIX allows the short count (the per-block path's
    // partial-write discipline, span-granular).
    // SUBMIT-2X: borrow in place, NEVER move — the inline completion
    // slots are part of the window value; a take()/move with spans in
    // flight would leave the device's pending table pointing at the
    // old stack location (the Box-based batch was accidentally
    // move-safe; the inline-slot one is not).
    if let Some(win) = dio_window.as_mut() {
        if win.in_flight() > 0 {
            if dio_drain!(win) {
                crate::pr_err!(
                    "ext4: direct write span batch failure — truncating to confirmed bytes"
                );
            }
        }
    }
    if let Some(fail_at) = dio_fail_at {
        let confirmed = fail_at.min(total_written);
        if confirmed < total_written {
            total_written = confirmed;
        }
    }

    // END-OF-WRITE UNWRITTEN CONVERSION (fio-delalloc lane, 2026-11-18):
    // the DIO path deferred the unwritten->written conversion here —
    // AFTER the drain above device-acked every span, the reference
    // kernel's end_io shape (iomap_dio_complete ->
    // ext4_convert_unwritten_io_end_vec over the write's io_end vec: one
    // bulk tree edit per write, off the data path). Data-before-metadata
    // crash order: a crash between the data write and this edit leaves
    // unwritten extents (readers/e2fsck see zeros — the strictly safer
    // exposure vs converting first). Read-after-write correctness needs
    // the conversion INSIDE the syscall (reads of unwritten zero-fill),
    // exactly where the reference runs it. Covers the freshly allocated
    // unwritten runs of this write AND any older fallocated unwritten
    // overlap; partial head/tail blocks were materialized per-block by
    // the body above. Range = the CONFIRMED full-block interior only (a
    // short/failed write leaves its unwritten tail unconverted — zeros,
    // which is what a never-completed write owes).
    if dio_unwritten {
        let first_full = (offset + block_size - 1) / block_size;
        let confirmed_end = offset + total_written as u64;
        let last_full = confirmed_end / block_size; // exclusive
        if last_full > first_full {
            let mut ext_meta: i64 = 0;
            match crate::fs::ext4::extent::ext4_ext_convert_unwritten(
                fs,
                &mut inode.block,
                first_full,
                last_full - first_full,
                &mut ext_meta,
                crate::fs::ext4::csum::dir_csum_seed(
                    fs.csum_seed,
                    inode.ino,
                    inode.generation,
                ),
            ) {
                Ok(()) => {
                    inode.blocks = apply_meta_delta(inode.blocks, ext_meta);
                }
                // A conversion failure after the data landed must not
                // report the write short: the bytes are on the device and
                // the syscall's contract is about DATA. The unconverted
                // unwritten extents zero-fill for readers until the next
                // conversion point (fsync/next write) — the honest
                // degraded state, reported once.
                Err(e) => {
                    crate::pr_err!(
                        "ext4: post-dio unwritten conversion failed (ino {} range {}+{}) — {}",
                        inode.ino,
                        first_full,
                        last_full - first_full,
                        e
                    );
                }
            }
        }
    }

    // Update file size (partial-write safe: a mid-loop allocation failure
    // broke out with fewer bytes than requested — the size must reflect
    // only what landed).
    let effective_end = offset + total_written as u64;
    if effective_end > inode.get_size() {
        inode.set_size(effective_end);
    }

    // Update inode timestamp
    let sec = crate::drivers::rtc::wall_secs() as u32;
    inode.mtime = sec;
    inode.ctime = sec;

    Ok(total_written)
}

pub fn allocate_blocks_for_file(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
    from_block: u64,
) -> Result<(), i32> {
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    let block_size = fs.block_size as u64;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let start_block = core::cmp::max(from_block, current_blocks).min(needed_blocks);
    let sectors_per_block = (fs.block_size / 512) as u64;
    let goal_group = (inode.ino / fs.inodes_per_group).min(fs.group_count - 1);

    // Check if file uses extents
    if inode.has_extent() {
        return allocate_blocks_with_extents(fs, inode, needed_blocks, start_block, &allocator, goal_group);
    }

    // Allocate new blocks (indirect block mode). Run-based like the
    // extents path: one bitmap pass per contiguous run instead of one per
    // block — a 300MB fallocate (LTP tst_acquire_device) is 76800 bitmap
    // read/write/desc/superblock round trips otherwise.
    let mut i = start_block;
    while i < needed_blocks {
        let want = core::cmp::min(needed_blocks - i, 0x7FFF) as u32;
        let (run_start, run_len) = match allocator.alloc_block_run(goal_group, want) {
            Ok(Some(run)) => run,
            Ok(None) => {
                return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
            }
            Err(e) => return Err(e),
        };
        for k in 0..run_len as u64 {
            let data_block = run_start + k;
            // Zero newly allocated data block (deferred-dirty).
            // SAFETY: bio::bread returns a valid BufferHead; b_data is a
            // block-sized writable buffer. We zero it before use.
            {
                let bh = bio::bread(fs.device, data_block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                for byte in bio::bh_data_mut(&bh).iter_mut() {
                    *byte = 0;
                }

                bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                bio::brelse(bh);
            }
            // Dirty-note (see DIRTY_DATA_BLOCKS).
            note_dirty_data_block(fs as *const _ as u64, inode.ino as u64, data_block);
            let block_index = i + k;

            if block_index < 12 {
                // Direct block
                inode.block[block_index as usize] = data_block as u32;
            } else {
                // Indirect block
                allocate_indirect_block(fs, inode, block_index, data_block, &allocator, goal_group)?;
            }
            inode.blocks += sectors_per_block;
        }
        i += run_len as u64;
    }

    Ok(())
}

/// Unwritten-extent preallocation for fallocate(2) (extent files only).
///
/// Allocates contiguous runs and records them as UNWRITTEN extents
/// (ee_len bit 15): the bitmap is updated (metadata-only — a handful of
/// I/Os regardless of size) and NO data blocks are zeroed or read. Reads
/// of unwritten blocks are zero-filled (ext4_ext_get_block_ex); the first
/// write converts the covered block (ext4_ext_materialize_block) — the
/// work stays proportional to the data actually written, not to the
/// preallocated size. This is what Linux's fallocate does and what makes
/// LTP's 300MB tst_acquire_device scratch file instant.
///
/// The extent tree grows into external nodes past the 4-entry inline root
/// (ext4_ext_append): on a fragmented filesystem a 300MB preallocation
/// spans dozens of runs — the old 4-entry cap failed the fallocate midway,
/// leaving a partial scratch file that LTP never cleans, which leaked the
/// whole filesystem into ENOSPC (the r3 mkdir/mknod/lseek regression
/// family).
pub fn preallocate_unwritten_extents(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
    current_blocks: u64,
    allocator: &crate::fs::ext4::allocator::BlockAllocator,
    goal_group: u32,
) -> Result<(), i32> {
    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};

    let sectors_per_block = (fs.block_size / 512) as u64;
    const MAX_EXTENT_LEN: u32 = 0x7FFF;

    let mut meta_sectors: u64 = 0;
    let mut logical_block = current_blocks;
    while logical_block < needed_blocks {
        let want = core::cmp::min((needed_blocks - logical_block) as u32, MAX_EXTENT_LEN);
        let Some((run_start, run_len)) = allocator.alloc_block_run(goal_group, want)? else {
            return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
        };
        // Make sure the root carries a valid (empty) extent header.
        {
            // INV-EXT4-IBLOCK-HDR: typed header view of the 60-byte root.
            let header = crate::fs::ext4::extent::iblock_header_mut(&mut inode.block);
            if header.eh_magic != EXT4_EXT_MAGIC {
                header.eh_magic = EXT4_EXT_MAGIC;
                header.eh_entries = 0;
                header.eh_max = 4;
                header.eh_depth = 0;
                header.eh_generation = 0;
            }
        }
        // Append the run as an UNWRITTEN extent (merges with the previous
        // unwritten extent when contiguous — append_node handles that).
        let ext = crate::fs::ext4::extent::RawExtent {
            ee_block: logical_block as u32,
            phys: run_start,
            len: run_len as u16,
            unwritten: true,
        };
        let csum_seed =
            crate::fs::ext4::csum::dir_csum_seed(fs.csum_seed, inode.ino, inode.generation);
        crate::fs::ext4::extent::ext4_ext_append(fs, &mut inode.block, &ext, &mut meta_sectors, csum_seed)?;
        inode.blocks += sectors_per_block * run_len as u64;
        logical_block += run_len as u64;
    }
    inode.blocks += meta_sectors;

    Ok(())
}

/// Allocate blocks for extent-based files (written data path).
///
/// Run-based: contiguous blocks are claimed with one bitmap pass
/// (alloc_block_run) and zeroed DEFERRED-dirty. Extents append through
/// the multi-level tree engine (ext4_ext_append) — fragmentation no
/// longer caps a file at 4 extents.
fn allocate_blocks_with_extents(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
    current_blocks: u64,
    allocator: &crate::fs::ext4::allocator::BlockAllocator,
    goal_group: u32,
) -> Result<(), i32> {
    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};

    let sectors_per_block = (fs.block_size / 512) as u64;
    const MAX_EXTENT_LEN: u32 = 0x7FFF; // ee_len u16 with the unwritten bit reserved

    let mut meta_sectors: u64 = 0;
    let mut logical_block = current_blocks;
    while logical_block < needed_blocks {
        let want = core::cmp::min((needed_blocks - logical_block) as u32, MAX_EXTENT_LEN);
        let Some((run_start, run_len)) = allocator.alloc_block_run(goal_group, want)? else {
            return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
        };

        // Zero each new block in the run (deferred-dirty, no sync).
        // SAFETY: bio::bread returns a valid BufferHead; b_data is a
        // block-sized writable buffer.
        for b in run_start..run_start + run_len as u64 {
            {
                let bh = bio::bread(fs.device, b)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;
                for byte in bio::bh_data_mut(&bh).iter_mut() {
                    *byte = 0;
                }
                bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                bio::brelse(bh);
            }
        }
        // Dirty-notes for the zeroed run (see DIRTY_DATA_BLOCKS).
        note_dirty_data_run(fs as *const _ as u64, inode.ino as u64, run_start, run_len as u64);

        // Valid (possibly empty) extent header on the root.
        {
            // INV-EXT4-IBLOCK-HDR: typed header view of the 60-byte root.
            let header = crate::fs::ext4::extent::iblock_header_mut(&mut inode.block);
            if header.eh_magic != EXT4_EXT_MAGIC {
                header.eh_magic = EXT4_EXT_MAGIC;
                header.eh_entries = 0;
                header.eh_max = 4;
                header.eh_depth = 0;
                header.eh_generation = 0;
            }
        }

        // Append the run (merge-with-last handled by the engine).
        let ext = crate::fs::ext4::extent::RawExtent {
            ee_block: logical_block as u32,
            phys: run_start,
            len: run_len as u16,
            unwritten: false,
        };
        let csum_seed =
            crate::fs::ext4::csum::dir_csum_seed(fs.csum_seed, inode.ino, inode.generation);
        crate::fs::ext4::extent::ext4_ext_append(fs, &mut inode.block, &ext, &mut meta_sectors, csum_seed)?;
        inode.blocks += sectors_per_block * run_len as u64;
        logical_block += run_len as u64;
    }
    inode.blocks += meta_sectors;

    Ok(())
}

/// Metadata-only UNWRITTEN allocation for a growing O_DIRECT write
/// (fio-delalloc lane, 2026-11-18 — the reference kernel's DIO shape:
/// ext4_map_blocks(PRE_IO) maps unwritten extents at submit time, the
/// data bios target their physical blocks, and the unwritten->written
/// conversion runs at the write's completion/end_io).
///
/// Identical run-based bitmap discipline to `allocate_blocks_with_extents`
/// (one bitmap read/scan/mark/write per contiguous run, spiral goal-group
/// search, multi-level extent append), but the fresh data blocks are
/// NEVER breaded/zeroed/dirtied: reads of unwritten blocks zero-fill, so
/// there is nothing to populate — allocation is bitmap + extent append,
/// full stop. No block-cache entries and no dirty notes are created, so
/// the write's direct fast arm sees a cold cache and DMAs the caller's
/// bytes straight over the unwritten run; the conversion at the write's
/// end (see ext4_file_write's END-OF-WRITE UNWRITTEN CONVERSION) flips
/// the extent state after the data is device-acked.
///
/// ENOSPC: same contract as the written twin — failure leaves the runs
/// already appended allocated-but-unwritten beyond the old EOF (readable
/// as zeros, invisible past the never-extended size); no torn extents
/// (each append is a complete run).
fn allocate_blocks_unwritten_dio(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    needed_blocks: u64,
    from_block: u64,
) -> Result<(), i32> {
    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};

    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    let block_size = fs.block_size as u64;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let start_block = core::cmp::max(from_block, current_blocks).min(needed_blocks);
    let sectors_per_block = (fs.block_size / 512) as u64;
    let goal_group = (inode.ino / fs.inodes_per_group).min(fs.group_count - 1);
    const MAX_EXTENT_LEN: u32 = 0x7FFF; // ee_len u16 with the unwritten bit reserved

    let mut meta_sectors: u64 = 0;
    let mut logical_block = start_block;
    while logical_block < needed_blocks {
        let want = core::cmp::min((needed_blocks - logical_block) as u32, MAX_EXTENT_LEN);
        let Some((run_start, run_len)) = allocator.alloc_block_run(goal_group, want)? else {
            return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
        };

        // Valid (possibly empty) extent header on the root (same as the
        // written twin).
        {
            // INV-EXT4-IBLOCK-HDR: typed header view of the 60-byte root.
            let header = crate::fs::ext4::extent::iblock_header_mut(&mut inode.block);
            if header.eh_magic != EXT4_EXT_MAGIC {
                header.eh_magic = EXT4_EXT_MAGIC;
                header.eh_entries = 0;
                header.eh_max = 4;
                header.eh_depth = 0;
                header.eh_generation = 0;
            }
        }

        // Append the run as an UNWRITTEN extent (merges with a trailing
        // unwritten extent when contiguous — the state-aware merge in
        // append_node refuses mixed states, exactly Linux's
        // ext4_can_extents_be_merged).
        let ext = crate::fs::ext4::extent::RawExtent {
            ee_block: logical_block as u32,
            phys: run_start,
            len: run_len as u16,
            unwritten: true,
        };
        let csum_seed =
            crate::fs::ext4::csum::dir_csum_seed(fs.csum_seed, inode.ino, inode.generation);
        crate::fs::ext4::extent::ext4_ext_append(
            fs,
            &mut inode.block,
            &ext,
            &mut meta_sectors,
            csum_seed,
        )?;
        inode.blocks += sectors_per_block * run_len as u64;
        logical_block += run_len as u64;
    }
    inode.blocks += meta_sectors;

    Ok(())
}

pub fn allocate_indirect_block(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    block_index: u64,
    data_block: u64,
    allocator: &crate::fs::ext4::allocator::BlockAllocator,
    goal_group: u32,
) -> Result<(), i32> {
    let block_size = fs.block_size as u64;
    let pointers_per_block = block_size / 4;
    let indirect_offset = block_index - 12;

    if indirect_offset < pointers_per_block {
        // Single indirect block
        if inode.block[12] == 0 {
            // Need to allocate single indirect block
            let indirect_block = allocator.alloc_block(goal_group)?;
            inode.block[12] = indirect_block as u32;

            // Zero indirect block
            // SAFETY: bio::bread returns a valid BufferHead; b_data is a
            // block-sized writable buffer. We zero it to initialize block
            // pointers before use.
            {
                let bh = bio::bread(fs.device, indirect_block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                for byte in bio::bh_data_mut(&bh).iter_mut() {
                    *byte = 0;
                }

                bio::bh_set_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty);
                let sync_res = bio::sync_dirty_buffer(bh);
                bio::brelse(bh);
                sync_res?;
            }
        }

        // Write block number to indirect block
        indirect::write_indirect_block(
            fs,
            inode.block[12] as u64,
            indirect_offset as usize,
            data_block as u32,
        )?;
    } else {
        let double_offset = indirect_offset - pointers_per_block;
        let double_pointers = pointers_per_block * pointers_per_block;

        if double_offset < double_pointers {
            // Double indirect block
            if inode.block[13] == 0 {
                // Need to allocate double indirect block
                let double_block = allocator.alloc_block(goal_group)?;
                inode.block[13] = double_block as u32;

                // Zero double indirect block: the block-update edge
                // owns pin/dirty/journal/sync/release (INV-BIO-BLOCKUPD).
                bio::with_block_mut(fs.device, double_block, |data, _| {
                    data.fill(0);
                    Ok(())
                })?;
            }

            // First level index
            let first_index = (double_offset / pointers_per_block) as usize;
            let second_index = (double_offset % pointers_per_block) as usize;

            // Get or allocate single indirect block
            let mut indirect_block = indirect::read_indirect_block(
                fs,
                inode.block[13] as u64,
                first_index,
            )?;

            if indirect_block == 0 {
                // Need to allocate single indirect block
                indirect_block = allocator.alloc_block(goal_group)?;

                // Zero single indirect block: same block-update edge
                // (INV-BIO-BLOCKUPD).
                bio::with_block_mut(fs.device, indirect_block, |data, _| {
                    data.fill(0);
                    Ok(())
                })?;

                // Update double indirect block
                indirect::write_indirect_block(
                    fs,
                    inode.block[13] as u64,
                    first_index,
                    indirect_block as u32,
                )?;
            }

            // Write data block number to single indirect block
            indirect::write_indirect_block(
                fs,
                indirect_block,
                second_index,
                data_block as u32,
            )?;
        } else {
            // Triple indirect block - not supported yet
            return Err(errno::Errno::FileTooLarge.as_neg_i32());
        }
    }

    Ok(())
}

pub fn ext4_file_lseek(
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: isize,
    whence: i32,
) -> Result<isize, i32> {
    let file_size = inode.get_size() as isize;

    let new_pos = match whence {
        0 => offset,              // SEEK_SET
        1 => {
            // TODO: Need to track current file position
            return Err(errno::Errno::FunctionNotImplemented.as_neg_i32());
        }
        2 => file_size + offset,   // SEEK_END
        _ => return Err(errno::Errno::InvalidArgument.as_neg_i32()),
    };

    if new_pos < 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    Ok(new_pos)
}

pub fn ext4_sync_file(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    datasync: i32,
) -> Result<(), i32> {
    // ext4:ext4_sync_file_enter/exit — the durability point bracket.
    // Divergence (see the event doc): parent reports 0 (no dentry parent
    // at hand here); datasync is the VFS entry's flag (fdatasync=1).
    if crate::trace::EVENT_EXT4_SYNC_FILE_ENTER.armed() {
        crate::trace::trace_ext4_sync_file_enter(
            super::dev_id(fs),
            inode.ino as u64,
            0,
            datasync,
        );
    }
    let ret = ext4_sync_file_inner(fs, inode);
    if crate::trace::EVENT_EXT4_SYNC_FILE_EXIT.armed() {
        crate::trace::trace_ext4_sync_file_exit(
            super::dev_id(fs),
            inode.ino as u64,
            match ret {
                Ok(()) => 0,
                Err(e) => e,
            },
        );
    }
    ret
}

// =====================================================================
// Per-inode dirty DATA-block notes (wrfsync lane, 2026-11-17).
//
// The old fsync data walk probed EVERY block of the file's extent map
// (2048 hash+LRU probes for an 8 MiB file) and found nothing dirty in
// the write+fsync pattern — the flush pass had already written and
// settled everything (measured: 127M probes, 0 dirty over 63k fsyncs,
// 80 us per fsync of pure waste). The reference kernel's fsync walks
// only what its page tree tags DIRTY; this tracker is the buffer-cache
// analog. Every ext4 path that leaves a FILE-DATA block BH_Dirty notes
// it here (the write core's per-block body, the allocation zero
// loops); the fsync walk iterates the notes, keeps only the entries the
// block cache still reports dirty (a synced eviction leaves a stale
// note — the probe is the truth), and syncs the survivors in ONE
// batched window. Directory entry blocks do NOT go through the write
// core — fsync(dirfd) keeps the full extent walk below.
// =====================================================================
type DirtyDataMap =
    alloc::collections::BTreeMap<(u64, u64), alloc::collections::BTreeSet<u64>>;
static DIRTY_DATA_BLOCKS: crate::sync::spinlock::Spinlock<DirtyDataMap> =
    crate::sync::spinlock::Spinlock::new(alloc::collections::BTreeMap::new());

/// Note `blk` as a dirty data block of (fs, ino). Cheap: one BTreeSet
/// insert under a short-held global lock; called only from paths that
/// just set BH_Dirty on a file-data buffer.
fn note_dirty_data_block(fs_id: u64, ino: u64, blk: u64) {
    let mut m = DIRTY_DATA_BLOCKS.lock();
    m.entry((fs_id, ino)).or_default().insert(blk);
}

/// Note every block of `[start, start+len)` (the allocation zero loops).
fn note_dirty_data_run(fs_id: u64, ino: u64, start: u64, len: u64) {
    if len == 0 {
        return;
    }
    let mut m = DIRTY_DATA_BLOCKS.lock();
    let set = m.entry((fs_id, ino)).or_default();
    for b in start..start + len {
        set.insert(b);
    }
}

/// Snapshot (and keep) the noted dirty blocks of (fs, ino).
fn take_dirty_data_notes(fs_id: u64, ino: u64) -> alloc::vec::Vec<u64> {
    let m = DIRTY_DATA_BLOCKS.lock();
    match m.get(&(fs_id, ino)) {
        Some(set) => set.iter().copied().collect(),
        None => alloc::vec::Vec::new(),
    }
}

/// Drop notes whose blocks are no longer dirty (acked/clean); keep the
/// rest for the next durability point.
fn settle_dirty_data_notes(fs_id: u64, ino: u64, still_dirty: &[u64]) {
    let mut m = DIRTY_DATA_BLOCKS.lock();
    if let Some(set) = m.get_mut(&(fs_id, ino)) {
        let keep: alloc::collections::BTreeSet<u64> =
            still_dirty.iter().copied().collect();
        *set = keep;
        if set.is_empty() {
            m.remove(&(fs_id, ino));
        }
    }
}

// =====================================================================
// Delalloc block reservations (fio-delalloc lane, 2026-11-18) — the
// reference kernel's ext4_da_reserve_space shape (fs/ext4/inode.c:
// reserved-cluster accounting; a delayed-allocation write RESERVES space
// at write time so ENOSPC lands at write(2), not at writeback).
//
// Buffered writes defer physical allocation to the writeback points (the
// da shape, ext4_buffered_write); until then the blocks the dirty pages
// will need are counted here, per (fs, ino) as MERGED RUNS of file block
// indices [start, start+len) (the extent shape itself; the
// DIRTY_DATA_BLOCKS side-table discipline: one short-held global lock).
// The write-time ENOSPC gate then refuses an extending write when the
// filesystem's free blocks are exhausted BY RESERVATIONS (free <=
// reserved), not only when literally zero blocks remain — the precision
// the reference has and the old est_free_blocks()==0 gate lacked:
// concurrent writers on a nearly full fs used to race past the zero
// check and fail at flush time (the unflushable dirty balloon the
// falloc-wedge lane documented).
//
// Representation history: the first cut stored per-block indices in a
// BTreeSet — a sequential append walked a growing tree per 4 KiB write
// (p6_append +0.8 µs/op vs main, +24% — marginal-out on the gate band)
// and an earlier cut summed the whole map per write (O(files ever
// written); p6_append 4.9 -> 27+ µs — caught by the gate boards). Merged
// runs make the append case ONE tail-extend.
//
// Reserve points: buffered writes extending past the inode's allocated
// frontier (ceil of the cached on-disk inode size — allocation never
// leads the size it serves). Release points: the writeback allocation
// (ext4_file_write's allocate succeeds → every reserved index below the
// allocated frontier is now a real block), truncate-down (indices at or
// past the new size), inode deletion (all). Quota: no quota
// infrastructure exists yet — the reference's dq_alloc_block hook is the
// recorded follow-up.
// =====================================================================
type ResRuns = alloc::vec::Vec<(u64, u64)>; // sorted, merged [start, len)
type DelallocResMap = alloc::collections::BTreeMap<(u64, u64), ResRuns>;
static DELALLOC_RESERVATIONS: crate::sync::spinlock::Spinlock<DelallocResMap> =
    crate::sync::spinlock::Spinlock::new(alloc::collections::BTreeMap::new());
/// Total reserved blocks across every (fs, ino) — the fs-wide claim the
/// write-time gate checks against. Maintained INCREMENTALLY under the
/// DELALLOC_RESERVATIONS lock (Relaxed atomics; every mutation holds the
/// lock). The first cut summed `m.values().map(len)` per write: O(distinct
/// inodes ever written) per buffered write — p6_append measured 4.9 -> 27+
/// µs as the map filled (a probe pass that had already written thousands
/// of files paid ~50 µs per 4 KiB append). The counter is exact, not an
/// estimate: insert adds the NEW indices, releases subtract what they drop.
static DELALLOC_TOTAL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Reserve file blocks [first, first+n) for (fs, ino)'s pending
/// delayed-allocation writeback. Fails -ENOSPC when the filesystem's free
/// blocks (estimated — see est_free_blocks) cannot cover the reservations
/// already held plus the NEW indices of this range (already-reserved
/// indices are free — a second write over the same range owes nothing
/// new). Call BEFORE any page is dirtied: a refusal must leave the file
/// and the size watermark untouched. `free_est` is the caller's
/// est_free_blocks() snapshot (the eager gate's — one estimate per write,
/// never two group-descs walks).
///
/// Run shape: the sequential-append common case is ONE tail-extend of the
/// last run (no tree walk, one Vec store); a straddling write merges into
/// its covering runs in place.
pub(crate) fn da_reserve_blocks(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ino: u32,
    first: u64,
    n: u64,
    free_est: u64,
) -> Result<(), i32> {
    if n == 0 {
        return Ok(());
    }
    let free = free_est;
    let end = first.saturating_add(n);
    let key = (fs as *const _ as u64, ino as u64);
    let mut m = DELALLOC_RESERVATIONS.lock();
    let reserved_total = DELALLOC_TOTAL.load(core::sync::atomic::Ordering::Relaxed);
    let runs = m.entry(key).or_default();
    // Fast path: extend the last run when contiguous (sequential append).
    if let Some(last) = runs.last_mut() {
        if last.0 + last.1 == first {
            // Fully new coverage (the run ends exactly at `first`).
            if free != 0 && free <= reserved_total + n && n > 0 {
                return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
            }
            last.1 += n;
            DELALLOC_TOTAL.store(reserved_total + n, core::sync::atomic::Ordering::Relaxed);
            return Ok(());
        }
    }
    // General path: count the UNCOVERED portion of [first, end), then
    // splice the range in, merging with every run it touches.
    let mut covered: u64 = 0;
    let mut lo = first;
    for &(s, l) in runs.iter() {
        let e = s + l;
        if e <= first {
            continue;
        }
        if s >= end {
            break;
        }
        let ov_s = s.max(first);
        let ov_e = e.min(end);
        covered += ov_e.saturating_sub(ov_s);
        lo = lo.min(s);
    }
    let new = n - covered.min(n);
    if free != 0 && free <= reserved_total + new {
        // Reservations (held + this ask) exhaust the estimated free
        // space: refuse at write time — the reference's
        // ext4_da_reserve_space ENOSPC. (free == 0 is the eager gate's
        // case; already refused by the caller.)
        if new > 0 {
            return Err(errno::Errno::NoSpaceLeftOnDevice.as_neg_i32());
        }
    }
    if new == 0 && covered == n {
        return Ok(()); // entirely already reserved
    }
    // Merge: drop the runs overlapped by [first, end), remember the
    // union's bounds, insert one merged run at the right position.
    let mut hi = end;
    runs.retain(|&(s, l)| {
        let e = s + l;
        let keep = e <= first || s >= end;
        if !keep {
            lo = lo.min(s);
            hi = hi.max(e);
        }
        keep
    });
    // Insert sorted by start.
    let pos = runs.partition_point(|&(s, _)| s < lo);
    runs.insert(pos, (lo, hi - lo));
    // The merged run covers `new` blocks beyond what the dropped runs
    // already held — the counter moves by exactly the newly covered
    // blocks (math: (hi - lo) - merged_extra == new).
    DELALLOC_TOTAL.store(reserved_total + new, core::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// Release every reservation of (fs, ino) below `upto` (exclusive) — the
/// writeback allocated real blocks for those indices
/// (allocate_blocks_for_file / allocate_blocks_unwritten_dio succeeded).
pub(crate) fn da_release_below(fs: &crate::fs::ext4::Ext4FileSystem, ino: u32, upto: u64) {
    let key = (fs as *const _ as u64, ino as u64);
    let mut m = DELALLOC_RESERVATIONS.lock();
    if let Some(runs) = m.get_mut(&key) {
        let mut freed = 0u64;
        runs.retain(|&(s, l)| {
            let e = s + l;
            if e <= upto {
                freed += l;
                false
            } else if s < upto {
                freed += upto - s;
                true // straddler: head freed, tail clipped below
            } else {
                true
            }
        });
        // Clip the straddler's head (retain cannot mutate elements).
        for r in runs.iter_mut() {
            if r.0 < upto && r.0 + r.1 > upto {
                let cut = upto - r.0;
                *r = (upto, r.1 - cut);
            }
        }
        if runs.is_empty() {
            m.remove(&key);
        }
        if freed > 0 {
            DELALLOC_TOTAL.fetch_sub(freed, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Release every reservation of (fs, ino) at or past `from` — a
/// truncate-down discards the tail's pending writeback obligations.
pub(crate) fn da_release_from(fs: &crate::fs::ext4::Ext4FileSystem, ino: u32, from: u64) {
    let key = (fs as *const _ as u64, ino as u64);
    let mut m = DELALLOC_RESERVATIONS.lock();
    if let Some(runs) = m.get_mut(&key) {
        let mut freed = 0u64;
        runs.retain(|&(s, l)| {
            if s >= from {
                freed += l;
                false
            } else if s + l > from {
                freed += s + l - from;
                true // straddler clipped below
            } else {
                true
            }
        });
        for r in runs.iter_mut() {
            let e = r.0 + r.1;
            if r.0 < from && e > from {
                *r = (r.0, from - r.0);
            }
        }
        if runs.is_empty() {
            m.remove(&key);
        }
        if freed > 0 {
            DELALLOC_TOTAL.fetch_sub(freed, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Release all reservations of (fs, ino) — the inode is being deleted.
pub(crate) fn da_release_all(fs: &crate::fs::ext4::Ext4FileSystem, ino: u32) {
    let key = (fs as *const _ as u64, ino as u64);
    let mut m = DELALLOC_RESERVATIONS.lock();
    if let Some(runs) = m.remove(&key) {
        let held: u64 = runs.iter().map(|&(_, l)| l).sum();
        DELALLOC_TOTAL.fetch_sub(held, core::sync::atomic::Ordering::Relaxed);
    }
}

fn ext4_sync_file_inner(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
) -> Result<(), i32> {
    // Big-lock bracket for the whole durability point (paired-test item
    // 3): the batched windows below DMA straight from pinned buffer
    // bytes, which must not change under a racing write core mid-window
    // — the same serialization SpanFlusher::drain applies to its settle.
    // Task-recursive: the flush/write machinery re-entering is legal.
    let _sync_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock();
    // Durability point for THIS task's own publications only (per-cookie
    // scoped flush). fsync(F) must persist F's data and the metadata
    // needed to retrieve it — and, when the caller is the file's creator,
    // its still-pending directory-entry publication (pre blocks, then the
    // entry block). Other tasks' pending publications are not this
    // fsync's obligation: the old whole-queue drain here (compounded by
    // the commit's and the tail sync_buffers) made fdatasync latency
    // O(global pending publications), and the drain's racing-publisher
    // loop could pin an fsync for as long as creators kept appending
    // (sc310's 43-min hiview fdatasync; the 1.2-1.4 s accesstoken WAL
    // syncs). Ordering for whenever those foreign entry blocks DO reach
    // disk is enforced per-block at every write site (guards on
    // sync_dirty_buffer / eviction / commit write-through).
    let _ = bio::flush_publications_scoped();

    // RUN-BASED data sync (paired-test item 3, 2026-11-17): the old
    // shape materialized the WHOLE file map (one extent lookup per block,
    // a Vec per fsync) then walked every block with a cache probe. For an
    // extent file, ONE extent-tree walk per run resolves the whole span
    // (an 8 MiB file is 1-3 extents, not 2048 lookups); absent-durable
    // blocks skip without a pin; the surviving dirty blocks go out in
    // ONE batched window (bio::sync_dirty_blocks_batched — a single
    // device drain) instead of one serialized round trip each. This is
    // the measured 691 us -> sub-200 us fsync class on the fio
    // write+fsync row. Indirect-mapped files keep the old per-block
    // walk (get_data_blocks handles the indirect chains).
    // REGULAR FILES: dirty-NOTE-driven walk (O(dirty), the reference's
    // tagged-page-tree shape — see DIRTY_DATA_BLOCKS above). The old
    // extent walk probed every block of the file per fsync (2048 probes
    // for an 8 MiB file, 80 us, finding nothing after the flush pass).
    // Non-regular inodes (directories: entry blocks dirtied by namei,
    // never noted) keep the full walk below.
    if inode.mode & 0o170000 == 0o100000 {
        let notes = take_dirty_data_notes(fs as *const _ as u64, inode.ino as u64);
        let mut batch: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        let mut still_dirty: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        let mut walk_err: Option<i32> = None;
        for blk in notes {
            // SAFETY: fs.device is the fs's registered GenDisk (probe only).
            if bio::block_dirty_cached(fs.device, blk) {
                batch.push(blk);
            }
            if batch.len() >= 4096 {
                let rc = bio::sync_dirty_blocks_batched(fs.device, &batch, false);
                if rc != 0 {
                    still_dirty.extend_from_slice(&batch);
                    if walk_err.is_none() {
                        walk_err = Some(rc);
                    }
                }
                batch.clear();
            }
        }
        if !batch.is_empty() {
            let rc = bio::sync_dirty_blocks_batched(fs.device, &batch, false);
            if rc != 0 {
                still_dirty.extend_from_slice(&batch);
                if walk_err.is_none() {
                    walk_err = Some(rc);
                }
            }
        }
        settle_dirty_data_notes(fs as *const _ as u64, inode.ino as u64, &still_dirty);
        if let Some(e) = walk_err {
            return Err(e);
        }
    } else if inode.has_extent() {
        const WALK_CHUNK: usize = 4096;
        let bs = fs.block_size as u64;
        let total_blocks = (inode.get_size() + bs - 1) / bs;
        let mut batch: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        let mut blk: u64 = 0;
        let mut walk_err: Option<i32> = None;
        while blk < total_blocks {
            let (phys, avail, _uw) = match crate::fs::ext4::extent::ext4_ext_get_run(
                fs,
                inode.ino,
                &inode.block,
                blk,
            ) {
                Ok(v) => v,
                Err(e) => {
                    walk_err = Some(e);
                    break;
                }
            };
            if phys == 0 {
                blk += avail.max(1);
                continue;
            }
            let n = core::cmp::min(avail, total_blocks - blk);
            // DIRTY-ONLY FILTER: a block whose buffer is absent (its
            // eviction synced it) or clean (already persisted) owes the
            // fsync nothing. The no-pin `block_dirty_cached` probe
            // replaces the old absent-check + bread pin of EVERY cached
            // block — 2048 hash+LRU pins per fsync of an 8 MiB file was
            // the 3.9M breads/s CPU burn (fsw br=59M on the test suite row).
            // SAFETY: fs.device is the fs's registered GenDisk.
            for k in 0..n {
                if bio::block_dirty_cached(fs.device, phys + k) {
                    batch.push(phys + k);
                }
            }
            blk += n;
            if batch.len() >= WALK_CHUNK {
                let rc = bio::sync_dirty_blocks_batched(fs.device, &batch, false);
                batch.clear();
                if rc != 0 {
                    return Err(rc);
                }
            }
        }
        if let Some(e) = walk_err {
            return Err(e);
        }
        if !batch.is_empty() {
            let rc = bio::sync_dirty_blocks_batched(fs.device, &batch, false);
            if rc != 0 {
                return Err(rc);
            }
        }
    } else {
        // Sync all data blocks of file (indirect-mapped shape)
        let blocks = inode.get_data_blocks(fs)?;

        for block in blocks {
            if block == 0 {
                continue; // sparse hole
            }
            // SAFETY: fs.device is the fs's registered GenDisk.
            if bio::block_absent_durable(fs.device, block) {
                continue;
            }
            {
                let bh = bio::bread(fs.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let sync_res = if bio::bh_is_dirty(&bh) {
                    bio::sync_dirty_buffer(bh)
                } else {
                    Ok(())
                };
                bio::brelse(bh);
                sync_res?;
            }
        }
    }


    // Sync the inode-table block: the inode's own durability point. The
    // write paths leave it in one of three states — already synced
    // (eager write-through outside a journal handle), registered in the
    // running transaction (the forced commit below persists it), or dirty
    // AND unregistered (standalone updates outside any handle, e.g. the
    // O_TRUNC size rewrite of open(O_CREAT|O_TRUNC) — size is metadata
    // fsync must persist). Only the third state needs help; syncing the
    // block is idempotent for the other two.
    //
    // Pair-test suite item 3: the no-pin dirty probe skips the bread pin on
    // the common clean path; a dirty table block rides the SAME batched
    // window the data walk used (one drain) instead of its own serial
    // round trip.
    if let Some(tbl_blk) = super::inode::inode_table_block_of(fs, inode.ino) {
        // SAFETY: fs.device is the fs's registered GenDisk (probe only).
        if bio::block_dirty_cached(fs.device, tbl_blk) {
            let rc = bio::sync_dirty_blocks_batched(
                fs.device,
                core::slice::from_ref(&tbl_blk),
                false,
            );
            if rc != 0 {
                return Err(rc);
            }
        }
    }

    // fsync semantics (review 5.5: ext4_sync_file/fsync 桩): with a journal
    // present, durability requires the current transaction to COMMIT — the
    // journal superblock's s_start then advances past this update, so a
    // crash cannot replay older metadata over it. Without a journal the
    // per-block sync above already wrote everything through.
    if let Some(journal) = fs.journal.clone() {
        let mut handle = super::journal::ext4_journal_start(fs, 0)?;
        // h_sync: fsync must force the lazy-batched transaction to commit
        // now (jbd2_journal_stop otherwise leaves it open for batching) —
        // the commit's write-through fast path persists the registered
        // inode-table blocks, making the fsynced state durable.
        handle.h_sync = true;
        let res = super::journal::ext4_journal_stop(&mut handle);
        let _ = journal; // Journal Arc retained for clarity
        res?;
    } else {
        // Route-1 (leg 52, item 3): with the null-journal handle armed in
        // the write path, a journal-less fs defers the allocation
        // metadata (block bitmaps, group descriptors, indirect/extent
        // tree blocks) the old "synced eagerly at its write" text above
        // assumed. fsync(F) must persist the metadata needed to
        // RETRIEVE F's data — that set is: F's data blocks (synced
        // above), F's inode slot (synced above), and, for
        // INDIRECT-mapped files, the indirect-chain blocks that map
        // them (synced below). Depth-0 extent files keep their mapping
        // in the inode itself — nothing extra. Allocation bitmaps and
        // group descriptors are NOT part of F's reachability: they are
        // recomputable by e2fsck (the documented F4-R2 stance; Linux's
        // own no-journal mounts leave them to writeback) and ride
        // sync(2) / buffer-cache eviction.
        //
        // (The first cut of this arm flushed the whole device's dirty
        // set under EXT4_BIG_LOCK — WRONG twice: O(world-dirty) per
        // fsync is the sc310 anti-pattern, and the cache walk faulted
        // on a concurrently-released BufferHead (pa92nj crash chain
        // sync_all_excluding -> is_dirty -> state lock at null+0x30).
        // The per-file set below walks no shared cache.)
        if !inode.has_extent() {
            // SAFETY: bio::bread returns valid BufferHeads; the indirect
            // blocks are plain metadata blocks of this inode.
            {
                let mut chain = [0u64; 2];
                if inode.block[12] != 0 {
                    chain[0] = inode.block[12] as u64;
                }
                if inode.block[13] != 0 {
                    chain[1] = inode.block[13] as u64;
                }
                for top in chain.iter() {
                    if *top == 0 {
                        continue;
                    }
                    let bh = match bio::bread(fs.device, *top) {
                        Some(b) => b,
                        None => continue,
                    };
                    // Sync the top-level indirect block itself.
                    if bio::bh_is_dirty(&bh) {
                        let _ = bio::sync_dirty_buffer(bh);
                    }
                    // Double-indirect: sync each pointed single-indirect
                    // block (they hold the data pointers).
                    if *top == inode.block[13] as u64 {
                        let ppc = (fs.block_size / 4) as usize;
                        for i in 0..ppc {
                            let v = {
                                let src = bio::bh_data(&bh);
                                let mut b4 = [0u8; 4];
                                let end = (i * 4 + 4).min(src.len());
                                if i * 4 < end {
                                    b4[..end - i * 4]
                                        .copy_from_slice(&src[i * 4..end]);
                                }
                                u32::from_le_bytes(b4) as u64
                            };
                            if v != 0 {
                                if let Some(sb) = bio::bread(fs.device, v) {
                                    if bio::bh_is_dirty(&sb) {
                                        let _ = bio::sync_dirty_buffer(sb);
                                    }
                                    bio::brelse(sb);
                                }
                            }
                        }
                    }
                    bio::brelse(bh);
                }
            }
        }
    }

    // NO whole-cache flush here (the old bio::sync_buffers() tail). It
    // existed to catch unregistered metadata left dirty by the lazy
    // write-back, but none of it belongs to THIS fsync:
    // - F's data blocks: synced above.
    // - F's inode slot: synced above (and by the forced commit).
    // - F's extent/bitmap updates: each was either synced eagerly at its
    //   write (no journal handle active) or registered in a transaction
    //   the forced commit above persists.
    // - F's directory entry: a pending ordered publication; POSIX leaves
    //   entry durability to fsync(dirfd). Whenever any writer does flush
    //   it, the per-block guards order it after its prerequisites.
    // - Other files' dirty metadata: never fsync(F)'s obligation; the
    //   buffer-cache write-back (eviction sync) and sync(2) own them.
    // Dropping the tail removes fsync's O(world-dirty-blocks) cost.

    Ok(())
}

// ============================================================================
// VFS Wrapper Functions
// ============================================================================

/// INV-EXT4-FILERES (unsafe-slim r3c): the ONE audited resolution of a
/// VFS File to its ext4 (VFS-inode, fs, cached-inode) triple. All three
/// raw fields ride the same open-time contract: file.inode is installed
/// once at ext4_open and never re-pointed for the file's lifetime; the
/// VFS Inode's private_data (Ext4FileSystem) and sb (Box'd Ext4Inode)
/// are fixed when the inode is instantiated. Every VFS wrapper in this
/// file used to re-deref all three inside its own unsafe wrapper (and
/// re-state the same contract); they all ride this accessor now.
fn file_ext4(
    file: &File,
) -> Option<(
    &Inode,
    &crate::fs::ext4::Ext4FileSystem,
    &super::inode::Ext4Inode,
)> {
    // The inode cell rides INV-FS-FILEINODE-1; private_data rides
    // INV-FS-PRIVDATA-1 (the ext4 instance installed at create_vfs_inode);
    // the sb box rides INV-EXT4-SBRES-1 (INV-EXT4-FILERES above).
    let inode = crate::fs::unsafe_edges::file_inode(file)?;
    let fs = crate::fs::unsafe_edges::privdata_ref::<crate::fs::ext4::Ext4FileSystem>(inode)?;
    let ext4_ino = super::sb_inode_of(inode).ok()?;
    Some((inode, fs, ext4_ino))
}

/// (INV-EXT4-FILERES) safe twin for the EBADF/EIO split: is the file's
/// VFS inode slot empty at all?
fn file_inode_missing(file: &File) -> bool {
    // Same slot contract as file_ext4 (INV-FS-FILEINODE-1).
    crate::fs::unsafe_edges::file_inode(file).is_none()
}

/// VFS read wrapper - calls ext4_file_read_cached with page cache and read-ahead
pub fn ext4_file_read_vfs(file: &File, buf: &mut [u8]) -> isize {
    let Some((inode, fs, ext4_inode)) = file_ext4(file) else {
        // Resolution distinguishes no-inode (EBADF) from no-fs/sb (EIO).
        if file_inode_missing(file) {
            return errno::Errno::BadFileNumber.as_neg_i32() as isize;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };

    // Serialize the whole get_pos → read → set_pos sequence against
    // other readers/writers sharing this open file description
    // (fork-inherited fds): without the lock, two CPUs can both read
    // at the old pos and both store pos+1 — a lost offset update
    // (LTP fork07: parent's EOF read() returned 1). Linux calls this
    // f_pos_lock. Sleeping mutex — the read may block on disk I/O.
    let _pos_guard = file.io_lock.guard();

    // Get current file position
    let offset = file.get_pos() as u64;

    // P2 O_DIRECT: bypass the page cache entirely and serve straight
    // from the block layer (ext4_file_read walks bio::bread). The
    // write side already runs uncached-synchronous (ext4_file_write),
    // so only the read path needs the branch. Simplification vs Linux:
    // offset/length block alignment is not enforced.
    if file.flags_bits() & crate::fs::file::FileFlags::O_DIRECT != 0 {
        // Reference parity (generic_file_read_iter's DIO leg):
        // filemap_write_and_wait_range BEFORE the direct read — a
        // buffered writer's dirty page-cache frames must reach the
        // disk this read serves from. Since the delayed-allocation
        // change, buffered write(2) leaves data ONLY in dirty frames
        // (see sys_sync), so without this flush a direct read of a
        // just-buffered-written range served stale blocks (LTP
        // diotest2/5 testblock 1: buffered write + O_DIRECT read,
        // read/write comparison failed).
        if !buf.is_empty() {
            let ps = crate::mm::PAGE_SIZE as u64;
            let first_index = offset / ps;
            let last_index = (offset + buf.len() as u64 - 1) / ps;
            super::file::ext4_flush_dirty_inode(inode, first_index, last_index);
        }
        match ext4_file_read(fs, ext4_inode, offset, buf) {
            Ok(read_bytes) => {
                file.set_pos(offset + read_bytes as u64);
                // No IN_ACCESS here: this runs as ops.read behind
                // File::read, whose wrapper owns the notify. Op-level
                // self-notify double-fired the event (hidden by the
                // inotify last-event merge, but a duplicate surfaced
                // whenever another watcher event interleaved — review
                // CR-3). Only the read(2) FAST path below self-notifies
                // (it bypasses File::read entirely).
                read_bytes as isize
            }
            Err(e) => e as isize,
        }
    } else {
        // Get or create read-ahead state from file.private_data
        let ra_state = get_or_create_ra_state(file, fs.block_size as u64);

        // Call cached read function
        match ext4_file_read_cached(fs, ext4_inode, offset, buf, ra_state) {
            Ok(read_bytes) => {
                file.set_pos(offset + read_bytes as u64);
                // atime: keep the CACHED inode in sync with the read,
                // gated by the reference-default relatime rule
                // (stamp_atime_relatime doc).
                if read_bytes > 0 {
                    // INV-EXT4-ATIME: the cached Ext4Inode in inode.sb is
                    // a Box owned by this VFS inode; the io_lock above
                    // serializes writers, and readers of atime tolerate
                    // torn u32 values (advisory field). stamp_atime_
                    // relatime is the audited write span.
                    stamp_atime_relatime(inode.sb.unwrap() as *mut super::inode::Ext4Inode);
                }
                read_bytes as isize
            }
            Err(e) => e as isize,
        }
    }
}

/// read(2) fast path for ext4 regular files: copy straight from the page
/// cache into user memory (no syscall-layer staging buffer).
///
/// `dst` must be access_ok-validated for `count` bytes. O_DIRECT falls back
/// to the caller's staged path (rare, unaligned semantics kept simple).
pub fn ext4_file_read_user_vfs(file: &File, dst: *mut u8, count: usize) -> isize {
    // rux:filerd (retired dfx=filerd): outer bracket for the whole fast path (the lock /
    // pin / copy / unpin / epilog phases are charged inside; the
    // residue vs their sum is resolution + ra_state + loop scaffolding).
    let probe = crate::dfx::filerd::on();
    let t_outer = if probe { crate::dfx::filerd::now() } else { 0 };
    let Some((inode, fs, ext4_inode)) = file_ext4(file) else {
        if file_inode_missing(file) {
            return errno::Errno::BadFileNumber.as_neg_i32() as isize;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };
    // Same pos-race serialization as ext4_file_read_vfs (LTP fork07).
    let t_lock = if probe { crate::dfx::filerd::now() } else { 0 };
    let _pos_guard = file.io_lock.guard();
    let offset = file.get_pos() as u64;
    if probe {
        crate::dfx::filerd::charge(&crate::dfx::filerd::LOCK_CYCLES, t_lock);
    }

    let ra_state = get_or_create_ra_state(file, fs.block_size as u64);
    match ext4_file_read_cached_dst(
        fs, ext4_inode, offset,
        ReadDst::User(dst, count), ra_state,
    ) {
        Ok(read_bytes) => {
            let t_ep = if probe { crate::dfx::filerd::now() } else { 0 };
            file.set_pos(offset + read_bytes as u64);
            if read_bytes > 0 {
                // atime via the reference-default relatime gate
                // (stamp_atime_relatime doc).
                // INV-EXT4-ATIME: same contract as ext4_file_read_vfs
                // (advisory field, io_lock serializes writers).
                stamp_atime_relatime(inode.sb.unwrap() as *mut super::inode::Ext4Inode);
                // inotify IN_ACCESS, same as the staged read path below
                // (line ~1240): this fast path bypasses File::read, so
                // without an explicit notify every read(2) on an ext4
                // file dropped the event (LTP inotify01: the whole
                // expected-event train shifted by one).
                crate::fs::inotify::notify_file(
                    file,
                    crate::fs::inotify::inotify_events::IN_ACCESS,
                );
            }
            if probe {
                crate::dfx::filerd::charge(&crate::dfx::filerd::EPILOG_CYCLES, t_ep);
                crate::dfx::filerd::charge(&crate::dfx::filerd::OUTER_CYCLES, t_outer);
                crate::dfx::filerd::OUTER_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            read_bytes as isize
        }
        Err(e) => {
            if probe {
                crate::dfx::filerd::charge(&crate::dfx::filerd::OUTER_CYCLES, t_outer);
                crate::dfx::filerd::OUTER_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            e as isize
        }
    }
}

/// True when `file` uses the ext4 regular-file ops (fast-path dispatch for
/// the read(2) syscall layer).
pub fn is_ext4_file(file: &File) -> bool {
    file.get_ops()
        .is_some_and(|ops| core::ptr::eq(ops as *const _, &EXT4_FILE_OPS as *const _))
}

// ==================== GUP-pinned O_DIRECT reads (GUP-DIO) ====================
//
// The reference kernel's DIO shape (bio_iov_iter_get_pages +
// iomap_dio_bio_iter): the user pages backing the request are PINNED and
// the device DMAs straight into them — the syscall staging buffer and
// its full-data copy (the hotspot-diff +34.5 µs/op copyout term on the
// 1 MiB direct read) disappear. Per-page fallback discipline: pages
// that cannot be pinned are holes in the pin; blocks touching them are
// served through the per-block body (bread + copy_to_user — exactly
// the staged path's bytes-for-bytes semantics).

/// Minimum request size that takes the GUP read path: below it the pin
/// walk (a PTE walk + refcount per page) costs as much as the staging
/// copy it saves (4 KiB random reads stage 4 KiB — the copy is one
/// cache-line memcpy class).
pub const GUP_DIO_READ_MIN: usize = 64 * 1024;

/// GUP O_DIRECT read core. `pins` backs `[va, va+to_read)`. Returns
/// None when the geometry/device makes the batched direct reader
/// unusable (the caller falls back to the staged path untouched).
fn ext4_dio_read_gup_core(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    pins: &crate::mm::gup::PinnedUserPages,
    va: usize,
    to_read: usize,
) -> Option<usize> {
    let block_size = fs.block_size as usize;
    if block_size != bio::get_block_cache().cache_block_size() as usize || !inode.has_extent() {
        return None;
    }
    // The GUP arm expresses runs only as SG chains of pinned pieces.
    if !(*fs.device).has_async_read_sg() {
        return None;
    }
    let mut dst = DioReadDst::User { pins, va };
    ext4_dio_read_batched(fs, inode, offset, &mut dst, to_read, block_size)
}

/// read(2) O_DIRECT GUP entry: pin the user window, DMA into the
/// frames, per-page fallback for pin holes. Returns None when the GUP
/// path is not applicable (caller: staged loop unchanged). `dst` must
/// be access_ok-validated for `count`.
pub fn ext4_file_read_dio_user_vfs(file: &File, dst: *mut u8, count: usize) -> Option<isize> {
    let (_, fs, ext4_inode) = file_ext4(file)?;
    // The device must express SG chains before anything else.
    if !fs.device.has_async_read_sg() {
        return None;
    }
    // Pin BEFORE the io_lock (the fault-in may sleep/allocate).
    let pins = crate::mm::gup::pin_user_pages(dst as usize, count, false)?;

    let _pos_guard = file.io_lock.guard();
    let offset = file.get_pos() as u64;
    let file_size = ext4_inode.get_size();
    let to_read = core::cmp::min(count as u64, file_size.saturating_sub(offset)) as usize;
    let n = match ext4_dio_read_gup_core(fs, ext4_inode, offset, &pins, dst as usize, to_read) {
        Some(n) => n,
        None => return None, // staged fallback owns the whole call
    };
    if n == 0 && to_read > 0 {
        // Nothing delivered of a non-empty request: the staged twin
        // reports the failing errno (a faulted user window = EFAULT,
        // a device failure = EIO). A pin hole at the first byte is
        // the EFAULT face; anything else that zeroed the read is
        // the EIO face.
        return Some(if !pins.range_pinned(0, 1) {
            errno::Errno::BadAddress.as_neg_i32() as isize
        } else {
            errno::Errno::IOError.as_neg_i32() as isize
        });
    }
    if n > 0 {
        file.set_pos(offset + n as u64);
        // atime + IN_ACCESS: same discipline as ext4_file_read_user_vfs
        // (INV-EXT4-ATIME: the sb Box lives for the inode; io_lock
        // serializes; stamp_atime_relatime is the audited write span).
        stamp_atime_relatime(
            ext4_inode as *const super::inode::Ext4Inode as *mut super::inode::Ext4Inode,
        );
        crate::fs::inotify::notify_file(file, crate::fs::inotify::inotify_events::IN_ACCESS);
    }
    Some(n as isize)
}

/// pread64(2) O_DIRECT GUP entry — the read(2) twin at an explicit
/// offset (fio's psync engine preads). Same contract: None = use the
/// staged path.
pub fn ext4_file_pread_dio_user_vfs(
    file: &File,
    offset: u64,
    dst: *mut u8,
    count: usize,
) -> Option<isize> {
    let (_, fs, ext4_inode) = file_ext4(file)?;
    if !fs.device.has_async_read_sg() {
        return None;
    }
    let pins = crate::mm::gup::pin_user_pages(dst as usize, count, false)?;

    let _io_guard = file.io_lock.guard();
    let file_size = ext4_inode.get_size();
    let to_read = core::cmp::min(count as u64, file_size.saturating_sub(offset)) as usize;
    let n = match ext4_dio_read_gup_core(fs, ext4_inode, offset, &pins, dst as usize, to_read) {
        Some(n) => n,
        None => return None,
    };
    if n == 0 && to_read > 0 {
        // Same errno discipline as the read(2) entry (staged twin).
        return Some(if !pins.range_pinned(0, 1) {
            errno::Errno::BadAddress.as_neg_i32() as isize
        } else {
            errno::Errno::IOError.as_neg_i32() as isize
        });
    }
    if n > 0 {
        // atime (advisory, same as ext4_file_read_at_vfs_inner —
        // INV-EXT4-ATIME); no IN_ACCESS here — File::read_at's native-op
        // arm owns the notify on this path's staged twin, and the
        // syscall layer dispatches here INSTEAD of it, so mirror the
        // entry it replaces (none fired for the O_DIRECT arm).
        let atime = crate::drivers::rtc::wall_secs() as u32;
        // Audited advisory-field store (INV-EXT4-ATIME); the sb Box
        // is exclusively ours while io_lock is held. Volatile by the
        // same discipline as stamp_atime_relatime.
        {
            let i = crate::syscall::edges::boxed_mut(
                ext4_inode as *const super::inode::Ext4Inode
                    as *mut super::inode::Ext4Inode,
            );
            // INV-EXT4-ATIME — one volatile store of the advisory u32;
            // readers tolerate torn values.
            crate::safety::bytes::store_volatile(&mut i.atime, atime);
        }
    }
    Some(n as isize)
}

/// mm filemap_fault cache warm-up (x86_64 demand faults): run ONE
/// page-shaped read through the cached read machinery with a DISCARD
/// destination — no bytes are copied to a bounce buffer; the point is the
/// side effect, `fill_page_cache_batch` populating the page cache (and the
/// fault-around detector warming the surrounding window, exactly as a
/// read_at from the old private-copy fault path did).
///
/// The caller then pins frames with `page_cache::get_phys` and maps them
/// directly (Linux filemap_fault shape). Returns false when the file is
/// not resolvable through the ext4 read path (treated as "not cacheable"
/// by the fault arm, which falls back to a private copy).
pub fn ext4_fault_populate_cache(file: &File, offset: u64) -> bool {
    let Some((_, fs, ext4_inode)) = file_ext4(file) else {
        return false;
    };
    let _io_guard = file.io_lock.guard();
    let ra_state = get_or_create_ra_state(file, fs.block_size as u64);
    matches!(
        ext4_file_read_cached_dst(
            fs,
            ext4_inode,
            offset,
            ReadDst::Discard(crate::mm::PAGE_SIZE),
            ra_state
        ),
        Ok(n) if n > 0
    )
}

/// VFS write wrapper - calls ext4_file_write
pub fn ext4_file_write_vfs(file: &File, buf: &[u8]) -> isize {
    let Some((inode, fs, _)) = file_ext4(file) else {
        if file_inode_missing(file) {
            return errno::Errno::BadFileNumber.as_neg_i32() as isize;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };
    let ext4_ino = inode.ino as u32;

    // Serialize the get_pos → write → set_pos sequence against
    // concurrent readers of the same open file description (they hold
    // no ext4 lock — see ext4_file_read_vfs). Taken BEFORE the big
    // lock so a slow file-level waiter never pins the machine-wide
    // ext4 lock (LTP fork07, Linux f_pos_lock discipline).
    let _pos_guard = file.io_lock.guard();

    // Get current file position (O_APPEND: always write at end of file)
    let offset = if file.flags().bits() & crate::fs::file::FileFlags::O_APPEND != 0 {
        // Computed inside the core, from the disk-fresh inode under the
        // big lock — an append racing on a different open description
        // must be visible before our offset is fixed.
        u64::MAX
    } else {
        file.get_pos()
    };

    // RLIMIT_FSIZE (Linux generic_write_checks — ext4 files carry their
    // own write op, so the reg_file_write twin is not enough): a write
    // starting at/after the soft limit fails EFBIG with SIGXFSZ; a
    // write that would CROSS the limit is silently truncated to the
    // boundary (LTP setrlimit01 case 2 sets the limit to 10, writes 26
    // and requires write() to return exactly 10).
    let mut buf = buf;
    if let Some(task) = crate::sched::current() {
        let (fsz_cur, _) = task.rlimit(crate::process::task::rlimit_res::FSIZE);
        if fsz_cur != u64::MAX {
            let start = if offset == u64::MAX {
                inode.size.load(core::sync::atomic::Ordering::Acquire)
            } else {
                offset
            };
            if start >= fsz_cur {
                let _ = crate::signal::send_signal(
                    task.pid() as u32,
                    crate::signal::Signal::SIGXFSZ as i32,
                );
                return crate::errno::Errno::FileTooLarge.as_neg_i32() as isize;
            }
            let allowed = (fsz_cur - start) as usize;
            if buf.len() > allowed {
                buf = &buf[..allowed];
            }
        }
    }

    // Route-1: defer the write on journal-less images unless the
    // description demands write-return durability.
    let durability = file.flags().bits()
        & (crate::fs::file::FileFlags::O_SYNC
            | crate::fs::file::FileFlags::O_DSYNC
            | crate::fs::file::FileFlags::O_DIRECT);
    // Delayed allocation (perf-gaps 2026-11): plain buffered writes
    // (no O_SYNC/O_DSYNC/O_DIRECT) take the page-cache fast path —
    // write(2) copies into (dirty) frames and defers allocation +
    // journaling to the writeback points; see ext4_buffered_write.
    let written = if durability == 0
        && inode.mode.is_regular_file()
        && !fs.read_only
    {
        // ext4_buffered_write is a safe fn (the audited write span).
        ext4_buffered_write(
            fs,
            inode,
            offset,
            WriteSrc::Kernel(buf.as_ptr(), buf.len()),
        )
    } else {
        ext4_write_at_core(
            fs,
            ext4_ino,
            Some(inode),
            offset,
            durability & (crate::fs::file::FileFlags::O_SYNC
                | crate::fs::file::FileFlags::O_DSYNC) == 0,
            buf,
            &[0],
            durability & crate::fs::file::FileFlags::O_DIRECT != 0,
        )
    };
    if written > 0 {
        let end = if offset == u64::MAX {
            inode.size.load(core::sync::atomic::Ordering::Acquire)
        } else {
            offset + written as u64
        };
        file.set_pos(end);
        // O_DIRECT write-return durability (paired-test item 1,
        // 2026-11-17): the core leaves the range's data blocks
        // BH_Dirty in the block cache; a direct write is contractually
        // on the device when write(2) returns (the reference's DIO
        // submits bios and waits). Push the written range through the
        // plugged span pipeline (ONE batched submit + drain — a 1 MiB
        // contiguous write is one span) instead of leaving 256 dirty
        // victims to serialized one-at-a-time eviction syncs (the
        // measured 10.6 ms 1 MiB DIO write class). no-op for block
        // sizes the span machinery cannot express.
        if durability & crate::fs::file::FileFlags::O_DIRECT != 0 && !fs.read_only {
            let mut stage: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut flusher = SpanFlusher::new();
            run_sync_spans(
                fs,
                inode,
                end - written as u64,
                written as usize,
                &mut stage,
                &mut flusher,
            );
            flusher.drain();
        }
    }
    written
}

/// Positional write for the ext4 file ops (FileOps::write_at, session 25):
/// pwrite(2)/msync/flush writeback need an explicit-offset write that
/// never touches the shared file position. O_APPEND is ignored per POSIX.
/// This replaces the generic File::write_at fallback (set_pos/write/
/// restore), which raced concurrent plain writes on the same open file
/// description — the write twin of the R71 read race.
///
/// SAFETY: file.inode.get() fields written at open time, read-only after.
pub fn ext4_file_write_at_vfs(file: &File, offset: u64, buf: *const u8, count: usize) -> isize {
    if file.flags().is_readonly() {
        return -9; // EBADF
    }
    let Some((inode, fs, _)) = file_ext4(file) else {
        if file_inode_missing(file) {
            return -9;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };
    // The fn's own contract — `buf` holds `count` readable bytes (the
    // caller's raw-pointer write_at boundary); INV-EXT4-WAVFS.
    let data = crate::syscall::edges::slice_of_kernel_bytes(buf, count);
    // Same routing as write(2): plain buffered writes take the
    // delayed-allocation page-cache path (pwrite/mmap-writeback flushes
    // still come through ext4_write_at_core directly).
    let durability = file.flags().bits()
        & (crate::fs::file::FileFlags::O_SYNC
            | crate::fs::file::FileFlags::O_DSYNC
            | crate::fs::file::FileFlags::O_DIRECT);
    if durability == 0 && inode.mode.is_regular_file() && !fs.read_only {
        ext4_buffered_write(
            fs,
            inode,
            offset,
            WriteSrc::Kernel(data.as_ptr(), data.len()),
        )
    } else {
        let written = ext4_write_at_core(
            fs,
            inode.ino as u32,
            Some(inode),
            offset,
            durability & (crate::fs::file::FileFlags::O_SYNC
                | crate::fs::file::FileFlags::O_DSYNC) == 0,
            data,
            &[0],
            durability & crate::fs::file::FileFlags::O_DIRECT != 0,
        );
        // O_DIRECT write-return durability — see the write(2) twin
        // (paired-test item 1): plugged span drain of the written
        // range so a direct pwrite is device-acked at return.
        if written > 0
            && durability & crate::fs::file::FileFlags::O_DIRECT != 0
            && !fs.read_only
        {
            let mut stage: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut flusher = SpanFlusher::new();
            run_sync_spans(
                fs,
                inode,
                offset,
                written as usize,
                &mut stage,
                &mut flusher,
            );
            flusher.drain();
        }
        written
    }
}

/// write(2) fast path for ext4 regular files, USER source (BUFW-2X): the
/// bytes go user→page-cache-frame in ONE copy (the syscall layer's
/// staging buffer and its second copy are skipped — the reference
/// kernel's copy_page_from_iter shape). Mirrors ext4_file_write_vfs
/// (io_lock/f_pos discipline, RLIMIT_FSIZE, routing, IN_MODIFY, set_pos);
/// `src` must be access_ok-validated for `count` bytes (write_file_core
/// contract). Durability flags route through a one-shot staging bounce
/// (rare — the syscall layer only dispatches here for plain buffered
/// writes; this is the safety net that keeps semantics identical).
pub fn ext4_file_write_user_vfs(file: &File, src: *const u8, count: usize) -> isize {
    if count == 0 {
        return 0;
    }
    let Some((inode, fs, _)) = file_ext4(file) else {
        if file_inode_missing(file) {
            return errno::Errno::BadFileNumber.as_neg_i32() as isize;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };

    // f_pos discipline (LTP fork07): serialize get_pos → write →
    // set_pos against the same description's readers/writers, taken
    // BEFORE the big lock (a slow waiter must not pin the machine-wide
    // ext4 lock). Sleeping mutex — the user copies below may fault
    // and sleep inside it (Linux holds i_rwsem across exactly this).
    let _pos_guard = file.io_lock.guard();

    let offset = if file.flags().bits() & crate::fs::file::FileFlags::O_APPEND != 0 {
        u64::MAX
    } else {
        file.get_pos()
    };

    // RLIMIT_FSIZE (generic_write_checks twin — see
    // ext4_file_write_vfs): at/after the soft limit EFBIG + SIGXFSZ;
    // crossing it truncates silently to the boundary.
    let mut wsrc = WriteSrc::User(src, count);
    if let Some(task) = crate::sched::current() {
        let (fsz_cur, _) = task.rlimit(crate::process::task::rlimit_res::FSIZE);
        if fsz_cur != u64::MAX {
            let start = if offset == u64::MAX {
                inode.size.load(core::sync::atomic::Ordering::Acquire)
            } else {
                offset
            };
            if start >= fsz_cur {
                let _ = crate::signal::send_signal(
                    task.pid() as u32,
                    crate::signal::Signal::SIGXFSZ as i32,
                );
                return crate::errno::Errno::FileTooLarge.as_neg_i32() as isize;
            }
            let allowed = (fsz_cur - start) as usize;
            if wsrc.len() > allowed {
                wsrc = wsrc.truncate(allowed);
            }
        }
    }

    let durability = file.flags().bits()
        & (crate::fs::file::FileFlags::O_SYNC
            | crate::fs::file::FileFlags::O_DSYNC
            | crate::fs::file::FileFlags::O_DIRECT);
    let written = if durability == 0 && inode.mode.is_regular_file() && !fs.read_only {
        // SAFETY: the audited ext4_buffered_write span (user-source copy
        // contract: wsrc is access_ok-validated by the syscall layer).
        ext4_buffered_write(fs, inode, offset, wsrc)
    } else {
        // Safety net (the syscall layer does not dispatch durable
        // writes here): stage the user bytes once and run the
        // write-through core, exactly like the staged path would.
        let mut staged = alloc::vec![0u8; wsrc.len()];
        // SAFETY: staged is a valid kernel destination of wsrc.len().
        let uncopied = wsrc.copy_to_frame(0, &mut staged[..]);
        if uncopied > 0 {
            let landed = staged.len() - uncopied;
            if landed == 0 {
                return errno::Errno::BadAddress.as_neg_i32() as isize;
            }
            staged.truncate(landed);
        }
        ext4_write_at_core(
            fs,
            inode.ino as u32,
            Some(inode),
            offset,
            durability & (crate::fs::file::FileFlags::O_SYNC
                | crate::fs::file::FileFlags::O_DSYNC) == 0,
            &staged,
            &[0],
            durability & crate::fs::file::FileFlags::O_DIRECT != 0,
        )
    };
    if written > 0 {
        let end = if offset == u64::MAX {
            inode.size.load(core::sync::atomic::Ordering::Acquire)
        } else {
            offset + written as u64
        };
        file.set_pos(end);
        // inotify IN_MODIFY: this fast path bypasses File::write,
        // whose wrapper owns the notify on the staged path (same
        // discipline as the read twin, LTP inotify01).
        crate::fs::inotify::notify_file(
            file,
            crate::fs::inotify::inotify_events::IN_MODIFY,
        );
    }
    written
}

/// pwrite(2) fast path for ext4 regular files, USER source (BUFW-2X): the
/// write(2) twin at an explicit offset — ONE user→frame copy per page, no
/// syscall-layer staging. Same routing and IN_MODIFY discipline as
/// ext4_file_write_at_vfs (which the staged path uses); `src` must be
/// access_ok-validated for `count` bytes. Durability flags bounce through
/// a one-shot staging copy (the write-through core takes a kernel slice).
pub fn ext4_file_pwrite_user_vfs(file: &File, offset: u64, src: *const u8, count: usize) -> isize {
    if count == 0 {
        return 0;
    }
    let Some((inode, fs, _)) = file_ext4(file) else {
        if file_inode_missing(file) {
            return errno::Errno::BadFileNumber.as_neg_i32() as isize;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };

    let durability = file.flags().bits()
        & (crate::fs::file::FileFlags::O_SYNC
            | crate::fs::file::FileFlags::O_DSYNC
            | crate::fs::file::FileFlags::O_DIRECT);
    let written = if durability == 0 && inode.mode.is_regular_file() && !fs.read_only {
        // SAFETY: the audited ext4_buffered_write span (user-source copy
        // contract: wsrc is access_ok-validated by the syscall layer).
        ext4_buffered_write(fs, inode, offset, WriteSrc::User(src, count))
    } else {
        // Safety net (the syscall layer does not dispatch durable
        // pwrites here): stage once, run the write-through core —
        // exactly the staged path's shape.
        let mut staged = alloc::vec![0u8; count];
        // SAFETY: staged is a valid kernel destination of count bytes.
        let uncopied =
            WriteSrc::User(src, count).copy_to_frame(0, &mut staged[..count]);
        if uncopied > 0 {
            let landed = count - uncopied;
            if landed == 0 {
                return errno::Errno::BadAddress.as_neg_i32() as isize;
            }
            staged.truncate(landed);
        }
        ext4_write_at_core(
            fs,
            inode.ino as u32,
            Some(inode),
            offset,
            durability & (crate::fs::file::FileFlags::O_SYNC
                | crate::fs::file::FileFlags::O_DSYNC) == 0,
            &staged,
            &[0],
            durability & crate::fs::file::FileFlags::O_DIRECT != 0,
        )
    };
    if written > 0 {
        // inotify IN_MODIFY: the staged path's File::write_at wrapper
        // owns the notify; this fast path bypasses it (same discipline
        // as the read twin, LTP inotify01).
        crate::fs::inotify::notify_file(
            file,
            crate::fs::inotify::inotify_events::IN_MODIFY,
        );
    }
    written
}

// ==================== GUP-pinned O_DIRECT writes (GUP-DIO) ====================
//
// The write twin of the pinned read path: pin the user pages (WRITE
// faults — COW-resolved, the reference's FOLL_WRITE semantics), DMA the
// full-block runs straight FROM the frames, per-block fallback for
// partial/cached/hole blocks. The staged path's full-buffer copy (the
// hotspot-diff ~56 µs/op term on the 1 MiB direct write) disappears.

/// Minimum request size that takes the GUP write path (see
/// GUP_DIO_READ_MIN).
pub const GUP_DIO_WRITE_MIN: usize = 64 * 1024;

/// GUP O_DIRECT write core shared by write(2)/pwrite(2) entries.
/// Returns None when the request must go through the staged path
/// (device lacks SG chains, geometry unusable, or ANY user page could
/// not be pinned — a pin hole would force copy_from_user under the ext4
/// big lock, which the staged path never does; holes are exactly the
/// EFAULT/OOM shapes where the staged path's semantics are the law).
/// `count` here is the (already RLIMIT-clipped) byte count.
#[allow(clippy::too_many_arguments)]
fn ext4_dio_write_gup_run(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: Option<&crate::fs::inode::Inode>,
    ext4_ino: u32,
    offset: u64,
    src: *const u8,
    count: usize,
) -> Option<isize> {
    // SG chains are the only zero-copy expression of discontiguous
    // frames; without them the staged path is the shape.
    if !(*fs.device).has_async_write_sg() {
        return None;
    }
    if fs.block_size != 4096 {
        return None;
    }
    // Pin BEFORE any ext4 lock (faults may sleep/allocate).
    let pins = crate::mm::gup::pin_user_pages(src as usize, count, true)?;
    if pins.pinned_pages() != pins.page_count() {
        // Pin hole: request-granularity fallback (module doc above).
        return None;
    }
    // fs/inode resolution mirrors ext4_file_write_user_vfs; the core
    // owns locking; the pins outlive the call (dropped at scope end —
    // after the core's drains device-acked every span). The core is a
    // safe fn (unsafe-slim L4).
    let written = ext4_write_at_core_src(
        fs,
        ext4_ino,
        inode,
        offset,
        true,
        DioWriteSrc::User {
            pins: &pins,
            va: src as usize,
        },
        &[0],
        true,
    );
    Some(written)
}

/// write(2) O_DIRECT GUP entry. None = use the staged path unchanged.
/// `src` must be access_ok-validated for `count`.
pub fn ext4_file_write_dio_user_vfs(file: &File, src: *const u8, count: usize) -> Option<isize> {
    let (inode, fs, _) = file_ext4(file)?;
    if !inode.mode.is_regular_file() {
        return None;
    }
    if fs.read_only {
        return None;
    }

    // RLIMIT_FSIZE clip (generic_write_checks twin — see
    // ext4_file_write_user_vfs): at/after the limit EFBIG+SIGXFSZ,
    // crossing truncates to the boundary.
    let mut count = count;
    let offset = if file.flags().bits() & crate::fs::file::FileFlags::O_APPEND != 0 {
        u64::MAX
    } else {
        file.get_pos()
    };
    if let Some(task) = crate::sched::current() {
        let (fsz_cur, _) = task.rlimit(crate::process::task::rlimit_res::FSIZE);
        if fsz_cur != u64::MAX {
            let start = if offset == u64::MAX {
                inode.size.load(core::sync::atomic::Ordering::Acquire)
            } else {
                offset
            };
            if start >= fsz_cur {
                let _ = crate::signal::send_signal(
                    task.pid() as u32,
                    crate::signal::Signal::SIGXFSZ as i32,
                );
                // The staged twin reports EFBIG; keep the observable
                // behavior by NOT falling back (the staged path would
                // re-send the signal and EFBIG identically — hand the
                // verdict back through the None contract's sibling:
                // report it directly here).
                return Some(errno::Errno::FileTooLarge.as_neg_i32() as isize);
            }
            let allowed = (fsz_cur - start) as usize;
            if count > allowed {
                count = allowed;
            }
        }
    }
    if count == 0 {
        return Some(0);
    }

    let _pos_guard = file.io_lock.guard();
    let written = ext4_dio_write_gup_run(fs, Some(inode), inode.ino as u32, offset, src, count)?;
    if written > 0 {
        let end = if offset == u64::MAX {
            inode.size.load(core::sync::atomic::Ordering::Acquire)
        } else {
            offset + written as u64
        };
        file.set_pos(end);
        // IN_MODIFY: same discipline as ext4_file_write_user_vfs.
        crate::fs::inotify::notify_file(
            file,
            crate::fs::inotify::inotify_events::IN_MODIFY,
        );
    }
    Some(written)
}

/// pwrite(2) O_DIRECT GUP entry — the write(2) twin at an explicit
/// offset (fio's psync engine pwrites). None = staged path.
pub fn ext4_file_pwrite_dio_user_vfs(
    file: &File,
    offset: u64,
    src: *const u8,
    count: usize,
) -> Option<isize> {
    let (inode, fs, _) = file_ext4(file)?;
    if !inode.mode.is_regular_file() {
        return None;
    }
    if fs.read_only {
        return None;
    }
    if count == 0 {
        return Some(0);
    }
    let _io_guard = file.io_lock.guard();
    let written =
        ext4_dio_write_gup_run(fs, Some(inode), inode.ino as u32, offset, src, count)?;
    if written > 0 {
        crate::fs::inotify::notify_file(
            file,
            crate::fs::inotify::inotify_events::IN_MODIFY,
        );
    }
    Some(written)
}

/// Kernel-side positional write for callers with no VFS file description
/// (klogd's /var/log/kmsg flush): the same big-lock + journal +
/// cache-coherence discipline as write(2), addressed by (fs, ino, offset).
/// Returns the bytes written, or a negative errno.
pub fn ext4_kernel_write_at(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ext4_ino: u32,
    offset: u64,
    buf: &[u8],
) -> isize {
    // SAFETY: fs is a valid Ext4FileSystem reference; the core owns all
    // locking and operates on (fs, ino) exactly like the fd paths.
    { ext4_write_at_core(fs, ext4_ino, None, offset, true, buf, &[0], false) }
}

/// Buffered (delayed-allocation) write for regular files — the reference
/// kernel's write(2) shape (perf-gaps 2026-11, the p6_append 301x gap).
///
/// The old path (still taken for O_SYNC/O_DSYNC/O_DIRECT and as the
/// per-page fallback here) runs the full write-through machinery per
/// write(2): journal start, disk-fresh inode read, block allocation,
/// bitmap/extent/inode rewrites and a SYNCHRONOUS journal commit at stop
/// — the measured ~88µs append class, vs the reference's 0.39µs: the
/// reference does delayed allocation, where write(2) only
/// 1. copies the user bytes into page-cache frames (creating them
///     zero-based for fresh tail pages),
/// 2. extends the in-memory size,
/// and defers ALL block allocation + metadata journaling to the
/// writeback points that already exist: fsync/msync
/// (ext4_flush_dirty_inode → ext4_write_at_core per page), truncate
/// (flush-then-cut, ext4_setattr), direct-reclaim writeback
/// (shrink_reclaim) and the P1 shutdown flush.
///
/// Size protocol (two views, one authority):
/// - `Inode.size` (atomic) is the monotonic reservation watermark. An
///   appending write reserves [size, size+len) with one CAS, so
///   concurrent appends on different open descriptions land at disjoint
///   offsets exactly like the reference's inode-lock serialization.
/// - the sb-boxed Ext4Inode copy (what every reader bounds reads by, and
///   what stat serves) is published LAST, under the ext4 big lock, only
///   after the data sits in frames — data-before-size, so a racing
///   reader never observes an extended EOF over missing bytes. The
///   write-through refresh paths hold the same big lock, so the
///   read-modify-write max() here cannot tear against them.
///
/// Correctness notes:
/// - dirty frames are born dirty (`insert_dirty`) — no clean window an
///   eviction could exploit to drop a returned write;
/// - a partial page whose base may live on disk is filled through the
///   normal read machinery (big lock, coherence rules) before the copy;
/// - a lost insert race (duplicate) or a stale-pinned orphan entry falls
///   back to the write-through core for that page after bounded rounds;
/// - crash class: identical to the reference — a write(2) that returned
///   without a following fsync/sync is not durable.
fn ext4_buffered_write(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::inode::Inode,
    offset: u64,
    src: WriteSrc,
) -> isize {
    use core::sync::atomic::Ordering;

    let srclen = src.len();
    if srclen == 0 {
        return 0;
    }
    // The cached Ext4Inode box behind inode.sb (INV-EXT4-SBRES-1),
    // resolved through the ext4 framework edge; the raw word is kept
    // only for the audited advisory-store epilog below.
    let sbi = match super::sb_inode_of(inode) {
        Ok(b) => b,
        // INV-EXT4-SBRES-1 — the sb box lives for the inode; this shared
        // view is read-mostly (the epilog's advisory stores go through
        // the audited helper below, under the big lock).
        Err(_) => return errno::Errno::IOError.as_neg_i32() as isize,
    };
    // The raw word is kept only for the audited advisory-store epilog
    // below (the same box the resolver above borrowed).
    let sb_ptr = sbi as *const super::inode::Ext4Inode as *mut super::inode::Ext4Inode;
    let fs_id = fs as *const crate::fs::ext4::Ext4FileSystem as u64;
    let ino = inode.ino as u64;
    let cache = page_cache::get_page_cache();
    let ps = crate::mm::PAGE_SIZE as u64;
    let psu = crate::mm::PAGE_SIZE;

    // EAGER-ENOSPC gate (falloc-wedge lane, 2026-11-16): an EXTENDING
    // write on a zero-free fs fails BEFORE any size publication or page
    // dirtying. Without it the delayed-allocation path kept accepting
    // writes into a full fs (the pages' flushes then failed -ENOSPC
    // forever — the unflushable dirty balloon), so an ENOSPC fill
    // (LTP tst_fill_fs, fallocate05/06) never terminated: every write
    // kept tripping the dirty watermark and paying a futile whole-inode
    // flush (measured 39 j per 4 KiB write, run/f05h). The reference
    // kernel's eager-allocation fses (ext2 among them) fail at write
    // time; overwrites inside the allocated size pass — they may need
    // no new blocks (a hole-overwrite that does surfaces its ENOSPC at
    // writeback, the delayed-allocation latitude both kernels keep).
    {
        let would_extend = if offset == u64::MAX {
            true // O_APPEND always extends
        } else {
            match offset.checked_add(srclen as u64) {
                Some(end) => end > inode.size.load(Ordering::Acquire),
                None => return errno::Errno::FileTooLarge.as_neg_i32() as isize,
            }
        };
        // ONE free estimate per write, shared by the eager zero-gate and
        // the reservation below (two group-descs walks per 4 KiB write
        // was measurable on the fsync4k row).
        let free_est = if would_extend {
            fs.est_free_blocks()
        } else {
            0 // non-extending: the eager gate does not apply
        };
        if would_extend && free_est == 0 {
            return errno::Errno::NoSpaceLeftOnDevice.as_neg_i32() as isize;
        }

        // Delalloc reservation (fio-delalloc lane, 2026-11-18): count the
        // blocks this write's writeback will have to allocate — the
        // written range's block indices AT OR PAST the inode's allocated
        // frontier (ceil of the cached on-disk size; allocation never
        // leads the size it serves) — against the fs's free space BEFORE
        // any page is dirtied and BEFORE the size watermark is published
        // below. A refusal is the write-time ENOSPC of the reference's
        // ext4_da_reserve_space: reservations held by ANY writer reduce
        // the space a new writer may promise, so concurrent writers on a
        // nearly full fs fail at write() instead of racing past the zero
        // check and failing at flush time (the unflushable dirty
        // balloon). Overwrites below the frontier owe nothing (their
        // blocks exist); a repeat write over an already-reserved range
        // owes nothing new (set dedup). Quota check: the reference's
        // dq_alloc_block hook has no infrastructure here yet — recorded
        // follow-up.
        let bs = fs.block_size as u64;
        let base_size = sbi.get_size();
        let frontier = base_size.div_ceil(bs);
        let (first, last) = if offset == u64::MAX {
            let cur = inode.size.load(Ordering::Acquire);
            (cur / bs, (cur + srclen as u64).div_ceil(bs))
        } else {
            (offset / bs, (offset + srclen as u64).div_ceil(bs))
        };
        let first = first.max(frontier);
        if last > first {
            if let Err(e) = da_reserve_blocks(fs, inode.ino as u32, first, last - first, free_est) {
                return e as isize;
            }
        }
    }

    // 1) Resolve + reserve the range.
    let mut offset = offset;
    let end = match offset.checked_add(srclen as u64) {
        Some(e) if offset != u64::MAX => e,
        _ => {
            if offset == u64::MAX {
                // O_APPEND: CAS-reserve [size, size+len).
                loop {
                    let cur = inode.size.load(Ordering::Acquire);
                    let new_end = match cur.checked_add(srclen as u64) {
                        Some(v) => v,
                        None => {
                            return errno::Errno::FileTooLarge.as_neg_i32() as isize
                        }
                    };
                    match inode.size.compare_exchange(
                        cur,
                        new_end,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            offset = cur;
                            break;
                        }
                        Err(_) => continue,
                    }
                }
                offset + srclen as u64
            } else {
                // Explicit offset beyond the representable range.
                return errno::Errno::FileTooLarge.as_neg_i32() as isize;
            }
        }
    };
    let published_end = end;
    if offset != u64::MAX && end > inode.size.load(Ordering::Acquire) {
        // Explicit-offset write extending the file: raise the watermark
        // (monotonic CAS — never regresses a concurrent append).
        let mut cur = inode.size.load(Ordering::Acquire);
        while end > cur {
            match inode
                .size
                .compare_exchange(cur, end, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(c) => cur = c,
            }
        }
    }

    // ext4:ext4_da_write_begin — fired after the offset resolution (an
    // O_APPEND write resolves its position here), with the final pos the
    // reference's write_begin reports. This path defers block allocation
    // + journaling to the writeback points, so the da_ names are the
    // honest ones.
    if crate::trace::EVENT_EXT4_DA_WRITE_BEGIN.armed() {
        crate::trace::trace_ext4_da_write_begin(
            super::dev_id(fs),
            ino,
            offset as i64,
            srclen as u32,
        );
    }

    // Pre-write published size: pages starting below it may carry disk
    // content (partial-page fills read them); above it the base is zeros.
    let base_size = sbi.get_size();

    let first_idx = offset / ps;
    let last_idx = (offset + srclen as u64 - 1) / ps;
    let mut copied: usize = 0;
    // BUFW-2X: a user-page fault ends the write early (short-write /
    // EFAULT semantics — see the User arms below). The staged path could
    // not fault mid-write (its copy_in ran whole, up front), so this flag
    // is only ever set for WriteSrc::User.
    let mut user_fault = false;

    // DIO-CONC: whole-run dirty batch. The maximal span of FULL pages
    // ([first_full, last_full]) rides ONE page-cache batch — one wlock
    // pass per 64 pages, batched order-0 allocations, memcpys outside
    // every lock (the per-page insert_dirty loop cost ~0.6 µs/page, the
    // 1 MiB buffered-write row's 0.62x). The batch's duplicate arm
    // OVERWRITES an already-cached page and turns it dirty (the
    // pin-and-copy outcome), so racing cached pages are served
    // correctly; an early stop (OOM / table full / user fault) returns
    // the consumed prefix and the per-page loop below serves the tail.
    // Head/tail partial pages stay with the per-page loop
    // (read-modify-write semantics).
    //
    // BUFW-2X: a USER source rides insert_dirty_batch_user — the frames
    // are filled straight from user memory, ONE copy per page (the
    // staging buffer and its second copy are gone), and a faulting page
    // stops the batch at the previous page.
    let first_full = first_idx + ((offset % ps) != 0) as u64;
    let n_full = ((offset + srclen as u64) / ps).saturating_sub(first_full);
    let mut batch_start_idx = first_idx;
    if n_full > 0 {
        let n_pages = n_full as usize;
        let src_off = (first_full * ps - offset) as usize;
        match src {
            WriteSrc::Kernel(..) => {
                // INV-EXT4-WRITESRC-K: the whole source span.
                let buf = src.kernel_bytes(0, srclen).expect("Kernel source");
                let consumed = cache.insert_dirty_batch(
                    fs_id,
                    ino,
                    first_full,
                    psu,
                    &buf[src_off..src_off + n_pages * psu],
                );
                copied += consumed * psu;
                batch_start_idx = first_full + consumed as u64;
            }
            WriteSrc::User(p, _) => {
                let (consumed, fault) = cache.insert_dirty_batch_user(
                    fs_id,
                    ino,
                    first_full,
                    psu,
                    p as usize + src_off,
                    n_pages * psu,
                );
                copied += consumed * psu;
                batch_start_idx = first_full + consumed as u64;
                user_fault |= fault;
            }
        }
    }

    // The per-page loop covers EVERY page the batch did not fully serve:
    // the head partial page (offset % ps != 0), the tail partial page,
    // and any batch tail after an early stop. (BUFW-2X fix: the loop
    // used to start at first_full+consumed, which SKIPPED the head
    // partial page whenever the batch covered full pages — its bytes
    // were silently dropped and the `copied < buf_start + want` fallback
    // reset the accounting so the loss was invisible except to a
    // read-back: fio --verify=md5 --offset=4196 --bs=8k failed err=84.)
    for idx in first_idx..=last_idx {
        // Batch-served pages: the batch installed (or overwrote) them.
        if idx >= first_full && idx < batch_start_idx {
            continue;
        }
        let page_off = if idx == first_idx {
            (offset % ps) as usize
        } else {
            0
        };
        let buf_start = if idx == first_idx {
            0
        } else {
            (idx * ps - offset) as usize
        };
        let in_page_avail = psu - page_off;
        let want = core::cmp::min(srclen - buf_start, in_page_avail);
        let file_off = idx * ps + page_off as u64;
        let full_page = page_off == 0 && want == psu;

        let mut filled_once = false;
        let mut rounds = 0u32;
        // Set by every arm that landed this page's bytes (pin-and-copy,
        // insert, revive). The write-through fallback below must fire on
        // THIS page's failure — the old `copied < buf_start + want`
        // accounting check cannot see a head-partial page's failure once
        // the batch's later pages already pushed `copied` past it.
        let mut served = false;
        loop {
            rounds += 1;
            // Pin-and-copy fast path (also the retry path after a lost
            // insert race: the winner's frame carries our bytes after
            // this copy). BUFW-2X: the copy is the ONE user→frame copy
            // for a User source; a mid-copy fault counts the landed
            // prefix (the frame stays coherent with exactly what is
            // counted — no torn bytes beyond the returned count) and
            // ends the write.
            if let Some(frame) = cache.get(fs_id, ino, idx) {
                // frame is a PAGE_SIZE pinned cache frame; the span
                // [page_off, page_off+want) is inside it by construction
                // (want <= PAGE_SIZE - page_off). The offset advance is
                // in-bounds by that same inequality (wrapping_add keeps
                // the in-range arithmetic explicit rather than relying on
                // the unsafe add's debug-overflow behavior).
                let frame = frame as *mut u8;
                let uncopied = src.copy_to_frame(
                    buf_start,
                    crate::safety::bytes::pinned_frame_bytes_mut(
                        &frame,
                        page_off,
                        want,
                    ),
                );
                if uncopied > 0 {
                    let landed = want - uncopied;
                    copied += landed;
                    if landed > 0 {
                        // The landed prefix IS in the frame; keep the
                        // frame coherent with the counted bytes.
                        cache.mark_dirty(fs_id, ino, idx);
                        served = true;
                    }
                    cache.put(fs_id, ino, idx);
                    user_fault = true;
                    break;
                }
                cache.mark_dirty(fs_id, ino, idx);
                cache.put(fs_id, ino, idx);
                copied += want;
                served = true;
                break;
            }
            if full_page {
                // Base content fully replaced: no read-modify-write.
                match src {
                    WriteSrc::Kernel(..) => {
                        // INV-EXT4-WRITESRC-K: the [buf_start, +want) span.
                        let s = src.kernel_bytes(buf_start, want).expect("Kernel source");
                        if cache.insert_dirty(fs_id, ino, idx, s) {
                            copied += want;
                            served = true;
                            break;
                        }
                        // Lost the insert race — loop back to
                        // pin-and-copy.
                        if rounds > 8 {
                            break;
                        }
                        continue;
                    }
                    WriteSrc::User(p, _) => {
                        match cache.insert_dirty_user(
                            fs_id,
                            ino,
                            idx,
                            p as usize + buf_start,
                            want,
                        ) {
                            page_cache::DirtyUserOutcome::Installed => {
                                copied += want;
                                served = true;
                                break;
                            }
                            page_cache::DirtyUserOutcome::Contention => {
                                // Lost the insert race — loop back to
                                // pin-and-copy.
                                if rounds > 8 {
                                    break;
                                }
                                continue;
                            }
                            page_cache::DirtyUserOutcome::Fault => {
                                user_fault = true;
                                break;
                            }
                        }
                    }
                }
            }
            // Partial page. If it may carry disk content, fill it once
            // through the read machinery (big lock + coherence rules),
            // then retry the pin-and-copy path.
            if !filled_once && idx * ps < base_size {
                filled_once = true;
                let _g = crate::fs::ext4::EXT4_BIG_LOCK.lock();
                fill_page_cache_batch(fs, sbi, cache, fs_id, ino, idx, idx + 1);
                continue;
            }
            // Beyond the pre-write EOF (or a hole the fill could not
            // populate): the uncovered bytes read as zeros — zero-fill
            // base frame, then copy the covered span through a pin.
            if cache.insert_dirty(fs_id, ino, idx, &[]) {
                if let Some(frame) = cache.get(fs_id, ino, idx) {
                    // As above: a pinned-frame mutable span.
                    let frame = frame as *mut u8;
                    let uncopied = src.copy_to_frame(
                        buf_start,
                        crate::safety::bytes::pinned_frame_bytes_mut(
                            &frame,
                            page_off,
                            want,
                        ),
                    );
                    if uncopied > 0 {
                        let landed = want - uncopied;
                        copied += landed;
                        if landed > 0 {
                            cache.mark_dirty(fs_id, ino, idx);
                            served = true;
                        }
                        cache.put(fs_id, ino, idx);
                        user_fault = true;
                        break;
                    }
                    cache.mark_dirty(fs_id, ino, idx);
                    cache.put(fs_id, ino, idx);
                    copied += want;
                    served = true;
                    break;
                }
                // Racing invalidation between insert and pin: fall
                // through to the bounded retry.
            }
            if rounds > 8 {
                break;
            }
        }

        if user_fault {
            break;
        }

        if !served || copied < buf_start + want {
            // Bounded retries exhausted (stale-pinned orphan entry or
            // pathological insert racing): write this page's span through
            // the durable write-through core. Allocation + journal commit
            // happen there; the frames this write already dirtied for
            // other pages are unaffected (write-through updates frames in
            // place, writeback_populate_range).
            //
            // BUFW-2X: the write-through core takes a kernel slice; a
            // USER source has none — bounce the page (≤ PAGE_SIZE)
            // through a heap buffer scoped to this page's iteration.
            // This arm is the pathological-race fallback; a faulting
            // bounce is the same user fault the direct copies above
            // would have hit.
            let mut bounce;
            let src_slice: &[u8] = match src {
                WriteSrc::Kernel(..) => src.kernel_bytes(buf_start, want).expect("Kernel source"),
                WriteSrc::User(..) => {
                    bounce = alloc::vec![0u8; want];
                    // bounce is a valid kernel destination.
                    let uncopied = src.copy_to_frame(buf_start, &mut bounce[..]);
                    if uncopied > 0 {
                        copied += want - uncopied;
                        user_fault = true;
                        break;
                    }
                    &bounce
                }
            };
            let r = {
                ext4_write_at_core(fs, inode.ino as u32, Some(inode), file_off, true, src_slice, &[0], false)
            };
            if r < 0 {
                if copied > 0 && crate::trace::EVENT_EXT4_DA_WRITE_END.armed() {
                    // ext4:ext4_da_write_end — partial success (a later
                    // page's write-through failed after earlier pages
                    // copied): the reference fires write_end with the
                    // partial copied count, and so does this arm.
                    crate::trace::trace_ext4_da_write_end(
                        super::dev_id(fs),
                        ino,
                        offset as i64,
                        srclen as u32,
                        copied as u32,
                    );
                }
                return if copied > 0 { copied as isize } else { r };
            }
            copied = buf_start + r as usize; // r == want on success
        }
    }

    // BUFW-2X fault epilogue. The staged path copies the whole request
    // into kernel memory BEFORE any file state moves, so a faulting user
    // buffer leaves the file untouched (or returns the fully-written
    // prefix). The direct-copy path copies per page AFTER the size
    // reservation, so on a fault the up-front publication must be pulled
    // back to what actually landed — CAS only succeeds while our
    // published_end is still the live watermark (a concurrent writer
    // that advanced it wins, monotonicity preserved). copied==0 keeps
    // the staged path's observable contract exactly: EFAULT, file
    // untouched.
    let mut pub_end = published_end;
    if user_fault {
        if copied == 0 {
            // Roll the reservation back (best-effort CAS — a racing
            // append may legitimately own a larger watermark now).
            let _ = inode.size.compare_exchange(
                published_end,
                offset,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            return errno::Errno::BadAddress.as_neg_i32() as isize;
        }
        let target = offset + copied as u64;
        let _ = inode.size.compare_exchange(
            published_end,
            target,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        // A concurrent writer may hold a size between target and
        // published_end — close it only down to target (monotonic CAS,
        // never below the rollback target).
        let mut cur = inode.size.load(Ordering::Acquire);
        while target < cur && cur <= published_end {
            match inode
                .size
                .compare_exchange(cur, target, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(c) => cur = c,
            }
        }
        // Publish timestamps + the dirty-eof watermark with the
        // ROLLED-BACK end (the common block below).
        pub_end = target;
    }

    // 2) Publish: data is in frames; lift the read bound + timestamps
    // under the big lock (atomic vs the write-through refresh paths).
    // (BUFW-2X: pub_end is the rolled-back end after a user fault —
    // timestamps and the dirty-eof watermark then cover exactly the
    // copied prefix.)
    {
        let _g = crate::fs::ext4::EXT4_BIG_LOCK.lock();
        let now = crate::drivers::rtc::wall_secs() as u32;
        // Audited advisory-field store (INV-EXT4-ATIME discipline):
        // the write-tail epilog. sb_ptr is the inode's boxed copy,
        // installed at open and never freed while the inode lives
        // (INV-EXT4-SBRES-1).
        stamp_write_tail(sb_ptr, pub_end, now);
        // Exact-EOF watermark for the revival guard (create_vfs_inode_in):
        // data is already in the dirty frames, so a later icache eviction +
        // re-instantiation can recover the byte-exact size. CAS-max inside
        // PageCache (a plain store could regress a concurrent append's
        // watermark).
        cache.note_dirty_eof(fs_id, ino, pub_end);
    }

    // ext4:ext4_da_write_end — the full-copy success return (partial
    // copied after a user fault: the reference fires write_end with the
    // partial count too).
    if crate::trace::EVENT_EXT4_DA_WRITE_END.armed() {
        crate::trace::trace_ext4_da_write_end(
            super::dev_id(fs),
            ino,
            offset as i64,
            srclen as u32,
            copied as u32,
        );
    }

    // DIRTY-BALLOON WATERMARK (falloc-wedge lane, 2026-11-16): buffered
    // writes are delayed-allocation — before this, the only writers of
    // dirty frames back to disk were fsync/msync, the shutdown flush,
    // and reclaim's starved shrink_reclaim flush. A sustained buffered
    // writer (mkfs/fill through a loop device: every ext2 page write
    // becomes a backing-file buffered write on the parent fs) grows the
    // dirty set to RAM scale — dirty pages skip eviction, the zone
    // drains to zero, and reclaim's own flush, nested inside the loop
    // io claim and allocation-starved, wedges (the fallocate05/06
    // reclaim io_lock re-entry + fdreap RCU stall; the wedge boot's
    // census: 243k LRU pages, evict-skip dominated by the dirty bit).
    // The reference kernel starts writeback far below the memory wall
    // and throttles writers in their own context
    // (dirty_background_threshold + balance_dirty_pages). Same
    // discipline here: once the zone is half consumed, this writer
    // flushes its OWN inode's dirty range before returning — the
    // run-batched flush engine in this file, in writer context, no
    // reclaim involvement. Stacked-device re-entry: the flush targets
    // the inode being written, whose blocks live on THIS fs's device —
    // a loop backing-file write flushes to the parent fs's device, never
    // back through the loop it is inside (and shrink_reclaim separately
    // degrades to clean-only under any loop io claim). One flusher at a
    // time: a contending writer skips — the winner's flush already
    // serves the watermark for both.
    if copied > 0
        && crate::mm::vmscan::zone_below_dirty_writeback_mark()
        && WRITER_FLUSH_IN_FLIGHT
            .compare_exchange(
                false,
                true,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    {
        let t0 = crate::timer::get_jiffies();
        // Flush THIS writer's inode first (the throttle proper — the
        // writer waits for its own data), then the system-wide recovery
        // pass: flush every dirty inode the windowed collect finds
        // (WHOLE inodes, run-batched) and clean-evict up to the recover
        // target (mark + 1/16 managed). The shrink stage is what raises
        // nr_free, so the mark actually un-trips and the next
        // ~1/16-managed dirty pages stay buffered — without it the mark
        // trips permanently once the cache reaches its cap and every
        // buffered write pays a synchronous flush of itself (measured on
        // the fallocate05 fill: one 4 KiB page flushed per write
        // syscall). Re-entrancy inside the recovery is loop-claim-aware
        // (clean-only degrade) — see writeback_recover.
        let flushed = ext4_flush_dirty_inode(inode, 0, u64::MAX);
        let mut shrunk = 0usize;
        if let Some((free, _mark, target)) = crate::mm::vmscan::zone_dirty_writeback_state() {
            let shortfall = target.saturating_sub(free);
            if shortfall > 0 {
                shrunk = page_cache::get_page_cache().writeback_recover(shortfall);
            }
        }
        let dt = crate::timer::get_jiffies().saturating_sub(t0);
        WRITER_FLUSH_IN_FLIGHT.store(false, core::sync::atomic::Ordering::Release);
        // Bounded tuning print (the first 40 watermark flushes per boot):
        // names the flush cost class on the repro rigs; silent otherwise.
        // free/dirty ride along (the recovery verdict: free must rise
        // above mark, dirty must fall) plus the reclaim census line.
        static WB_PRINTS: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if WB_PRINTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            let (free_now, mark, _target) =
                crate::mm::vmscan::zone_dirty_writeback_state().unwrap_or((0, 0, 0));
            crate::pr_info!(
                "WB-WATERMARK fs={:x} ino={} wrote={}B flushed_pages={} shrunk={} took={}j free={}/mark={} dirty={}",
                fs_id,
                ino,
                copied,
                flushed,
                shrunk,
                dt,
                free_now,
                mark,
                page_cache::dirty_pages()
            );
            if shrunk == 0 {
                crate::pr_info!(
                    "WB-WATERMARK census: {}",
                    page_cache::pcache_reclaim_census_line()
                );
                static HIST_PRINTS: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if HIST_PRINTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 6 {
                    crate::pr_info!("{}", page_cache::pcache_dirty_histogram());
                }
            }
        }
    }
    copied as isize
}

/// One-writer-flush-at-a-time guard for the dirty-balloon watermark
/// writeback in [`ext4_buffered_write`] (see there).
static WRITER_FLUSH_IN_FLIGHT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Explicit-offset ext4 write core shared by write(2) and pwrite/writeback
/// (session 25). Owns the EXT4_BIG_LOCK + journal discipline and the
/// page-cache WRITE-THROUGH policy: after a successful write the cached
/// frames overlapping the written range are updated IN PLACE (never
/// invalidated — a pinned frame that serves misses would fork a second,
/// divergent copy of the page on the next fault, permanently splitting
/// mapper views). Same big-lock/bitmap serialization as the old inline
/// body (review 5.5 high).
/// Refresh a cached VFS inode's view after a write: the sb-boxed
/// Ext4Inode copy (bounds every read/stat served from the cache) and the
/// dentry-visible size. Shared by the fd write paths and the kernel-side
/// writer (klogd) so both leave identical cache state.
fn refresh_cached_vfs_inode(
    vi: &crate::fs::inode::Inode,
    ext4_inode: &super::inode::Ext4Inode,
) {
    let fresh_size = ext4_inode.get_size();
    // Delayed-allocation size guard (perf-gaps 2026-11): buffered writes
    // publish the cached size AHEAD of the disk inode — the dirty
    // page-cache frames carry the bytes, writeback catches the disk up.
    // This refresh carries the DISK-FRESH size, which can lag: overwriting
    // with it while dirty frames still live beyond it would (a) hide the
    // dirty tail from every reader (reads/stats clamp at the cached size)
    // and, worse, (b) DROP it at the next flush — ext4_flush_dirty_inode
    // clamps each page by inode.get_size() and treats past-clamp pages as
    // truncate discards. Hold the current cached size until writeback
    // catches up (the final flushed page refreshes with the exact value);
    // the probe only runs when the fresh size SHRINKS, so the common
    // write-through path (fresh >= cached) pays nothing.
    // Truncate is unaffected: setattr writes the cache through
    // refresh_inode_cache + the explicit ATTR_SIZE store and then
    // invalidates the whole page-cache entry — the dirty frames are gone,
    // so this guard sees nothing to hold.
    let size = if fresh_size < vi.get_size()
        && vi.private_data.is_some_and(|p| {
            crate::fs::page_cache::get_page_cache()
                .dirty_extent_hint(p as u64, vi.ino as u64)
                .is_some_and(|e| e > fresh_size)
        }) {
        vi.get_size()
    } else {
        fresh_size
    };
    // Update cached copy in inode.sb
    // sb is the Box<Ext4Inode> this VFS inode owns (leaked in
    // create_vfs_inode_in); no concurrent fd path frees it while the
    // ext4 big lock is held (this runs under EXT4_BIG_LOCK) — the
    // INV-EXT4-SBRES-1 mutable resolver carries that contract.
    if let Ok(cached) = super::sb_inode_of_mut(vi) {
        cached.block = ext4_inode.block;
        cached.size = size;
        cached.blocks = ext4_inode.blocks;
        cached.mtime = ext4_inode.mtime;
        cached.ctime = ext4_inode.ctime;
    }
    // Update cached VFS inode size
    vi.size.store(size, core::sync::atomic::Ordering::Relaxed);
}

/// `inode` is the caller's VFS inode when it has one (the fd paths). A
/// kernel-side writer with no VFS file description passes `None`; the
/// cache-coherence tail then resolves the cached VFS inode through the
/// icache — if one exists its size/sb view is refreshed exactly like the
/// fd path does; if none exists there is nothing stale to refresh (the
/// next lookup reads the just-written on-disk inode).
/// Staged-slice wrapper — the pre-GUP signature (see
/// ext4_write_at_core_src).
fn ext4_write_at_core(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ext4_ino: u32,
    inode: Option<&crate::fs::inode::Inode>,
    offset: u64,
    defer_unjournaled: bool,
    buf: &[u8],
    skip_pfns: &[u64],
    dio_fast: bool,
) -> isize {
    ext4_write_at_core_src(fs, ext4_ino, inode, offset, defer_unjournaled, DioWriteSrc::Staging(buf), skip_pfns, dio_fast)
}

/// The write core (source-parameterized GUP-DIO form): staging slice or
/// pinned user frames; the journal/locking/write-through discipline is
/// identical.
fn ext4_write_at_core_src(
    fs: &crate::fs::ext4::Ext4FileSystem,
    ext4_ino: u32,
    inode: Option<&crate::fs::inode::Inode>,
    offset: u64,
    // Route-1 (leg 52, item 3): allow the write to run DEFERRED on a
    // journal-less filesystem (see the handle arm below). The fd callers
    // pass false for O_SYNC/O_DSYNC files; kernel-side writers and the
    // page-cache flush path pass true (J-image parity — the journaled
    // flush already defers under its handle today).
    defer_unjournaled: bool,
    wsrc: DioWriteSrc<'_>,
    // Flush-source frames exempt from the write-through populate (a
    // frame copied onto itself is a self-overlap; the run-batched flush
    // passes EVERY source frame of its staged run). `&[0]` = no skip.
    skip_pfns: &[u64],
    // O_DIRECT fast arm (paired-test item 1): the fd callers set this
    // from the description's O_DIRECT bit — full-block runs then go to
    // the device straight from the caller's staging slice (or, GUP-DIO,
    // its pinned user frames).
    dio_fast: bool,
) -> isize {
    // SMP serialization for the write path (review 5.5 high: 写路径/位图
    // RMW 无锁——SMP 位图丢更新双分配): allocation, bitmap RMW, group
    // descriptor updates and the inode rewrite all happen under the
    // ext4 big lock, same as namei.
    let _ext4_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock();

    // Start a journal transaction for data=ordered semantics:
    // data blocks are synced during write, then the inode metadata is
    // committed to the journal with all data already on disk.
    // Route-1 (leg 52, item 3, perf-report 8.10.20/8.10.22 + route map
    // §5(a)): on a journal-less fs, arming the NULL-JOURNAL handle here
    // turns every inline sync_dirty_buffer in this write (bitmap, group
    // desc, extent tree, inode table) into a deferral — persistence
    // moves to the standing durability points (fsync/sync_buffers/
    // eviction), the J arm's proven ~88us append class instead of the
    // measured ~1.95ms inline-sync class (pa85: NJ append 23.1x). The
    // null handle's stop() commits nothing, so defer_unjournaled is
    // FALSE for O_SYNC/O_DSYNC files — their write-return durability on
    // NJ is today's inline sync and must not be deferred. Crash class:
    // same write-back stance the NJ create/mkdir path already documents
    // (journal.rs route-1 comment; recovery is e2fsck).
    let use_journal = fs.journal.is_some();
    let mut journal_handle = match if use_journal || defer_unjournaled {
        super::journal::ext4_journal_start(fs, 4)
    } else {
        Err(0)
    } {
        Ok(h) => Some(h),
        Err(_) => None,
    };
    // R9-6: register the FINAL location of the handle — the old code
    // stored &mut of the match-arm binding and then MOVED it into
    // Some(h), leaving Task.journal_handle pointing at a dead stack
    // slot that later calls reused (the exact dead-frame deref M3 was
    // meant to remove, in the main write path).
    if let Some(h) = journal_handle.as_mut() {
        // h is the journal handle this core just created; the setter only
        // stores it into the current task's cell.
        super::namei::set_current_handle(h);
    }

    // Read ext4 inode from disk (write needs fresh on-disk data)
    let mut ext4_inode = match fs.read_inode(ext4_ino) {
        Ok(inode) => inode,
        Err(e) => {
            if journal_handle.is_some() {
                // Clears the same cell the setter above filled.
                super::namei::clear_current_handle();
                if let Some(mut h) = journal_handle {
                    let _ = super::journal::ext4_journal_stop(&mut h);
                }
            }
            return e as isize;
        }
    };

    // O_APPEND sentinel from write_vfs: resolve the end-of-file offset NOW,
    // from the disk-fresh inode under the big lock (the original inline
    // body's discipline — an append racing on another description must be
    // visible before the offset is fixed).
    let offset = if offset == u64::MAX {
        ext4_inode.get_size()
    } else {
        offset
    };

    // Call internal write function
    let result = match ext4_file_write_src(fs, &mut ext4_inode, offset, wsrc, dio_fast) {
        Err(e) => e as isize,
        Ok(written_bytes) => {
            // CACHE COHERENCE TAIL (the klogd lesson, 2026-10-26): the
            // cached VFS view — the sb-boxed Ext4Inode copy that bounds
            // every read, plus the dentry-visible size — must move with
            // the write. The fd paths pass their inode; a kernel-side
            // writer (klogd's /var/log/kmsg flush) passes None and the
            // icache resolves whatever cached inode userspace would
            // otherwise read through. Skipping this tail left direct
            // kernel writes invisible: the flush landed on disk (offsets
            // and the on-disk inode advanced, probe-printed) while every
            // subsequent open/stat/read served the pre-write cached view
            // forever (the oops-smoke "klog_lines2=3" stuck family — the
            // wake path was never broken).
            if let Some(vi) = inode {
                refresh_cached_vfs_inode(vi, &ext4_inode);
            } else if let Some(vi) = crate::fs::inode::icache_lookup(
                ext4_ino as u64,
                fs as *const crate::fs::ext4::Ext4FileSystem as u64,
            ) {
                refresh_cached_vfs_inode(&vi, &ext4_inode);
            }
            // Write back inode to disk (registers with journal if handle active)
            let wi_res = crate::fs::ext4::inode::write_inode(fs, ext4_ino, &ext4_inode);
            match wi_res {
                Ok(()) => {
                    // Session 25 write-through: update cached frames in the
                    // written range IN PLACE instead of invalidating the
                    // whole inode. Pinned entries (live mappings alias the
                    // frame) MUST stay coherent with the file; clean ones
                    // simply stay hot. (The old invalidate_inode freed
                    // every clean page of the inode and marked pinned ones
                    // stale — a MAP_PRIVATE read-only mapper then served
                    // PRE-WRITE bytes from its still-pinned stale frame,
                    // and a MAP_SHARED re-fault forked a divergent frame.)
                    // skip_pfns: the msync/fsync flush path writes cache
                    // frames' own bytes back to the file — updating THOSE
                    // entries would copy each frame onto itself.
                    //
                    // writeback_populate_range (perf-mm 2026-10-31): the
                    // update arm is unchanged, PLUS the fully-written
                    // page run is inserted into the page cache as clean
                    // frames — the reference kernel's buffered-write
                    // shape. A file written then mmap'd faults from the
                    // cache (p3c cold 14.1 us/page vs warm 0.94) instead
                    // of repopulating through the read machinery.
                    if written_bytes > 0 {
                        // DIO fast arm (paired-test item 1): UPDATE-only.
                        // The direct spans' bytes are device-acked at this
                        // point (ext4_file_write drained before returning)
                        // and the fast arm leaves NO block-cache entry, so
                        // inserting page-cache frames here would only add
                        // DIRTY duplicates the writeback flusher later
                        // rewrites through the full write core — a random
                        // 4 KiB direct-write storm measured at 4 ms/op
                        // (populate-insert -> periodic flush ->
                        // sync_flushed_data's whole-file map walk).
                        // Coherence: existing mapped frames are updated in
                        // place (the update arm); fresh readers MISS the
                        // page cache and fill from disk (the fast arm
                        // wrote it) or from cached-dirty blocks (the
                        // per-block arm's guard).
                        match &wsrc {
                            DioWriteSrc::Staging(buf) => {
                                if dio_fast {
                                    page_cache::get_page_cache().writeback_update_range_multi(
                                        fs as *const crate::fs::ext4::Ext4FileSystem as u64,
                                        ext4_ino as u64,
                                        offset,
                                        &buf[..written_bytes],
                                        skip_pfns,
                                    );
                                } else {
                                    page_cache::get_page_cache().writeback_populate_range_multi(
                                        fs as *const crate::fs::ext4::Ext4FileSystem as u64,
                                        ext4_ino as u64,
                                        offset,
                                        &buf[..written_bytes],
                                        skip_pfns,
                                    );
                                }
                            }
                            DioWriteSrc::User { pins, .. } => {
                                // GUP-DIO: the same UPDATE-only coherence
                                // walk, reading the written bytes from the
                                // pinned frames (no staging copy exists).
                                page_cache::get_page_cache().writeback_update_range_pinned(
                                    fs as *const crate::fs::ext4::Ext4FileSystem as u64,
                                    ext4_ino as u64,
                                    offset,
                                    pins,
                                    written_bytes,
                                    skip_pfns,
                                );
                            }
                        }
                    }
                    written_bytes as isize
                }
                Err(e) => e as isize,
            }
        }
    };

    // Stop journal transaction
    if journal_handle.is_some() {
        // Clears the current task's journal-handle cell (the setter's
        // twin).
        super::namei::clear_current_handle();
        if let Some(mut h) = journal_handle {
            let _ = super::journal::ext4_journal_stop(&mut h);
        }
    }

    result
}

/// msync/fsync/truncate writeback (session 25): flush every DIRTY
/// page-cache entry of this inode in the inclusive index range back to
/// the file through the ext4 write machinery. Chunked collect → write →
/// complete: the page-cache inodes lock is NEVER held across block I/O
/// (R-WRITESTORM-1).
///
/// Each dirty frame is written at its own file offset, clamped to the
/// CURRENT VFS inode size (a page straddling EOF flushes only its in-file
/// tail — writeback never extends a file; a page fully past EOF — e.g. a
/// truncate raced the flush — writes nothing and just clears its dirty
/// state, which is the discard truncate semantics demand). For ftruncate
/// the caller flushes BEFORE the new size is committed, so a straddling
/// dirty page writes its full content first and the cut removes the tail
/// (flush-then-cut = Linux's writeback-then-truncate semantics).
///
/// Returns the number of dirty pages processed (flush attempts, not
/// necessarily clean writes: an I/O error leaves the page dirty for the
/// next sync and still drops the flush pin here).
pub fn ext4_flush_dirty_inode(
    inode: &crate::fs::inode::Inode,
    first_index: u64,
    last_index: u64,
) -> usize {
    // OPS-IDENTITY GATE (the msync panic, 2026-11-18): only
    // EXT4_INODE_OPS inodes carry an Ext4FileSystem in private_data.
    // Every other filesystem installs its OWN node pointer there
    // (tmpfs a TmpfsNode Arc, memfd a MemfdState Box, devfs/sysfs/...
    // their entries) — reinterpreting one of those as an
    // Ext4FileSystem is type confusion whose garbage geometry reached
    // inode_table_block_of's inodes_per_group divide and panicked the
    // kernel (sys_msync flushed a tmpfs-backed MAP_SHARED file; rt-r1's
    // LTP sched gate, inode.rs:679). The gate lives HERE — like
    // persist_ioc_flags — so every caller (msync's two flush points,
    // fsync, sync_file_range, the page-cache writeback engines) is
    // covered once. Non-ext4 inodes have nothing in THIS machinery to
    // flush (tmpfs pages are written back by tmpfs itself): 0 pages.
    if !inode.ops.is_some_and(|o| {
        core::ptr::eq(o as *const _, &crate::fs::ext4::EXT4_INODE_OPS as *const _)
    }) {
        return 0;
    }
    // INV-FS-PRIVDATA-1: the ext4 instance this inode was created on.
    let fs_ref = match crate::fs::unsafe_edges::privdata_ref::<crate::fs::ext4::Ext4FileSystem>(inode) {
        Some(f) => f,
        None => return 0,
    };
    let fs_id = fs_ref as *const crate::fs::ext4::Ext4FileSystem as u64;
    let ino = inode.ino as u64;
    let cache = page_cache::get_page_cache();

    // Run width (pages per collected chunk / maximal run). 16 matched the
    // run-batched flush's first cut (64 KiB spans); 64 quadruples the run
    // (256 KiB spans, Linux writeback-chunk class) so the per-RUN fixed
    // costs of the write core (journal bracket, read_inode/write_inode,
    // big-lock pair) amortize over 4x more bytes — diskstats attribution
    // on the plugged-span kernel showed the 128 MB fsync wall is the
    // per-run core (2048 runs x ~3 ms), not span submission.
    const CHUNK: usize = 64;
    let mut chunk: [(u64, u64); CHUNK] = [(0, 0); CHUNK];
    // Per-entry write outcomes for flush_done: an entry may only go clean
    // when its bytes verifiably reached the file (full write or a
    // legitimate discard). A FAILED write keeps the page dirty — for a
    // page whose mapping already unmapped (the flush is the only copy
    // left) clearing the bit would make the sole surviving frame
    // evictable and its data silently losable.
    let mut done: [(u64, u64, bool); CHUNK] = [(0, 0, false); CHUNK];
    // RUN-BATCHED FLUSH (flush-wall, 2026-11): contiguous dirty pages are
    // staged into this buffer and written through ONE ext4_write_at_core
    // per RUN (16x fewer journal brackets / read_inode / write_inode /
    // big-lock pairs per page), then the run's data blocks are pushed to
    // disk in coalesced multi-block requests (run_sync_spans) instead of
    // trickling through per-4 KiB dirty-buffer evictions. Grown on first
    // use; a no-dirty call never grows it (collect returns 0 first).
    let mut stage: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let mut total = 0usize;
    // Plugged span pipeline for the whole pass (see SpanFlusher): spans of
    // consecutive runs share one submission window; the final drain below
    // device-acks everything before the caller proceeds to its journal
    // commit (ext4_sync_file) — the ordered-mode data-before-metadata
    // direction the synchronous span writes had.
    let mut flusher = SpanFlusher::new();
    // Resume cursor — the PROGRESS RULE (collect_dirty_range doc): pages
    // of live writable mappings STAY dirty after their flush (flush_done
    // clears the bit only at pin count 0), so re-collecting from
    // first_index would return the same leading chunk forever. Advance
    // past the last collected index each round.
    let mut cursor = first_index;
    loop {
        if cursor > last_index {
            break;
        }
        let n = cache.collect_dirty_range(fs_id, ino, cursor, last_index, &mut chunk);
        if n == 0 {
            break;
        }
        // Partition the collected chunk into maximal runs of CONTIGUOUS
        // page indices (collect walks the BTreeMap in key order, so each
        // run is a slice of the chunk). Isolated pages degenerate to
        // runs of one — exactly the old per-page shape.
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n && chunk[j].0 == chunk[j - 1].0 + 1 {
                j += 1;
            }
            flush_run(
                fs_ref,
                inode,
                &chunk[i..j],
                &mut stage,
                &mut done[i..j],
                &mut flusher,
            );
            i = j;
        }
        cache.flush_done(fs_id, ino, &done[..n]);
        total += n;
        let last_idx = chunk[n - 1].0;
        if last_idx == u64::MAX {
            break;
        }
        cursor = last_idx + 1;
    }
    // Inode-table block rides the SAME window (wrfsync lane, 2026-11-17):
    // the flush's write_inode calls dirtied the inode's table block; pin
    // it and submit it through the SAME SpanFlusher window so the data
    // spans and the inode slot settle in ONE device burst — the jbd2
    // "prepare everything, submit one burst, drain once" shape,
    // journal-less edition. The settle's snapshot guard keeps a RACED
    // table block dirty (another inode's write_inode updated a neighbor
    // slot mid-flight); ext4_sync_file_inner's ITB probe below then finds
    // the block clean in the common case — the second serialized device
    // round trip per fsync (~30 us on the fio rig) is gone.
    {
        let _itb_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock();
        let (tbl_blk, dev) = (
            super::inode::inode_table_block_of(fs_ref, inode.ino as u32),
            fs_ref.device,
        );
        if let Some(tbl_blk) = tbl_blk {
            let (mj, mi) = { ((*dev).major, (*dev).first_minor) };
            // SAFETY: probe/pin of the fs's ITB cache block; the pin is
            // balanced inside submit_span's settle (or the put below).
            {
                if bio::block_dirty_cached(dev, tbl_blk) {
                    let cache = crate::fs::bio::get_block_cache();
                    if let Some(bh) = cache.peek_buffer((mj, mi), tbl_blk) {
                        if bio::bh_is_dirty(&bh) {
                            let mut bhs = alloc::vec![bh];
                            let bytes: &[u8] = bio::bh_data(&bh);
                            flusher.submit_span(
                                dev,
                                tbl_blk * (4096 / 512),
                                bytes,
                                &mut bhs,
                                Some(bytes),
                            );
                        } else {
                            cache.put(bh);
                        }
                    }
                }
            }
        }
    }
    // Final window drain: every submitted span is device-acked (or left
    // dirty by its error status) before the flush returns.
    flusher.drain();
    total
}

/// Flush one contiguous run of pinned dirty pages (the run-batched flush
/// unit). Stages every in-file page's bytes into `stage`, writes the
/// whole run through a single ext4_write_at_core, then (on success)
/// persists the run's data blocks in coalesced multi-block requests.
/// Pages fully past the CURRENT VFS size are legitimate discards
/// (truncate won) and count as done without writing. On any short or
/// failed run write the run falls back to the per-page write core with
/// the bounded transient-failure retry (fi-fix-verify razor-6: only
/// persistent failure keeps a page dirty — the standing contract).
fn flush_run(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::inode::Inode,
    run: &[(u64, u64)],
    stage: &mut alloc::vec::Vec<u8>,
    done_out: &mut [(u64, u64, bool)],
    flusher: &mut SpanFlusher,
) {
    let ps = crate::mm::PAGE_SIZE as u64;
    let psu = crate::mm::PAGE_SIZE;
    let size = inode.get_size();
    let off0 = run[0].0 * ps;

    // i_size clamp (see ext4_flush_dirty_inode's doc comment): pages past
    // the CURRENT size are discards; the run's writable prefix is the
    // leading pages with off < size, its tail clamped mid-page.
    let mut run_pages = 0usize;
    while run_pages < run.len() && run[run_pages].0 * ps < size {
        run_pages += 1;
    }
    for k in run_pages..run.len() {
        done_out[k] = (run[k].0, run[k].1, true); // past-EOF discard
    }
    if run_pages == 0 {
        return;
    }
    let run_bytes = core::cmp::min(run_pages as u64 * ps, size - off0) as usize;

    // Stage the run's bytes from the pinned frames. run_bytes <= len*ps;
    // the (only) partial tail's uncovered bytes stay zero — they are
    // never written (the write clamps at run_bytes).
    if stage.len() < run_pages * psu {
        stage.resize(run_pages * psu, 0);
    }
    for k in 0..run_pages {
        let (idx, pfn) = run[k];
        let off = idx * ps;
        let n_bytes = core::cmp::min(psu as u64, size - off) as usize;
        // pfn belongs to a page-cache page PINNED by collect_dirty_range
        // (ref++, balanced by the caller's flush_done) — INV-CBYTES-4;
        // the linear map covers the whole frame.
        let frame =
            crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pfn * ps)).0 as *const u8;
        let page = crate::safety::bytes::pinned_frame_bytes(&frame, 0, psu);
        // The page's [0, n_bytes) prefix is live file data (n_bytes is
        // the EOF clamp); the uncovered tail of a partial page stays
        // zero — it is never written (the write clamps at run_bytes).
        let slot = &mut stage[k * psu..(k + 1) * psu];
        slot[..n_bytes].copy_from_slice(&page[..n_bytes]);
        slot[n_bytes..].fill(0);
    }

    // The skip set: every SOURCE frame of the run (the write core's
    // populate must not copy a frame onto itself, and must not revert a
    // mapper store that raced the staging copy — POSIX leaves
    // unsynchronized stores vs writeback unordered).
    let skip_pfns: alloc::vec::Vec<u64> = run[..run_pages].iter().map(|&(_, p)| p).collect();

    // SAFETY: same (fs, ino, inode) discipline as the fd write paths;
    // stage[..run_bytes] carries the pinned frames' bytes.
    let written = {
        ext4_write_at_core(
            fs,
            inode.ino as u32,
            Some(inode),
            off0,
            true,
            &stage[..run_bytes],
            &skip_pfns,
            false,
        )
    };
    if written == run_bytes as isize {
        // Persist the run's data blocks now, in coalesced multi-block
        // requests (best-effort: a block left dirty by a fallback inside
        // is owned by the standing durability points — eviction, the
        // fsync data loop, sync(2)).
        run_sync_spans(fs, inode, off0, run_bytes, stage, flusher);
        for k in 0..run_pages {
            done_out[k] = (run[k].0, run[k].1, true);
        }
        return;
    }

    // Short/failed run write: per-page fallback with the bounded retry
    // (razor-6). Each page is judged on its own clamped length (a mid-run
    // ENOSPC may legitimately cover fewer bytes than the run did).
    for k in 0..run_pages {
        let (idx, pfn) = run[k];
        let off = idx * ps;
        let n_bytes = core::cmp::min(psu as u64, size - off) as usize;
        let src = &stage[k * psu..k * psu + n_bytes];
        // SAFETY: same discipline as the run write; src borrows the
        // staged bytes (the pinned frames' copy).
        let mut written =
            { ext4_write_at_core(fs, inode.ino as u32, Some(inode), off, true, src, &[pfn], false) };
        if written != n_bytes as isize {
            let mut tries = 0u32;
            while tries < 3 {
                tries += 1;
                written = {
                    ext4_write_at_core(
                        fs,
                        inode.ino as u32,
                        Some(inode),
                        off,
                        true,
                        src,
                        &[pfn],
                        false,
                    )
                };
                if written == n_bytes as isize {
                    break;
                }
            }
        }
        // falloc-wedge capture: the persistent-failure face — the fill's
        // tail flushes failed wholesale while earlier inodes flushed
        // clean; name the write core's verdict (negative errno or short
        // count) for the first few failures per boot.
        if written != n_bytes as isize {
            static FAIL_PRINTS: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if FAIL_PRINTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
                crate::pr_err!(
                    "FLUSH-FAIL ino={} idx={} wrote={}/{} fs={:x}",
                    inode.ino,
                    idx,
                    written,
                    n_bytes,
                    fs as *const _ as u64
                );
            }
        }
        done_out[k] = (idx, pfn, written == n_bytes as isize);
    }
}

/// Persist a just-written run's data blocks in coalesced multi-block
/// requests (the run-batched flush's disk side). The write core left
/// every data block BH_Dirty in the 4 KiB buffer cache; with a cache
/// capacity of 8192 entries, a large file's flush used to turn over the
/// whole cache and pay one SERIALIZED synchronous 4 KiB write per dirty
/// victim at eviction time (measured: the mtest06-shape fsync/msync
/// walls, ~8192 dirty evictions per pass). Here the run's blocks are
/// resolved to physical, runs of PHYSICALLY CONSECUTIVE blocks are
/// assembled from their (pinned) buffer heads and pushed with ONE
/// blkdev_write per span — 16x fewer device round trips for contiguous
/// layouts — and their BH_Dirty bits cleared so eviction and the fsync
/// data loop skip them.
///
/// Ordering: data-before-metadata is the ordered-mode direction, so an
/// early data write is legal at any point of the flush; a block under a
/// pending publication constraint falls back to sync_dirty_buffer (which
/// satisfies the constraint first — the guard contract).
///
/// Span-write pipeline for the run-batched flush: submits consecutive
/// data-block spans to the device ASYNCHRONOUSLY (bio's WriteBatch — one
/// completion wait per window) and settles them (BH_Dirty clear + pin
/// release) at the drain, per-span by device-acked status.
///
/// Lives ACROSS runs of one ext4_flush_dirty_inode pass: a 128 MB flush is
/// ~2048 consecutive 64 KiB spans over ~2048 runs, and the batching win
/// comes from keeping the device busy across run boundaries (run N+1's
/// staging/journal work overlaps run N's writes). The final drain happens
/// at ext4_flush_dirty_inode's exit, so every span write is device-acked
/// before the caller proceeds to its journal commit — the ordered-mode
/// data-before-metadata direction is preserved exactly as with the
/// synchronous span writes.
///
/// Locking: the EXT4_BIG_LOCK discipline of run_sync_spans extends to the
/// drain — BH_Dirty clears and pin releases happen under the big lock
/// (task-recursive per INV-LOCK-8, so re-taking it inside a run_sync_spans
/// hold is legal), fencing the same racing write cores the synchronous
/// path was fenced against. A failed/timed-out span keeps its BH_Dirty
/// bits (the standing durability points retry), mirroring the sync path's
/// error contract.
struct SpanFlusher {
    batch: crate::fs::bio::WriteBatch,
    /// Per in-flight span (submission order): its pinned buffer heads.
    pending_bhs: alloc::vec::Vec<alloc::vec::Vec<*mut crate::fs::bio::BufferHead>>,
    /// Per in-flight span (submission order): the SETTLE GUARD snapshot
    /// (wrfsync lane, 2026-11-17). Empty = unguarded span (data spans:
    /// the flush owns the whole file and the write core re-reads the
    /// pinned frames' bytes — the pre-existing shape). Non-empty = the
    /// span's submitted bytes; at settle, BH_Dirty is cleared ONLY when
    /// the buffer still matches them. This is the jbd2 journal-head
    /// protection shape for the inode-table ride-along: the ITB block is
    /// SHARED by every inode of its group, so a racing write_inode can
    /// legitimately mutate a neighbor slot while our span is in flight —
    /// the buffer is already dirty and the mutation must NOT be cleared
    /// as persisted by our settle (the wrongly-cleared-BH_Dirty class).
    /// The raced block simply stays dirty for its own durability point;
    /// our span persists the older-but-valid snapshot (harmless).
    pending_snaps: alloc::vec::Vec<alloc::vec::Vec<u8>>,
    /// Device of the in-flight spans (one fs per flush pass).
    dev: Option<&'static crate::drivers::blkdev::GenDisk>,
}

impl SpanFlusher {
    fn new() -> Self {
        Self {
            batch: crate::fs::bio::WriteBatch::new(),
            pending_bhs: alloc::vec::Vec::new(),
            pending_snaps: alloc::vec::Vec::new(),
            dev: None,
        }
    }

    /// Submit one assembled span (bytes + its pinned buffer heads). Takes
    /// the bh Vec by value (capacity stays with the caller via swap-in of
    /// the taken Vec's remains); on a full window drains first, and on a
    /// device that cannot take the async write falls back to the
    /// synchronous single blkdev_write — the pre-plugging behavior.
    /// `snap` (Some) arms the settle guard for this span (see
    /// `pending_snaps`); pass None for unguarded data spans.
    fn submit_span(
        &mut self,
        dev: &'static crate::drivers::blkdev::GenDisk,
        sector: u64,
        bytes: &[u8],
        bhs: &mut alloc::vec::Vec<*mut crate::fs::bio::BufferHead>,
        snap: Option<&[u8]>,
    ) {
        self.dev = Some(dev);
        if !self.batch.submit(dev, sector, bytes) {
            // Window full or soft submission failure: drain the window
            // (frees device pending-table pressure), then try once more.
            self.drain();
            if !self.batch.submit(dev, sector, bytes) {
                // Synchronous fallback (device without async write, e.g.
                // a loop device, or persistent window lag): exactly the
                // pre-plugging span write. A write error leaves every
                // BH_Dirty bit set (the standing durability points retry).
                // No settle guard needed: the write COMPLETED before
                // return (under the caller's big-lock hold, so no racing
                // mutation can be in flight), and any later writer
                // re-dirties the buffer under its own lock.
                // dev is the fs's registered GenDisk; bytes covers exactly
                // the span's consecutive blocks.
                let rc = crate::drivers::blkdev::blkdev_write(dev, sector, bytes);
                if rc.is_ok() {
                    for &bh in bhs.iter() {
                        // SAFETY: bh is a pinned cached BufferHead whose
                        // content just persisted.
                        {
                            bio::bh_clear_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty)
                        };
                    }
                }
                for &bh in bhs.iter() {
                    // SAFETY: pinned by peek_buffer; balanced put.
                    crate::fs::bio::get_block_cache().put(bh);
                }
                bhs.clear();
                return;
            }
        }
        // Async path: the span's heads stay pinned and dirty until the
        // drain acks them.
        let mut owned = alloc::vec::Vec::new();
        core::mem::swap(&mut owned, bhs);
        self.pending_bhs.push(owned);
        self.pending_snaps
            .push(snap.map(|s| s.to_vec()).unwrap_or_default());
    }

    /// Drain the window: one completion wait for every in-flight span,
    /// then settle — BH_Dirty cleared only for device-acked spans, pins
    /// released for all. No-op when idle.
    fn drain(&mut self) {
        if self.batch.in_flight() == 0 {
            self.pending_bhs.clear();
            self.pending_snaps.clear();
            return;
        }
        // The settle runs under the big lock: same serialization vs
        // racing write cores as the span assembly itself (task-recursive
        // SleepMutex — re-entry under run_sync_spans' hold is legal).
        let _ext4_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock();
        self.batch.flush();
        let cache = crate::fs::bio::get_block_cache();
        for (i, bhs) in self.pending_bhs.drain(..).enumerate() {
            let snap = self.pending_snaps.get(i).map(|s| s.as_slice()).unwrap_or(&[]);
            if self.batch.status(i) == 0 {
                let mut off = 0usize;
                let mut settled = true;
                for &bh in bhs.iter() {
                    // SAFETY: bh is a pinned cached BufferHead; b_data is
                    // its block-sized buffer.
                    let cur = bio::bh_data(&bh);
                    if !snap.is_empty() {
                        if snap.len() - off < cur.len() || cur[..] != snap[off..off + cur.len()] {
                            // Settle guard tripped: the buffer changed
                            // while our span was in flight (racing
                            // write_inode on a shared ITB block). Keep the
                            // whole span's dirty bits — the mutation owns
                            // the next durability point.
                            settled = false;
                            break;
                        }
                        off += cur.len();
                    }
                }
                if settled {
                    for &bh in bhs.iter() {
                        // SAFETY: bh is a pinned cached BufferHead; the
                        // span's device write was acked AND the buffer
                        // still matches the persisted bytes.
                        {
                            bio::bh_clear_state_bit(&bh, crate::fs::bio::BufferState::BH_Dirty)
                        };
                    }
                }
            }
            for &bh in bhs.iter() {
                { cache.put(bh) };
            }
        }
        self.pending_snaps.clear();
    }
}

/// Best-effort by design: a block whose buffer is missing (evicted after
/// the write core dirtied it — the eviction write carried the same
/// bytes), clean (already persisted), or hole-mapped is skipped; the
/// standing durability points own whatever stays dirty.
///
/// PLUGGED SPANS (2026-11): spans are submitted ASYNCHRONOUSLY through
/// `SpanFlusher` (bio's WriteBatch) — N consecutive spans go to the device
/// with one completion wait at the end of the window (the blk-mq-plugging
/// shape), replacing the one-blkdev_write-one-round-trip serialization
/// that made the 128 MB fsync flush cost ~2048 serialized 64 KiB writes.
/// A span's BH_Dirty bits are cleared and its pins dropped only at the
/// window drain, and only for spans the device acked.
fn run_sync_spans(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &crate::fs::inode::Inode,
    off0: u64,
    run_bytes: usize,
    stage: &mut alloc::vec::Vec<u8>,
    flusher: &mut SpanFlusher,
) {
    // The buffer cache's block size is fixed at 4096 (bio::get_block_cache);
    // a different fs block size has no 1:1 page/block mapping — leave the
    // dirty buffers to the standing durability points.
    if fs.block_size != 4096 {
        return;
    }
    let bs: u64 = 4096;
    let sectors_per_block = bs / 512;
    let n_blocks = (run_bytes as u64 + bs - 1) / bs;
    let first_block = off0 / bs;
    // Stage sizing is THIS function's job (the flush-run caller pre-sizes
    // only its own run; the O_DIRECT post-write drain passes an empty
    // Vec): a maximal span covers every block of the range.
    let stage_need = n_blocks as usize * bs as usize;
    if stage.len() < stage_need {
        stage.resize(stage_need, 0);
    }
    // Serialized against racing write cores: the span assembly reads the
    // buffers' CURRENT bytes and the dirty-clear must not revert a
    // concurrent writer's newer buffer content (the same serialization
    // the write cores apply to each other; held across the span I/O like
    // the write core holds it across its breads).
    let _ext4_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock();
    // Fresh extent view: write core's refresh_cached_vfs_inode published
    // the just-written mapping into the cached sb copy under the big lock.
    let sbi = match super::sb_inode_of(inode) {
        Ok(b) => b,
        Err(_) => return,
    };
    let dev = fs.device;
    // SAFETY: dev is the fs's registered GenDisk; only identity fields
    // are read here.
    let (mj, mi) = { ((*dev).major, (*dev).first_minor) };
    let cache = crate::fs::bio::get_block_cache();

    // Span assembly state: pinned dirty buffer heads whose block numbers
    // are physically consecutive.
    let mut span_bhs: alloc::vec::Vec<*mut crate::fs::bio::BufferHead> =
        alloc::vec::Vec::with_capacity(16);
    let mut span_first_phys: u64 = 0;
    let mut span_len: u64 = 0;

    macro_rules! close_span {
        () => {
            if !span_bhs.is_empty() {
                // Assemble the span's bytes from the buffer heads (the
                // authoritative buffer-committed truth — if a racing
                // write core ran between the run write and this sync,
                // ITS newer bytes are what must persist). The big lock
                // held here makes the assembled copy stable until the
                // batch's private copy is taken at submit.
                let mut dst_off = 0usize;
                for &bh in span_bhs.iter() {
                    // SAFETY: bh is pinned (peek_buffer) and holds a
                    // 4096-byte b_data Vec.
                    let src = bio::bh_data(&bh);
                    stage[dst_off..dst_off + src.len()].copy_from_slice(src);
                    dst_off += src.len();
                }
                // PLUGGED SUBMIT: the span goes to the device without
                // waiting; settle (dirty-clear + unpin) happens at the
                // window drain for device-acked spans. Submission
                // failures drain-then-retry, then fall back to the
                // synchronous single write inside submit_span.
                flusher.submit_span(
                    dev,
                    span_first_phys * sectors_per_block,
                    &stage[..dst_off],
                    &mut span_bhs,
                    None,
                );
                span_bhs.clear();
                span_len = 0;
            }
        };
    }

    for bi in 0..n_blocks {
        let lblock = first_block + bi;
        // Extent/indirect walk over cached tree blocks (the run write
        // just touched them).
        let phys = match sbi.get_data_block(fs, lblock) {
            Ok(p) if p != 0 => p,
            _ => {
                close_span!();
                continue;
            }
        };
        // SAFETY: peek_buffer pins the returned head; every pin is
        // balanced by a put below or inside close_span!.
        let bh = match { cache.peek_buffer((mj, mi), phys) } {
            Some(b) => b,
            None => {
                // Evicted after the write core dirtied it: the eviction's
                // sync write carried these bytes — nothing to do.
                close_span!();
                continue;
            }
        };
        let dirty = bio::bh_is_dirty(&bh);
        if !dirty {
            { cache.put(bh) };
            close_span!();
            continue;
        }
        // Publication-constrained block: the guarded drain owns its
        // ordering — sync_dirty_buffer satisfies the constraint first.
        if crate::fs::bio::publication_constrained(mj, mi, phys) {
            // SAFETY: bh is a pinned dirty BufferHead.
            let _ = crate::fs::bio::sync_dirty_buffer(bh);
            { cache.put(bh) };
            close_span!();
            continue;
        }
        if span_len != 0 && phys == span_first_phys + span_len {
            span_bhs.push(bh);
            span_len += 1;
        } else {
            close_span!();
            span_bhs.push(bh);
            span_first_phys = phys;
            span_len = 1;
        }
    }
    close_span!();
}
/// Get or create the description's ReadAheadState — embedded in File
/// (file.ra, the reference's file_ra_state-in-struct-file shape; oc-rc:
/// the Box-in-private_data form paid a slab alloc + free pair per
/// open-then-read cycle). Initialized in place at first use; mutated
/// only under io_lock, the same serialization the Box form had.
/// Reference-default relatime atime gate (oc-rc): stamp the cached
/// atime only when atime < mtime, atime < ctime, the last stamp is
/// >= 24 h old, or atime was never set — the exact `relatime` rule a
/// stock reference kernel mount applies. The unconditional per-read
/// stamp this replaces paid a TSC read + divide + store on EVERY
/// read(2); the gate costs three L1 loads and compares, and the
/// candidate stamp comes from the tick-cached wall value (one relaxed
/// load; 0 means "not refreshed yet" and falls back to the exact
/// wall_secs()). Advisory field; readers tolerate torn u32. Disk
/// persistence is unchanged (atime reaches the disk only through the
/// same paths that persist mtime/ctime).
///
/// SAFETY: `ino` is the cached Ext4Inode box owned by the VFS inode;
/// callers hold the io_lock/big-lock serialization the direct store had.
/// INV-EXT4-ATIME twin for the buffered-write epilog: publish the
/// post-write size (monotonic max — delayed allocation may have already
/// published a longer dirty tail) and mtime/ctime. Advisory fields;
/// readers tolerate torn u32s; callers hold the big lock (the write
/// path's own serialization).
///
/// Contract: `ino` is the cached Ext4Inode box owned by the VFS inode;
/// valid until the inode's Drop (INV-EXT4-SBRES-1).
fn stamp_write_tail(ino: *mut super::inode::Ext4Inode, pub_end: u64, now: u32) {
    // The caller pins the Box (INV-EXT4-SBRES-1); the fields below are
    // advisory u32/u64 stores, volatile to keep the same torn-tolerant
    // discipline the direct store had.
    let i = crate::syscall::edges::boxed_mut(ino);
    // INV-EXT4-ATIME: advisory volatile stores into the pinned box;
    // readers tolerate torn values.
    if pub_end > i.size {
        crate::safety::bytes::store_volatile(&mut i.size, pub_end);
    }
    crate::safety::bytes::store_volatile(&mut i.mtime, now);
    crate::safety::bytes::store_volatile(&mut i.ctime, now);
}

fn stamp_atime_relatime(ino: *mut super::inode::Ext4Inode) {
    // INV-EXT4-ATIME: the caller pins the Box (io_lock-held read paths;
    // the sb box lives for the inode); the time fields are u32s
    // read-only here and written once below.
    let i = crate::syscall::edges::boxed_mut(ino);
    let now = {
        let cached = crate::drivers::rtc::cached_wall_secs();
        if cached != 0 {
            cached
        } else {
            crate::drivers::rtc::wall_secs() as u32
        }
    };
    let stale = now.wrapping_sub(i.atime) >= 86_400;
    if i.atime < i.mtime || i.atime < i.ctime || stale || i.atime == 0 {
        // The same advisory-field store (INV-EXT4-ATIME).
        crate::safety::bytes::store_volatile(&mut i.atime, now);
    }
}

fn get_or_create_ra_state(file: &File, block_size: u64) -> &mut ReadAheadState {
    // SAFETY: file.ra is written only at File::new (None) and here
    // under io_lock by the description's reader; readers of the state
    // hold the same lock.
    crate::fs::unsafe_edges::file_ra_state_mut(file)
        .get_or_insert_with(|| ReadAheadState::new(block_size))
}

/// Ext4 file operations structure (no close op since the readahead
/// state moved into File::ra — freed with the description itself).
pub static EXT4_FILE_OPS: FileOps = FileOps::builder()
    .read(ext4_file_read_vfs)
    .write(ext4_file_write_vfs)
    .lseek(reg_file_lseek)
    .read_at(ext4_file_read_at_vfs)
    .build();

/// ext4 positional read (R71): serve an explicit-offset read without
/// touching the shared file position at all.
///
/// The generic File::read_at fallback (set_pos/read/restore) let a
/// concurrent plain read() on another thread sharing this open file
/// description interleave the sequence: the demand-fault path filled a
/// mapping page from the OTHER thread's offset — a libc .text page served
/// .rodata bytes and every multithreaded loader (Xorg, gnome-session,
/// dbus-daemon) crashed or misbehaved on its own libraries. Reads here
/// take io_lock only to serialize internal state (readahead window) with
/// plain reads; the file position is never read or written.
///
/// SAFETY: file.inode.get() returns a valid pointer to Option<Arc<Inode>>;
/// when Some, the Arc and its contents (private_data, sb) are valid for
/// the lifetime of the file — identical setup to ext4_file_read_vfs.
fn ext4_file_read_at_vfs(file: &File, offset: u64, buf: &mut [u8]) -> isize {
    // sc105 attribution: page-shaped positional reads (<= 1 page at a
    // block boundary) are the mmap demand-fault signature. Time them so
    // KILLDUMP can state the fault path's share of the launcher burn.
    let fault_like = buf.len() <= crate::mm::PAGE_SIZE && offset % fs_block_size_of(file) == 0;
    let t0 = if fault_like { fault_tsc_now() } else { 0 };
    let ret = ext4_file_read_at_vfs_inner(file, offset, buf);
    if fault_like {
        use core::sync::atomic::Ordering::Relaxed;
        FAULT_READS.fetch_add(1, Relaxed);
        FAULT_CYCLES.fetch_add(fault_tsc_now().saturating_sub(t0), Relaxed);
    }
    ret
}

/// Cycle stamp for the fault-attribution timer (0 where no counter).
#[inline]
fn fault_tsc_now() -> u64 {
    #[cfg(feature = "x86_64")]
    {
        crate::arch::x86_64::cpu::read_counter()
    }
    #[cfg(not(feature = "x86_64"))]
    {
        0
    }
}

/// Block size of `file`'s filesystem (attribution helper above).
fn fs_block_size_of(file: &File) -> u64 {
    if let Some(fs) = crate::fs::unsafe_edges::file_inode(file)
        .and_then(|inode| {
            crate::fs::unsafe_edges::privdata_ref::<crate::fs::ext4::Ext4FileSystem>(inode)
        })
    {
        return fs.block_size as u64;
    }
    crate::mm::PAGE_SIZE as u64
}

fn ext4_file_read_at_vfs_inner(file: &File, offset: u64, buf: &mut [u8]) -> isize {
    let Some((inode, fs, ext4_inode)) = file_ext4(file) else {
        if file_inode_missing(file) {
            return errno::Errno::BadFileNumber.as_neg_i32() as isize;
        }
        return errno::Errno::IOError.as_neg_i32() as isize;
    };

    let _io_guard = file.io_lock.guard();

    // atime on a successful positional read, same as the shared-pos
    // read(2) paths (Linux file_accessed in vfs_read — pread64 is
    // the same path; filemap_fault's read_at demand fills are the
    // mmap-fault arm). Advisory volatile write, no disk I/O.
    let mut atime_pending = false;

    // No IN_ACCESS in either arm: this runs as ops.read_at behind
    // File::read_at, whose native-op arm owns the notify. The
    // op-level self-notify double-fired the event — masked by the
    // inotify last-event merge except when another watcher event
    // interleaved between the two fires (review CR-3).
    let ret = if file.flags_bits() & crate::fs::file::FileFlags::O_DIRECT != 0 {
        match ext4_file_read(fs, ext4_inode, offset, buf) {
            Ok(read_bytes) => {
                if read_bytes > 0 {
                    atime_pending = true;
                }
                read_bytes as isize
            }
            Err(e) => e as isize,
        }
    } else {
        let ra_state = get_or_create_ra_state(file, fs.block_size as u64);
        match ext4_file_read_cached(fs, ext4_inode, offset, buf, ra_state) {
            Ok(read_bytes) => {
                if read_bytes > 0 {
                    atime_pending = true;
                }
                read_bytes as isize
            }
            Err(e) => e as isize,
        }
    };
    if atime_pending {
        let atime = crate::drivers::rtc::wall_secs() as u32;
        // The cached Ext4Inode in inode.sb is a Box owned by this
        // VFS inode; advisory field (INV-EXT4-ATIME, same discipline
        // as ext4_file_read_vfs).
        {
            let i = crate::syscall::edges::boxed_mut(
                inode.sb.unwrap() as *mut super::inode::Ext4Inode,
            );
            // INV-EXT4-ATIME — one volatile store of the advisory u32;
            // readers tolerate torn values.
            crate::safety::bytes::store_volatile(&mut i.atime, atime);
        }
    }
    ret
}

/// Default regular file lseek implementation
fn reg_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    let inode = match crate::fs::unsafe_edges::file_inode(file) {
        Some(i) => i,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };

    // Get file size from cached VFS inode (avoids re-reading from disk)
    let file_size = inode.size.load(core::sync::atomic::Ordering::Relaxed) as i64;

    let current_pos = file.get_pos() as i64;
    let new_pos = match whence {
        0 => offset as i64,              // SEEK_SET
        1 => current_pos + offset as i64, // SEEK_CUR
        2 => file_size + offset as i64,   // SEEK_END
        _ => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };

    if new_pos < 0 {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }

    file.set_pos(new_pos as u64);
    new_pos as isize
}
