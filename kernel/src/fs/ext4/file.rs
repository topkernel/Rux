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

    let blocks = inode.get_data_blocks(fs)?;
    let block_size = fs.block_size as usize;

    let mut total_read = 0;
    let mut current_offset = offset as usize;
    let mut buf_offset = 0;

    while total_read < to_read {
        let block_index = current_offset / block_size;
        let block_offset = current_offset % block_size;

        if block_index >= blocks.len() {
            break;
        }

        // Sparse hole (block 0 = unallocated): serve ZEROES, never physical
        // block 0 (superblock-backup garbage) — review 5.5 (稀疏洞读盘块 0,
        // 非缓存路径).
        if blocks[block_index] == 0 {
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

        // SAFETY: bio::bread returns a valid pinned BufferHead on success;
        // b_data points to a block-sized buffer aligned to block_size.
        unsafe {
            let bh = bio::bread(fs.device, blocks[block_index])
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &(*bh).b_data;
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
}

impl ReadDst {
    #[inline]
    fn len(&self) -> usize {
        match self {
            ReadDst::Kernel(_, n) | ReadDst::User(_, n) => *n,
        }
    }

    /// Copy `bytes` from a cached page into this destination at `off`.
    /// Returns bytes actually copied (a user fault truncates the read).
    ///
    /// # Safety
    /// `src` must hold `bytes` readable bytes; Kernel variant pointer must
    /// have `len` writable bytes, User variant must be access_ok-validated.
    unsafe fn put(&self, off: usize, src: *const u8, bytes: usize) -> usize {
        match self {
            ReadDst::Kernel(dst, _) => {
                // SAFETY: caller guarantees dst+off+bytes is in bounds.
                unsafe { core::ptr::copy_nonoverlapping(src, dst.add(off), bytes) };
                bytes
            }
            ReadDst::User(dst, _) => {
                // SAFETY: exception-table copy; uncopied bytes truncate.
                let uncopied = unsafe {
                    crate::arch::riscv64::uaccess::copy_to_user(dst.add(off), src, bytes)
                };
                bytes - uncopied
            }
        }
    }

    /// Zero-fill `bytes` at `off` (sparse holes).
    ///
    /// # Safety
    /// Same destination validity as `put`.
    unsafe fn put_zeroes(&self, off: usize, bytes: usize) -> usize {
        // The zero page: ext4 block_size-sized static is overkill — copy in
        // 4 KiB chunks from a small const zero block.
        static ZEROS: [u8; 4096] = [0u8; 4096];
        let mut done = 0usize;
        while done < bytes {
            let n = core::cmp::min(4096, bytes - done);
            let c = unsafe { self.put(off + done, ZEROS.as_ptr(), n) };
            if c < n {
                break; // user fault: short read
            }
            done += c;
        }
        done
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
        let run_len = cache.get_range_into(fs_id, ino, page_index, &mut run[..want]);
        if run_len > 0 {
            // Copy each pinned page's contribution.
            for (i, (_idx, page_data)) in run[..run_len].iter().enumerate() {
                let in_page_off = if i == 0 { page_offset } else { 0 };
                let remaining_now = to_read - total_read;
                // SAFETY: page_data is a valid page-aligned pointer of
                // block_size bytes pinned by get_range_into; the copy length
                // is bounded by the page size and the remaining request bytes.
                unsafe {
                    let avail = block_size_usize - in_page_off;
                    let copy_len = core::cmp::min(remaining_now, avail);
                    let copied = dst.put(buf_offset, page_data.add(in_page_off), copy_len);
                    total_read += copied;
                    buf_offset += copied;
                    current_offset += copied as u64;
                    if copied < copy_len {
                        // User fault: short read, stop.
                        cache.put_range(fs_id, ino, &run[..run_len]);
                        return Ok(total_read);
                    }
                }
            }
            cache.put_range(fs_id, ino, &run[..run_len]);
            continue;
        }

        // Cache miss on the first page: batch-fill from here. The window
        // covers the demand range plus (on sequential access) the read-ahead
        // window ahead of it; pages land in the cache and the loop serves
        // them through the fast path above.
        let seq_contd = offset == ra_state.last_read_end || ra_state.active;
        let window = if seq_contd || offset == 0 {
            core::cmp::max(demand_pages, crate::fs::readahead::MAX_READAHEAD_BLOCKS as u64)
        } else {
            // Random seek: read only what the request needs.
            demand_pages
        };
        let fill_upto = core::cmp::min(page_index + window, file_pages);
        fill_page_cache_batch(fs, inode, cache, fs_id, ino, page_index, fill_upto);

        // The fill skips unallocated pages (holes) but may also have bailed
        // early (I/O or metadata error, OOM). Verify against the block map:
        // an ALLOCATED-but-uncached page gets a synchronous single-block
        // read here; only true holes (block 0) zero-fill.
        if cache.get(fs_id, ino, page_index).is_none() {
            let block_nr = inode.get_data_block(fs, page_index).unwrap_or(0);
            if block_nr != 0 {
                // SAFETY: bio::bread returns a valid pinned BufferHead on
                // success; b_data points to a block-sized buffer.
                unsafe {
                    if let Some(bh) = bio::bread(fs.device, block_nr) {
                        let data = &(*bh).b_data;
                        let avail = block_size_usize - page_offset;
                        let copy_len = core::cmp::min(remaining, avail);
                        let copied = dst.put(buf_offset, data.as_ptr().add(page_offset), copy_len);
                        total_read += copied;
                        buf_offset += copied;
                        current_offset += copied as u64;
                        cache.insert(fs_id, ino, page_index, block_nr, data);
                        bio::brelse(bh);
                        continue;
                    }
                }
                // Synchronous read failed: report I/O error rather than
                // silently returning zeros for real data.
                return Err(errno::Errno::IOError.as_neg_i32());
            }
            // True hole: zero-fill in place.
            let avail = block_size_usize - page_offset;
            let zero_len = core::cmp::min(remaining, avail);
            // SAFETY: dst validity documented on ReadDst::put_zeroes.
            let copied = unsafe { dst.put_zeroes(buf_offset, zero_len) };
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

    let mut completions: [IoCompletion; MAX_BATCH] = core::array::from_fn(|_| IoCompletion::new());
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
            idx += 1;
            continue;
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

        match bio::bread_async_multi(fs.device, block_nr, nblocks, &completions[count]) {
            Some(bh) => {
                bh_ptrs[count] = bh;
                ra_idx[count] = run_first;
                ra_blk[count] = block_nr;
                ra_len[count] = nblocks;
                count += 1;
                blocks_in_batch += n;
                idx += n;
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
                        unsafe {
                            if let Some(bh) = bio::bread(fs.device, block_nr + k) {
                                let data = &(*bh).b_data;
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
    crate::drivers::virtio::pci_blk_kick();
    for i in 0..count {
        status[i] = bio::bread_wait(bh_ptrs[i], &completions[i]);
    }
    // Insert completed pages into the page cache under their own indices.
    for i in 0..count {
        // SAFETY: bread_wait has completed, so bh_ptrs[i] points to a valid
        // BufferHead with fully populated b_data (status 0 only).
        unsafe {
            if status[i] == 0 {
                let data = &(*bh_ptrs[i]).b_data;
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
                bio::brelse(bh_ptrs[i]);
            } else {
                // Multi-block buffers are private: free directly.
                bio::bfree_multi(bh_ptrs[i]);
            }
        }
    }
}

pub fn ext4_file_write(
    fs: &crate::fs::ext4::Ext4FileSystem,
    inode: &mut crate::fs::ext4::inode::Ext4Inode,
    offset: u64,
    buf: &[u8],
) -> Result<usize, i32> {
    let block_size = fs.block_size as u64;
    let to_write = buf.len() as u64;

    // Calculate required block count
    let end_offset = offset + to_write;
    let needed_blocks = (end_offset + block_size - 1) / block_size;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let sectors_per_block = (fs.block_size / 512) as u64;

    // If new blocks are needed, allocate them
    if needed_blocks > current_blocks {
        allocate_blocks_for_file(fs, inode, needed_blocks)?;
    }

    // Write data
    let mut total_written = 0;
    let mut current_offset = offset;
    let mut buf_offset = 0;

    while total_written < to_write as usize {
        let block_index = current_offset / block_size;
        let block_offset = (current_offset % block_size) as usize;

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
                    let mut ext_meta: u64 = 0;
                    match crate::fs::ext4::extent::ext4_ext_materialize_block(
                        fs, &mut inode.block, block_index, &mut ext_meta,
                    ) {
                        Ok(Some(phys)) => {
                            // Materialize the block contents now: the
                            // read-modify-write below breads it; an
                            // unzeroed block would splice garbage around
                            // the written span.
                            // SAFETY: fs.device is a valid GenDisk; phys is
                            // a freshly allocated block whose disk contents
                            // are irrelevant — getblk_zero creates a zeroed
                            // dirty buffer without the disk read.
                            unsafe {
                                if let Some(bh) = bio::getblk_zero(fs.device, phys) {
                                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                                    bio::brelse(bh);
                                } else {
                                    return Err(errno::Errno::IOError.as_neg_i32());
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
                            let new_block = allocator.alloc_block(goal_group)?;
                            {
                                let mut hm: u64 = 0;
                                crate::fs::ext4::extent::ext4_ext_insert_written(
                                    fs, &mut inode.block, block_index, new_block, 1, &mut hm,
                                )?;
                                inode.blocks += hm;
                            }
                            // SAFETY: freshly allocated block; getblk_zero
                            // fabricates the zeroed dirty buffer without a
                            // disk read (full-block overwrite follows).
                            unsafe {
                                if let Some(bh) = bio::getblk_zero(fs.device, new_block) {
                                    bio::brelse(bh);
                                } else {
                                    return Err(errno::Errno::IOError.as_neg_i32());
                                }
                            }
                            inode.blocks += sectors_per_block;
                            new_block
                        }
                        Err(e) => return Err(e),
                    }
                } else {
                    // Block not allocated, need to allocate a new one for writing
                    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                    let goal_group = (inode.ino / fs.inodes_per_group).min(fs.group_count - 1);
                    let new_block = match allocator.alloc_block(goal_group) {
                        Ok(b) => b,
                        Err(e) => return Err(e),
                    };

                    // Zero the new block (deferred-dirty: the write loop
                    // below leaves the data buffer BH_Dirty; the block
                    // cache write-back persists it — see ext4_file_write).
                    // SAFETY: bio::bread returns a valid BufferHead;
                    // b_data is a block-sized writable buffer.
                    unsafe {
                        let bh = bio::bread(fs.device, new_block)
                            .ok_or(errno::Errno::IOError.as_neg_i32())?;

                        for byte in (*bh).b_data.iter_mut() {
                            *byte = 0;
                        }
                        (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                        bio::brelse(bh);
                    }

                    // Update inode block pointer
                    if block_index < 12 {
                        inode.block[block_index as usize] = new_block as u32;
                    } else {
                        // Handle indirect blocks
                        allocate_indirect_block(fs, inode, block_index, new_block, &allocator, goal_group)?;
                    }
                    inode.blocks += sectors_per_block;

                    new_block
                }
            }
            Ok(b) => b,
            Err(e) => return Err(e),
        };

        // SAFETY: bio::bread returns a valid BufferHead; b_data is a writable
        // block-sized buffer. The slice ranges are bounded by block_size.
        unsafe {
            let bh = bio::bread(fs.device, block_num)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &mut (*bh).b_data;
            let remaining = to_write as usize - total_written;
            let available_in_block = block_size as usize - block_offset;
            let write_in_block = core::cmp::min(remaining, available_in_block);

            // Write data to block
            data[block_offset..block_offset + write_in_block]
                .copy_from_slice(&buf[buf_offset..buf_offset + write_in_block]);

            // Deferred write-back: leave the buffer BH_Dirty in the block
            // cache instead of syncing it here. A synchronous write per
            // write(2) (~10-20ms of virtio wait under TCG) made any
            // small-write workload — every LTP test's setup — blow the
            // 30s wall clock. Durability now comes from fsync
            // (ext4_sync_file syncs the file's dirty data blocks), the
            // buffer-cache eviction path (dirty victims are synced before
            // reuse), and sync(2).
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            bio::brelse(bh);

            total_written += write_in_block;
            buf_offset += write_in_block;
            current_offset += write_in_block as u64;
        }
    }

    // Update file size
    if end_offset > inode.get_size() {
        inode.set_size(end_offset);
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
) -> Result<(), i32> {
    let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
    let block_size = fs.block_size as u64;
    let current_blocks = (inode.get_size() + block_size - 1) / block_size;
    let sectors_per_block = (fs.block_size / 512) as u64;
    let goal_group = (inode.ino / fs.inodes_per_group).min(fs.group_count - 1);

    // Check if file uses extents
    if inode.has_extent() {
        return allocate_blocks_with_extents(fs, inode, needed_blocks, current_blocks, &allocator, goal_group);
    }

    // Allocate new blocks (indirect block mode). Run-based like the
    // extents path: one bitmap pass per contiguous run instead of one per
    // block — a 300MB fallocate (LTP tst_acquire_device) is 76800 bitmap
    // read/write/desc/superblock round trips otherwise.
    let mut i = current_blocks;
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
            unsafe {
                let bh = bio::bread(fs.device, data_block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                for byte in (*bh).b_data.iter_mut() {
                    *byte = 0;
                }

                (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                bio::brelse(bh);
            }
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
            // SAFETY: inode.block is 60 bytes; the 12-byte header is in-bounds.
            let header = unsafe { &mut *(inode.block.as_mut_ptr() as *mut Ext4ExtentHeader) };
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
        crate::fs::ext4::extent::ext4_ext_append(fs, &mut inode.block, &ext, &mut meta_sectors)?;
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
            unsafe {
                let bh = bio::bread(fs.device, b)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;
                for byte in (*bh).b_data.iter_mut() {
                    *byte = 0;
                }
                (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                bio::brelse(bh);
            }
        }

        // Valid (possibly empty) extent header on the root.
        {
            // SAFETY: inode.block is 60 bytes; header at its start is
            // in-bounds.
            let header = unsafe { &mut *(inode.block.as_mut_ptr() as *mut Ext4ExtentHeader) };
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
        crate::fs::ext4::extent::ext4_ext_append(fs, &mut inode.block, &ext, &mut meta_sectors)?;
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
            unsafe {
                let bh = bio::bread(fs.device, indirect_block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                for byte in (*bh).b_data.iter_mut() {
                    *byte = 0;
                }

                (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
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

                // Zero double indirect block
                // SAFETY: bio::bread returns a valid BufferHead; b_data is a
                // block-sized writable buffer. We zero it to initialize all
                // second-level pointers.
                unsafe {
                    let bh = bio::bread(fs.device, double_block)
                        .ok_or(errno::Errno::IOError.as_neg_i32())?;

                    for byte in (*bh).b_data.iter_mut() {
                        *byte = 0;
                    }

                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    let sync_res = bio::sync_dirty_buffer(bh);
                    bio::brelse(bh);
                    sync_res?;
                }
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

                // Zero single indirect block
                // SAFETY: bio::bread returns a valid BufferHead; b_data is a
                // block-sized writable buffer. We zero it to initialize block
                // pointers.
                unsafe {
                    let bh = bio::bread(fs.device, indirect_block)
                        .ok_or(errno::Errno::IOError.as_neg_i32())?;

                    for byte in (*bh).b_data.iter_mut() {
                        *byte = 0;
                    }

                    (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
                    let sync_res = bio::sync_dirty_buffer(bh);
                    bio::brelse(bh);
                    sync_res?;
                }

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
) -> Result<(), i32> {
    // Sync all data blocks of file
    let blocks = inode.get_data_blocks(fs)?;

    for block in blocks {
        if block == 0 {
            continue; // sparse hole
        }
        // SAFETY: bio::bread returns a valid BufferHead on success.
        unsafe {
            let bh = bio::bread(fs.device, block)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let sync_res = if (*bh).is_dirty() {
                bio::sync_dirty_buffer(bh)
            } else {
                Ok(())
            };
            bio::brelse(bh);
            sync_res?;
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
    }

    // The lazy write-back (bio::sync_dirty_buffer defers while a journal
    // handle is active) also leaves UNREGISTERED metadata dirty in the
    // buffer cache — block bitmaps, group descriptors, extent-tree and
    // directory blocks from the same update. fsync semantics require those
    // on disk too, so flush the whole cache; the common case (nothing new
    // dirty) scans the hash buckets only.
    bio::sync_buffers()?;

    Ok(())
}

// ============================================================================
// VFS Wrapper Functions
// ============================================================================

/// VFS read wrapper - calls ext4_file_read_cached with page cache and read-ahead
pub fn ext4_file_read_vfs(file: &File, buf: &mut [u8]) -> isize {    // SAFETY: file.inode.get() returns a valid pointer to Option<Arc<Inode>>;
    // when Some, the Arc and its contents (private_data, sb) are valid for the
    // lifetime of the file. The ext4 filesystem pointer in private_data and the
    // cached Ext4Inode in inode.sb are set during file open and remain valid.
    unsafe {
        // Get VFS inode from file
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt {
            Some(i) => i,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };

        // Get ext4 filesystem pointer from inode's private_data
        let fs_ptr = match inode.private_data {
            Some(ptr) => ptr as *const crate::fs::ext4::Ext4FileSystem,
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };
        let fs = &*fs_ptr;

        // Use cached Ext4Inode from inode.sb instead of re-reading from disk
        let ext4_inode = match inode.sb {
            Some(ptr) => &*(ptr as *const super::inode::Ext4Inode),
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };

        // Get current file position
        let offset = file.get_pos() as u64;

        // P2 O_DIRECT: bypass the page cache entirely and serve straight
        // from the block layer (ext4_file_read walks bio::bread). The
        // write side already runs uncached-synchronous (ext4_file_write),
        // so only the read path needs the branch. Simplification vs Linux:
        // offset/length block alignment is not enforced.
        if file.flags_bits() & crate::fs::file::FileFlags::O_DIRECT != 0 {
            match ext4_file_read(fs, ext4_inode, offset, buf) {
                Ok(read_bytes) => {
                    file.set_pos(offset + read_bytes as u64);
                    if read_bytes > 0 {
                        crate::fs::inotify::notify_file(
                            file,
                            crate::fs::inotify::inotify_events::IN_ACCESS,
                        );
                    }
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
                // atime: keep the CACHED inode in sync with the read (no
                // disk write — same wall-clock seconds ext4_setattr would
                // persist for mtime/ctime).
                if read_bytes > 0 {
                    let atime = crate::drivers::rtc::wall_secs() as u32;
                    // SAFETY: the cached Ext4Inode in inode.sb is a Box
                    // owned by this VFS inode; the big lock above
                    // serializes writers, and readers of atime tolerate
                    // torn u32 values (advisory field).
                    unsafe {
                        core::ptr::write_volatile(
                            &mut (*(inode.sb.unwrap() as *mut super::inode::Ext4Inode)).atime,
                            atime,
                        );
                    }
                }
                read_bytes as isize
            }
            Err(e) => e as isize,
            }
        }
    }
}

/// read(2) fast path for ext4 regular files: copy straight from the page
/// cache into user memory (no syscall-layer staging buffer).
///
/// `dst` must be access_ok-validated for `count` bytes. O_DIRECT falls back
/// to the caller's staged path (rare, unaligned semantics kept simple).
pub fn ext4_file_read_user_vfs(file: &File, dst: *mut u8, count: usize) -> isize {
    // SAFETY: same inode/fs lifetime guarantees as ext4_file_read_vfs.
    unsafe {
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt {
            Some(i) => i,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };
        let fs_ptr = match inode.private_data {
            Some(ptr) => ptr as *const crate::fs::ext4::Ext4FileSystem,
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };
        let fs = &*fs_ptr;
        let ext4_inode = match inode.sb {
            Some(ptr) => &*(ptr as *const super::inode::Ext4Inode),
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };
        let offset = file.get_pos() as u64;

        let ra_state = get_or_create_ra_state(file, fs.block_size as u64);
        match ext4_file_read_cached_dst(
            fs, ext4_inode, offset,
            ReadDst::User(dst, count), ra_state,
        ) {
            Ok(read_bytes) => {
                file.set_pos(offset + read_bytes as u64);
                if read_bytes > 0 {
                    let atime = crate::drivers::rtc::wall_secs() as u32;
                    // SAFETY: same as ext4_file_read_vfs (advisory field,
                    // big lock serializes writers).
                    unsafe {
                        core::ptr::write_volatile(
                            &mut (*(inode.sb.unwrap() as *mut super::inode::Ext4Inode)).atime,
                            atime,
                        );
                    }
                }
                read_bytes as isize
            }
            Err(e) => e as isize,
        }
    }
}

/// True when `file` uses the ext4 regular-file ops (fast-path dispatch for
/// the read(2) syscall layer).
pub fn is_ext4_file(file: &File) -> bool {
    file.get_ops()
        .is_some_and(|ops| core::ptr::eq(ops as *const _, &EXT4_FILE_OPS as *const _))
}

/// VFS write wrapper - calls ext4_file_write
pub fn ext4_file_write_vfs(file: &File, buf: &[u8]) -> isize {
    // SAFETY: file.inode.get() returns a valid pointer to Option<Arc<Inode>>;
    // when Some, the Arc and its contents (private_data, sb) are valid for the
    // lifetime of the file. The ext4 filesystem pointer and cached inode in
    // inode.sb are set during file open and remain valid.
    unsafe {
        // Get VFS inode from file
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt {
            Some(i) => i,
            None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
        };

        // Get ext4 filesystem pointer from inode's private_data
        let fs_ptr = match inode.private_data {
            Some(ptr) => ptr as *const crate::fs::ext4::Ext4FileSystem,
            None => return errno::Errno::IOError.as_neg_i32() as isize,
        };
        let fs = &*fs_ptr;
        let ext4_ino = inode.ino as u32;

        // SMP serialization for the write path (review 5.5 high: 写路径/位图
        // RMW 无锁——SMP 位图丢更新双分配): allocation, bitmap RMW, group
        // descriptor updates and the inode rewrite all happen under the
        // ext4 big lock, same as namei.
        let _ext4_guard = crate::fs::ext4::EXT4_BIG_LOCK.lock_fair();

        // Start a journal transaction for data=ordered semantics:
        // data blocks are synced during write, then the inode metadata is
        // committed to the journal with all data already on disk.
        let use_journal = fs.journal.is_some();
        let mut journal_handle = match if use_journal {
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
            super::namei::set_current_handle(h);
        }

        // Read ext4 inode from disk (write needs fresh on-disk data)
        let mut ext4_inode = match fs.read_inode(ext4_ino) {
            Ok(inode) => inode,
            Err(e) => {
                if journal_handle.is_some() {
                    super::namei::clear_current_handle();
                    if let Some(mut h) = journal_handle {
                        let _ = super::journal::ext4_journal_stop(&mut h);
                    }
                }
                return e as isize;
            }
        };

        // Get current file position (O_APPEND: always write at end of file)
        let offset = if file.flags().bits() & crate::fs::file::FileFlags::O_APPEND != 0 {
            ext4_inode.get_size()
        } else {
            file.get_pos() as u64
        };

        // Call internal write function
        let result = match ext4_file_write(fs, &mut ext4_inode, offset, buf) {
            Ok(written_bytes) => {
                // Update cached copy in inode.sb
                if let Some(ptr) = inode.sb {
                    let cached = &mut *(ptr as *mut super::inode::Ext4Inode);
                    cached.block = ext4_inode.block;
                    cached.size = ext4_inode.size;
                    cached.blocks = ext4_inode.blocks;
                    cached.mtime = ext4_inode.mtime;
                    cached.ctime = ext4_inode.ctime;
                }
                // Update cached VFS inode size
                inode.size.store(ext4_inode.get_size(), core::sync::atomic::Ordering::Relaxed);
                // Write back inode to disk (registers with journal if handle active)
                match crate::fs::ext4::inode::write_inode(fs, ext4_ino, &ext4_inode) {
                    Ok(()) => {
                        // Invalidate page cache for this inode after write
                        page_cache::get_page_cache().invalidate_inode(fs as *const crate::fs::ext4::Ext4FileSystem as u64, ext4_ino as u64);
                        // Update file position
                        file.set_pos(offset + written_bytes as u64);
                        written_bytes as isize
                    }
                    Err(e) => e as isize,
                }
            }
            Err(e) => e as isize,
        };

        // Stop journal transaction
        if journal_handle.is_some() {
            super::namei::clear_current_handle();
            if let Some(mut h) = journal_handle {
                let _ = super::journal::ext4_journal_stop(&mut h);
            }
        }

        result
    }
}

/// Get or create ReadAheadState stored in file.private_data.
unsafe fn get_or_create_ra_state<'a>(file: &File, block_size: u64) -> &'a mut ReadAheadState {
    let ptr = file.private_data.get();
    if let Some(state_ptr) = *ptr {
        &mut *(state_ptr as *mut ReadAheadState)
    } else {
        let state = alloc::boxed::Box::new(ReadAheadState::new(block_size));
        let state_ptr = alloc::boxed::Box::into_raw(state);
        *ptr = Some(state_ptr as *mut u8);
        &mut *state_ptr
    }
}

/// Close callback — free ReadAheadState from file.private_data.
fn ext4_file_close(file: &File) -> i32 {
    // SAFETY: If state_ptr is Some, it was created by Box::into_raw in
    // get_or_create_ra_state, so Box::from_raw is the correct way to free it.
    // Setting *ptr to None prevents double-free on subsequent close calls.
    unsafe {
        let ptr = file.private_data.get();
        if let Some(state_ptr) = *ptr {
            let _ = alloc::boxed::Box::from_raw(state_ptr as *mut ReadAheadState);
            *ptr = None;
        }
    }
    0
}

/// Ext4 file operations structure
pub static EXT4_FILE_OPS: FileOps = FileOps {
    read: Some(ext4_file_read_vfs),
    write: Some(ext4_file_write_vfs),
    lseek: Some(reg_file_lseek),
    close: Some(ext4_file_close),
    poll: None,
};

/// Default regular file lseek implementation
fn reg_file_lseek(file: &File, offset: isize, whence: i32) -> isize {
    // SAFETY: file.inode.get() returns a valid pointer set during file open;
    // the Option<Arc<Inode>> it points to remains valid for the file's lifetime.
    let inode_opt = unsafe { &*file.inode.get() };
    let inode = match inode_opt {
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
