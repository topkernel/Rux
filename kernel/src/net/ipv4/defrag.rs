//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 fragment reassembly (W3)
//!
//! Minimal RFC 791 reassembly keyed by (src, dst, id, protocol):
//! - fragments are copied into a growable per-datagram buffer at their
//!   offsets, with a (offset,len) list for coverage tracking;
//! - completeness is a sorted coverage walk against the total length
//!   learned from the first MF=0 fragment;
//! - bounded against flooding: fixed entry pool, per-datagram byte cap
//!   and a global byte cap, plus expiry GC on every insertion
//!   (IP_FRAG_TIMEOUT, Linux-style drop-on-expiry).

use super::{IpHdr, IPHDR_LEN, ip_frag_flags};
use crate::sync::spinlock::Spinlock;

/// Concurrently reassembling datagrams (flooding bound).
const IP_FRAG_MAX_ENTRIES: usize = 8;
/// Per-datagram payload cap (largest legal IPv4 datagram is 65515).
const IP_FRAG_MAX_BYTES: usize = 64 * 1024;
/// Total buffered payload bytes across all entries (heap protection).
const IP_FRAG_TOTAL_MAX_BYTES: usize = 256 * 1024;
/// Seconds before an incomplete datagram is dropped (Linux: 30s).
const IP_FRAG_TIMEOUT_SECS: u64 = 30;

struct FragEntry {
    src: u32,
    dst: u32,
    id: u16,
    proto: u8,
    /// Jiffies deadline; entry is GC'd after it.
    expiry: u64,
    /// Copy of the (first-seen) IP header — patched on completion
    /// (frag_off=0, tot_len, recomputed checksum).
    hdr: [u8; IPHDR_LEN],
    /// Payload buffer written at fragment offsets (gaps zero-filled).
    data: alloc::vec::Vec<u8>,
    /// Received (offset, len) list — coverage tracking.
    frags: alloc::vec::Vec<(u32, u32)>,
    /// Total datagram length incl. header; 0 until the last fragment (MF=0).
    total_len: u32,
}

impl FragEntry {
    fn matches(&self, src: u32, dst: u32, id: u16, proto: u8) -> bool {
        self.src == src && self.dst == dst && self.id == id && self.proto == proto
    }

    /// Sum of unique fragment bytes is tracked incrementally via `data.len()`
    /// upper bound; the global cap uses the buffer lengths.
    fn buffered(&self) -> usize {
        self.data.len()
    }

    /// Coverage walk: is the payload [0, total_len - hdr) fully received?
    /// Sorts `frags` in place (order is irrelevant afterwards).
    fn complete(&mut self) -> bool {
        let Some(total) = self.total_len_checked() else {
            return false;
        };
        let needed = total as usize - IPHDR_LEN;
        self.frags.sort_unstable_by_key(|f| f.0);
        let mut covered = 0usize;
        for (off, len) in self.frags.iter() {
            let off = *off as usize;
            let end = off + *len as usize;
            if off > covered {
                return false; // hole
            }
            if end > covered {
                covered = end;
            }
            if covered >= needed {
                return true;
            }
        }
        covered >= needed
    }

    /// total_len sanity-checked against the buffer.
    fn total_len_checked(&self) -> Option<u32> {
        if self.total_len == 0 || self.total_len < IPHDR_LEN as u32 {
            return None;
        }
        Some(self.total_len)
    }
}

struct FragTable {
    entries: [Option<FragEntry>; IP_FRAG_MAX_ENTRIES],
}

impl FragTable {
    const fn new() -> Self {
        const NONE: Option<FragEntry> = None;
        Self { entries: [NONE; IP_FRAG_MAX_ENTRIES] }
    }
}

static IP_FRAG_TABLE: Spinlock<FragTable> = Spinlock::new(FragTable::new());

