//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The net subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `unsafe.rs` nor `types.rs`, in one file.
//!
//! Sections follow the order of the old module declarations; each banner
//! names the topic file it was (file_arc was file_arc.rs) and the file's
//! module doc follows it as the section's header comment. What lived
//! directly in mod.rs — the three socket-file Arc edges — is the first
//! section. Inside a section the items keep their original order and
//! text. Type definitions are in `types.rs`, the audited unsafe edges in
//! `unsafe.rs`.
//!
//! None of the old files carried a per-file `#![forbid(unsafe_code)]`
//! inner attribute, so none was dropped; the scattered unsafe of the
//! sections is collected in pass 3.
//!
//! The file's NAME overstates until the shrink stage has run: it holds
//! all of the subsystem's code under the hot-path name, not a verified
//! set of hot-path functions (§2c.3, user decision 2026-11-25).
//!
//! Two renames were forced by the merge: the mod.rs root items `file_arc`
//! and `take_file_arc` collide with the live twins of the same names in
//! file_arc.rs (the ones the net service crate re-exports at its root),
//! so the dead mod.rs copies take their file name as prefix
//! (`mod_file_arc`, `mod_take_file_arc`); `reclaim_file_arc` had no twin
//! and keeps its name. No caller changes: the mod.rs trio has none (each
//! is a never-used `pub(crate)` fn, dead-code warned at base).

use super::types::{EthProtocol, IpProtocol, SkBuff};
use core::sync::atomic::AtomicU64;

// pass 3 (seed-core §2c.5): the audited unsafe operations moved to
// net::r#unsafe; the historical net::hotpath:: paths of the pub ones
// stay valid — the net service crate (its lib.rs root re-exports) and
// the ext4 crate resolve file_arc / take_file_arc / hdr_view /
// hdr_view_mut through here.
pub use super::r#unsafe::{file_arc, hdr_view, hdr_view_mut, take_file_arc};


// ═══════════════ net root ═════════════════════════════════════════
// (was mod.rs: its module declarations and its re-export of buffer's
//  items are gone — every path names types or hotpath now; the file's
//  module doc and its api note follow as this section's header comment)
// Network Subsystem

// The network hook table (NetHooks) is crate::api::net.

// ==================== Socket-file Arc edges (round-5) ====================


// ═══════════════ file_arc ═════════════════════════════════════════
// (was file_arc.rs)
// The net stack's file→Arc identity helpers (framework side; crate-flip
// rung 2): the raw adoption edge lives framework-side because the crate
// is forbid-enforced. The type claim is the caller's ops-identity check.



// ═══════════════ buffer ═════════════════════════════════════════
// (was buffer.rs)
// Network Buffer (SkBuff)

// (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks
// with headroom-tracked data pointers (alloc/reserve/put maintain the
// span invariants); data_slice_at rows are bounds-checked against the
// len tail; Send/Sync carried by the head's single-owner discipline.)

// (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks
// with headroom-tracked data pointers (alloc/reserve/put maintain the
// span invariants); data_slice_at rows are bounds-checked against the
// len tail; Send/Sync carried by the head's single-owner discipline.)


impl EthProtocol {
    /// Convert from u16
    pub fn from_u16(val: u16) -> Option<Self> {
        match val {
            0x0800 => Some(EthProtocol::ETH_P_IP),
            0x0806 => Some(EthProtocol::ETH_P_ARP),
            0x86DD => Some(EthProtocol::ETH_P_IPV6),
            0x8100 => Some(EthProtocol::ETH_P_8021Q),
            _ => None,
        }
    }

    /// Convert to u16
    pub fn to_u16(self) -> u16 {
        self as u16
    }
}

impl IpProtocol {
    /// Convert from u8
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(IpProtocol::IPPROTO_IP),
            1 => Some(IpProtocol::IPPROTO_ICMP),
            6 => Some(IpProtocol::IPPROTO_TCP),
            17 => Some(IpProtocol::IPPROTO_UDP),
            41 => Some(IpProtocol::IPPROTO_IPV6),
            _ => None,
        }
    }

    /// Convert to u8
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}


/// SkBuff owns its backing allocation; dropping a buffer that was not
/// explicitly freed releases it. Every error path that used to leak a
/// ~1.5KB skb now frees it automatically (review NET-C2 / M1).
impl Drop for SkBuff {
    fn drop(&mut self) {
        self.release();
    }
}

/// SkBuff global allocator ID
static SKBUFF_ALLOCATOR_ID: AtomicU64 = AtomicU64::new(0);

impl SkBuff {

    /// Free SkBuff
    ///
    /// # Notes
    /// Releases allocated memory. Idempotent: nulling the pointers makes the
    /// Drop impl (and a later double free) a no-op.
    pub fn free(mut self) {
        self.release();
    }







    /// Set MAC header
    ///
    /// # Arguments
    /// - `len`: MAC header length
    pub fn set_mac_header(&mut self, len: u8) {
        self.mac_header = self.data;
        self.mac_len = len;
    }

    /// Set network layer header
    ///
    /// # Notes
    /// Current data pointer position is the network layer header
    pub fn set_network_header(&mut self) {
        self.network_header = self.data;
    }

    /// Set transport layer header
    ///
    /// # Notes
    /// Current data pointer position is the transport layer header
    pub fn set_transport_header(&mut self) {
        self.transport_header = self.data;
    }

    /// Get MAC header
    pub fn get_mac_header(&self) -> *const u8 {
        self.mac_header
    }

    /// Get network layer header
    pub fn get_network_header(&self) -> *const u8 {
        self.network_header
    }

    /// Get transport layer header
    pub fn get_transport_header(&self) -> *const u8 {
        self.transport_header
    }

    /// Get data pointer
    pub fn data(&self) -> *const u8 {
        self.data
    }

    /// Get mutable data pointer
    pub fn data_mut(&mut self) -> *mut u8 {
        self.data
    }

    /// Get data length
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }










}

/// Helper function to allocate SkBuff
///
/// # Arguments
/// - `size`: Data size
///
/// # Returns
/// The allocated SkBuff
pub fn alloc_skb(size: u32) -> Option<SkBuff> {
    SkBuff::alloc(size)
}

/// Helper function to free SkBuff
///
/// # Arguments
/// - `skb`: SkBuff to free
pub fn kfree_skb(skb: SkBuff) {
    skb.free();
}


// ==================== Net-family header-view edges (round-5) ====================

impl SkBuff {
    /// The whole data area as a shared slice (safe composition of
    /// `data_slice` at full length — round-5).
    pub fn data_all(&self) -> &[u8] {
        self.data_slice(self.len)
    }
}