/// Feed one IP fragment in. On completion, re-dispatch the reassembled
/// datagram through the normal protocol dispatch (loopback-safe: the
/// rebuilt header has frag_off = 0 / MF = 0).
///
/// `ip_hdr` points at the fragment's own IP header; `payload` is the
/// fragment payload slice (`payload_off` its byte offset in the original
/// datagram, from frag_off*8).
pub fn ip_defrag(
    ip_hdr: &IpHdr,
    payload: &[u8],
    payload_off: u32,
    more_fragments: bool,
    src_ip: u32,
    dest_ip: u32,
) {
    let now = crate::drivers::timer::get_jiffies();
    let id = u16::from_be(ip_hdr.id);

    // The finished datagram is dispatched outside the table lock.
    let mut complete: Option<([u8; IPHDR_LEN], alloc::vec::Vec<u8>)> = None;

    {
        let mut table = IP_FRAG_TABLE.lock_irqsave();

        // GC expired entries and compute the global byte pressure.
        let mut total_bytes = 0usize;
        for slot in table.entries.iter_mut() {
            let expired = slot
                .as_ref()
                .map(|e| now >= e.expiry)
                .unwrap_or(false);
            if expired {
                *slot = None;
            } else {
                total_bytes += slot.as_ref().map(|e| e.buffered()).unwrap_or(0);
            }
        }

        // Find or create the matching entry.
        let mut target: Option<usize> = table
            .entries
            .iter()
            .position(|s| s.as_ref().map(|e| e.matches(src_ip, dest_ip, id, ip_hdr.protocol)).unwrap_or(false));
        if target.is_none() {
            // Reuse a free slot; if the pool is full, drop the fragment
            // (flooding pressure — Linux drops under memory pressure too).
            target = table.entries.iter().position(|s| s.is_none());
        }
        let Some(idx) = target else { return };
        if table.entries[idx].is_none() {
            if total_bytes >= IP_FRAG_TOTAL_MAX_BYTES {
                return; // global cap — drop
            }
            let mut hdr = [0u8; IPHDR_LEN];
            // SAFETY: ip_hdr aliases a validated repr(C) IpHdr of exactly
            // IPHDR_LEN bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    ip_hdr as *const IpHdr as *const u8,
                    hdr.as_mut_ptr(),
                    IPHDR_LEN,
                );
            }
            table.entries[idx] = Some(FragEntry {
                src: src_ip,
                dst: dest_ip,
                id,
                proto: ip_hdr.protocol,
                expiry: now + IP_FRAG_TIMEOUT_SECS * crate::drivers::timer::HZ,
                hdr,
                data: alloc::vec::Vec::new(),
                frags: alloc::vec::Vec::new(),
                total_len: 0,
            });
        }
        let entry = table.entries[idx].as_mut().unwrap();

        // R35 discipline: reservations BEFORE mutation so an OOM leaves the
        // entry consistent (fragment dropped, peer retransmits).
        let end = payload_off as usize + payload.len();
        let grow = end.saturating_sub(entry.data.len());
        if entry.data.len() + grow > IP_FRAG_MAX_BYTES
            || entry.data.try_reserve(grow).is_err()
            || entry.frags.try_reserve(1).is_err()
        {
            return;
        }
        if end > entry.data.len() {
            entry.data.resize(end, 0);
        }
        entry.data[payload_off as usize..end].copy_from_slice(payload);
        entry.frags.push((payload_off, payload.len() as u32));

        // The MF=0 fragment carries the total length.
        if !more_fragments {
            let total = payload_off + payload.len() as u32 + IPHDR_LEN as u32;
            if entry.total_len == 0 || total > entry.total_len {
                entry.total_len = total;
            }
        }

        if entry.complete() {
            let e = table.entries[idx].take().unwrap();
            // Patch: unfragmented header with the full length.
            let total = e.total_len_checked().unwrap() as u16;
            let ihl = e.hdr[0] & 0x0F;
            let mut hdr = e.hdr;
            let data = e.data;
            // SAFETY: hdr is a 20-byte aligned stack buffer.
            let h = unsafe { &mut *(hdr.as_mut_ptr() as *mut IpHdr) };
            h.tot_len = total.to_be();
            h.frag_off = 0;
            h.check = 0;
            if (ihl as usize) >= (IPHDR_LEN / 4) {
                h.check = h.compute_checksum().to_be();
            }
            let payload_len = total as usize - IPHDR_LEN;
            complete = Some((hdr, data[..payload_len].to_vec()));
        }
    } // lock released

    if let Some((hdr, payload)) = complete {
        dispatch_reassembled(hdr, &payload, src_ip, dest_ip);
    }
}

/// Rebuild a full-datagram skb and run the normal IP dispatch.
fn dispatch_reassembled(hdr: [u8; IPHDR_LEN], payload: &[u8], src_ip: u32, dest_ip: u32) {
    let mut skb = match crate::net::buffer::alloc_skb((IPHDR_LEN + payload.len()) as u32) {
        Some(s) => s,
        None => return, // drop — peer retransmits and we retry
    };
    // Header first, payload after (skb_put appends at the tail).
    if skb.skb_put_data(&hdr).is_err() || skb.skb_put_data(&payload).is_err() {
        skb.free();
        return;
    }
    // Parse the patched header fresh from the rebuilt packet.
    // SAFETY: skb holds exactly IPHDR_LEN + payload valid bytes.
    let hdr_ref = unsafe {
        let bytes = core::slice::from_raw_parts(skb.data, skb.len as usize);
        match IpHdr::from_bytes(bytes) {
            Some(h) => h,
            None => {
                skb.free();
                return;
            }
        }
    };
    super::ip_dispatch(&mut skb, hdr_ref, src_ip, dest_ip);
    skb.free();
}

/// Fragment offset helper: (offset_bytes, mf).
pub fn frag_fields(ip_hdr: &IpHdr) -> (u32, bool) {
    let raw = u16::from_be(ip_hdr.frag_off);
    let off = (raw & ip_frag_flags::OFFSET_MASK) as u32 * 8;
    let mf = (raw & ip_frag_flags::MF) != 0;
    (off, mf)
}
