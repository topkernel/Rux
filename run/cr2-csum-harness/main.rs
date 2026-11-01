// Host-side oracle for the ext4 metadata_csum write-path formulas.
// Replicates, byte for byte, what the Rux kernel write path does to a
// 4K-block / 256-byte-inode metadata_csum ext4 image for ONE metadata
// class per invocation, then leaves the verdict to e2fsck -fn.
//
// Usage: harness <img> <class>
//   classes: sb | gd | inode | ibitmap | bbitmap | dblock-split | dblock-new
//
// Layout constants mirror the kernel's (superblock.rs / allocator.rs).
use std::convert::TryInto;
use std::fs;
use std::process::exit;

mod csum;

const BLOCK: usize = 4096;
const INODE_SIZE: usize = 256;

fn rd_block(img: &[u8], b: u64) -> Vec<u8> {
    let off = b as usize * BLOCK;
    img[off..off + BLOCK].to_vec()
}

fn wr_block(img: &mut Vec<u8>, b: u64, data: &[u8]) {
    let off = b as usize * BLOCK;
    img[off..off + BLOCK].copy_from_slice(data);
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn crc32c_full(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = crc_table(b, c);
    }
    c
}

// Raw crc32c step identical to csum::crc32c but as a free function so we
// can run it over arbitrary spans without copying.
fn crc_table(b: u8, c: u32) -> u32 {
    // bit-reflect Castagnoli step
    let mut c = c;
    let idx = ((c ^ b as u32) & 0xFF) as usize;
    // reuse the module table via a tiny public shim
    c = CRC_T[idx] ^ (c >> 8);
    c
}

static CRC_T: [u32; 256] = make_crc_t();

const fn make_crc_t() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut b = 0;
        while b < 8 {
            c = if c & 1 != 0 { 0x82F6_3B78 ^ (c >> 1) } else { c >> 1 };
            b += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

struct Fs {
    sb_seed: u32,
    inodes_per_group: usize,
    blocks_per_group: usize,
    desc_size: usize,
    inode_table_block: u64,
    inode_bitmap_block: u64,
    block_bitmap_block: u64,
    root_dir_block: u64,
    root_ino: u32,
    free_inodes: u32,
    group_free_inodes: u16,
}

fn load_fs(img: &[u8]) -> Fs {
    let sb = &img[1024..2048];
    assert!(csum::superblock_csum_ok(sb), "baseline sb csum bad");
    let uuid: [u8; 16] = sb[104..120].try_into().unwrap();
    let disk_seed = u32::from_le_bytes(sb[0x262..0x266].try_into().unwrap()); // s_checksum_seed 0x262
    let seed = csum::csum_seed(&uuid, disk_seed);
    let inodes_per_group = u32::from_le_bytes(sb[40..44].try_into().unwrap()) as usize;
    let blocks_per_group = u32::from_le_bytes(sb[32..36].try_into().unwrap()) as usize;
    let desc_size = le16(sb, 254) as usize; // s_desc_size @ 0xFE
    let bpg = if blocks_per_group % 8 == 0 { blocks_per_group } else { blocks_per_group + 7 & !7 };
    let _ = bpg;
    // first group descriptor: block 1, offset 0
    let gd = &img[BLOCK..BLOCK + desc_size];
    assert!(csum::group_desc_csum_ok(seed, 0, gd), "baseline gd csum bad");
    let inode_table_block = le16(gd, 8) as u64 | (le16(gd, 0x28) as u64) << 16; // hi at 0x28
    let inode_bitmap_block = le16(gd, 4) as u64 | (le16(gd, 0x26) as u64) << 16;
    let block_bitmap_block = le16(gd, 0) as u64 | (le16(gd, 0x24) as u64) << 16;
    // root inode dir block: inode 2, first extent
    let ino2_off = inode_table_block as usize * BLOCK + 1 * INODE_SIZE; // ino 2 => idx 1
    let raw2 = &img[ino2_off..ino2_off + INODE_SIZE];
    assert!(csum::inode_csum_ok(seed, 2, raw2), "baseline inode-2 csum bad");
    let root_dir_block = u32::from_le_bytes(raw2[60..64].try_into().unwrap()) as u64; // i_block[0].ee_start_lo
    Fs {
        sb_seed: seed,
        inodes_per_group,
        blocks_per_group,
        desc_size,
        inode_table_block,
        inode_bitmap_block,
        block_bitmap_block,
        root_dir_block,
        root_ino: 2,
        free_inodes: u32::from_le_bytes(sb[16..20].try_into().unwrap()),
        group_free_inodes: le16(gd, 14),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <img> <class>", args[0]);
        exit(2);
    }
    let mut img = fs::read(&args[1]).expect("read img");
    let class = args[2].as_str();
    let f = load_fs(&img);
    println!(
        "seed={:08x} ipg={} bpg={} desc_size={} itable={} rootdirblk={} free_inodes={}",
        f.sb_seed, f.inodes_per_group, f.blocks_per_group, f.desc_size,
        f.inode_table_block, f.root_dir_block, f.free_inodes
    );

    match class {
        // update_superblock_free_inodes: s_free_inodes_count -= 1 + re-seal
        "sb" => {
            let mut b0 = rd_block(&img, 0);
            let v = u32::from_le_bytes(b0[1024 + 16..1024 + 20].try_into().unwrap());
            b0[1024 + 16..1024 + 20].copy_from_slice(&(v - 1).to_le_bytes());
            csum::superblock_csum_set(&mut b0[1024..2048]);
            wr_block(&mut img, 0, &b0);
        }
        // update_group_descriptor_inodes: bg_free_inodes_count_lo -= 1 + re-seal
        "gd" => {
            let mut b1 = rd_block(&img, 1);
            let v = le16(&b1, 14);
            b1[14..16].copy_from_slice(&(v - 1).to_le_bytes());
            csum::group_desc_csum_set(f.sb_seed, 0, &mut b1[..f.desc_size]);
            wr_block(&mut img, 1, &b1);
        }
        // inode write: touch i_mtime of root (inode 2), re-seal via inode_csum_set
        "inode" => {
            let itb = rd_block(&img, f.inode_table_block);
            let idx = ((f.root_ino - 1) as usize) % f.inodes_per_group;
            let off = idx * INODE_SIZE;
            let mut itb = itb;
            let mtime = u32::from_le_bytes(itb[off + 0x70..off + 0x74].try_into().unwrap());
            itb[off + 0x70..off + 0x74].copy_from_slice(&(mtime.wrapping_add(1)).to_le_bytes());
            csum::inode_csum_set(f.sb_seed, f.root_ino, &mut itb[off..off + INODE_SIZE]);
            wr_block(&mut img, f.inode_table_block, &itb);
        }
        // mark_inode_used: set the next free bit in the inode bitmap, refresh
        // bg_inode_bitmap_csum_lo/hi (set_bitmap_csum_in_gd) + re-seal gd.
        "ibitmap" => {
            let mut bm = rd_block(&img, f.inode_bitmap_block);
            let size = f.inodes_per_group / 8;
            let mut bit = 0usize;
            'outer: for byte in 0..size {
                for b in 0..8 {
                    if bm[byte] & (1 << b) == 0 {
                        bm[byte] |= 1 << b;
                        bit = byte * 8 + b;
                        break 'outer;
                    }
                }
            }
            println!("ibitmap: set bit {}", bit);
            let c = csum::bitmap_csum(f.sb_seed, &bm, size);
            let mut b1 = rd_block(&img, 1);
            {
                let d = &mut b1[..f.desc_size];
                d[0x1A..0x1C].copy_from_slice(&((c & 0xFFFF) as u16).to_le_bytes());
                if f.desc_size >= 0x3C {
                    d[0x3A..0x3C].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
                }
                csum::group_desc_csum_set(f.sb_seed, 0, d);
            }
            wr_block(&mut img, f.inode_bitmap_block, &bm);
            wr_block(&mut img, 1, &b1);
        }
        // allocator write_block_bitmap: set the next free bit in the block
        // bitmap, refresh bg_block_bitmap_csum_lo/hi + re-seal gd.
        "bbitmap" => {
            let mut bm = rd_block(&img, f.block_bitmap_block);
            let size = f.blocks_per_group / 8;
            let mut bit = 0usize;
            'outer: for byte in 0..size {
                for b in 0..8 {
                    if bm[byte] & (1 << b) == 0 {
                        bm[byte] |= 1 << b;
                        bit = byte * 8 + b;
                        break 'outer;
                    }
                }
            }
            println!("bbitmap: set bit {}", bit);
            let c = csum::bitmap_csum(f.sb_seed, &bm, size);
            let mut b1 = rd_block(&img, 1);
            {
                let d = &mut b1[..f.desc_size];
                d[0x18..0x1A].copy_from_slice(&((c & 0xFFFF) as u16).to_le_bytes());
                if f.desc_size >= 0x38 + 2 {
                    d[0x38..0x3A].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
                }
                csum::group_desc_csum_set(f.sb_seed, 0, d);
            }
            wr_block(&mut img, f.block_bitmap_block, &bm);
            wr_block(&mut img, 1, &b1);
        }
        // add_dir_entry split path: split the LAST dirent's slack in the root
        // dir block (exactly add_entry_to_block), then dir_block_seal.
        "dblock-split" => {
            let mut b = rd_block(&img, f.root_dir_block);
            let bs = BLOCK;
            // walk to last entry
            let mut off = 0usize;
            loop {
                let rec = le16(&b, off + 4) as usize;
                if off + rec >= bs {
                    break;
                }
                off += rec;
            }
            let rec = le16(&b, off + 4) as usize;
            let name_len = b[off + 6] as usize;
            let used = (8 + name_len + 3) & !3;
            assert!(rec > used, "no slack in last entry (off={} rec={} used={})", off, rec, used);
            let req = 12usize; // "newfile1" => 8+8=16; use 8+4 => 12
            // shrink existing
            b[off + 4..off + 6].copy_from_slice(&(used as u16).to_le_bytes());
            let no = off + used;
            let nrec = rec - used;
            let wrec = if nrec - req >= 8 { req } else { nrec };
            b[no..no + 4].copy_from_slice(&11u32.to_le_bytes()); // ino 11
            b[no + 4..no + 6].copy_from_slice(&(wrec as u16).to_le_bytes());
            b[no + 6] = 4; // name_len 4
            b[no + 7] = 1; // type REG
            b[no + 8..no + 12].copy_from_slice(b"nwx1");
            if nrec - wrec >= 8 {
                // free tail entry covering the remainder (kernel add_entry_to_block)
                let to = no + wrec;
                b[to..to + 4].copy_from_slice(&0u32.to_le_bytes());
                b[to + 4..to + 6].copy_from_slice(&((nrec - wrec) as u16).to_le_bytes());
            }
            let seed = csum::dir_csum_seed(f.sb_seed, f.root_ino, 0);
            csum::dir_block_csum_set(seed, &mut b);
            wr_block(&mut img, f.root_dir_block, &b);
        }
        // fresh dir block (create_initial_entry + dir_block_seal): a new
        // block with a single "."-style entry spanning the block.
        "dblock-new" => {
            let mut b = vec![0u8; BLOCK];
            // create_initial_entry: first entry with rec_len = block_size
            b[0..4].copy_from_slice(&f.root_ino.to_le_bytes());
            b[4..6].copy_from_slice(&(BLOCK as u16).to_le_bytes());
            b[6] = 1;
            b[7] = 2; // type DIR
            b[8] = b'.';
            let seed = csum::dir_csum_seed(f.sb_seed, f.root_ino, 0);
            csum::dir_block_csum_set(seed, &mut b);
            // write it over an allocated-free block: use root dir block's
            // block+1 only for formula testing (e2fsck verdict on the BLOCK
            // bytes, not on tree linkage)
            let probe = f.root_dir_block + 1;
            println!("dblock-new: wrote probe dir block at {}", probe);
            wr_block(&mut img, probe, &b);
        }
        // inspect: ground truth — sb csum field width/value and the dir tail
        // layout as written by mke2fs 1.47.2 with metadata_csum.
        "inspect" => {
            let sb = &img[1024..2048];
            let stored32 = u32::from_le_bytes(sb[1020..1024].try_into().unwrap());
            let full = crc32c_full(&sb[..1020]);
            println!(
                "sb: stored32={:08x} fullcrc={:08x} match32={} (lo16={:04x} match_lo16={})",
                stored32,
                full,
                stored32 == full,
                stored32 & 0xFFFF,
                (stored32 & 0xFFFF) == (full & 0xFFFF)
            );
            let b = rd_block(&img, f.root_dir_block);
            // walk the dirent chain
            let mut off = 0usize;
            let mut last = 0usize;
            while off + 8 <= BLOCK {
                let rec = le16(&b, off + 4) as usize;
                if rec == 0 || off + rec > BLOCK {
                    break;
                }
                last = off;
                off += rec;
            }
            println!(
                "dir: chain end={} last_entry@{} rec={} (tail region [{}..4096])",
                off,
                last,
                le16(&b, last + 4),
                off
            );
            let n = BLOCK;
            println!("last16 = {:02x?}", &b[n - 16..n]);
            let tail = &b[n - 12..n];
            println!(
                "tail12 = rz1={:02x?} rec={:04x} namelen={:04x} csum={:02x?}",
                &tail[0..4],
                le16(tail, 4),
                le16(tail, 6),
                &tail[8..12]
            );
            let seed = csum::dir_csum_seed(f.sb_seed, f.root_ino, 0);
            let expect = csum::crc32c(seed, &b[..n - 12]);
            println!(
                "tail12 csum stored={:02x?} computed(span-12)={:08x}",
                &tail[8..12],
                expect
            );
        }
        // sb32: mutate s_free_inodes_count and store the FULL 32-bit crc at
        // [1020..1024) — the reference-kernel/e2fsprogs encoding.
        "sb32" => {
            let mut b0 = rd_block(&img, 0);
            let v = u32::from_le_bytes(b0[1024 + 16..1024 + 20].try_into().unwrap());
            b0[1024 + 16..1024 + 20].copy_from_slice(&(v - 1).to_le_bytes());
            let full = crc32c_full(&b0[1024..2044]);
            b0[1024 + 1020..1024 + 1024].copy_from_slice(&full.to_le_bytes());
            wr_block(&mut img, 0, &b0);
        }
        // dirhunt: find the exact span/seed combination that reproduces the
        // stored 12-byte dir tail checksum on the pristine block.
        "dirhunt" => {
            // dump inode 2's i_block region to locate the extent
            let itb = rd_block(&img, f.inode_table_block);
            let idx = ((f.root_ino - 1) as usize) % f.inodes_per_group;
            let off = idx * INODE_SIZE;
            println!("ino2 i_block[40..100] = {:02x?}", &itb[off + 40..off + 100]);
            let b = rd_block(&img, f.root_dir_block);
            let stored = u32::from_le_bytes(b[4092..4096].try_into().unwrap());
            println!("stored tail csum = {:08x}", stored);
            for ino in 1u32..48 {
                // read i_generation of this inode
                let ioff = ((ino as usize) - 1) * INODE_SIZE;
                let gen = u32::from_le_bytes(itb[ioff + 100..ioff + 104].try_into().unwrap());
                let s1 = csum::dir_csum_seed(f.sb_seed, ino, 0);
                let s2 = csum::crc32c(s1, &gen.to_le_bytes());
                let s3 = csum::crc32c(f.sb_seed, &gen.to_le_bytes());
                for (name, s) in [("ino+gen", s2), ("gen", s3), ("ino", s1)] {
                    for (span, v) in [
                        (4084usize, &b[..4084] as &[u8]),
                        (4088, &b[..4088] as &[u8]),
                        (4092, &b[..4092] as &[u8]),
                    ] {
                        let c = csum::crc32c(s, v);
                        if c == stored {
                            println!("MATCH: ino={} gen={:08x} seed={} span={}", ino, gen, name, span);
                        }
                    }
                    let mut v = b[..4096].to_vec();
                    v[4092..4096].copy_from_slice(&[0u8; 4]);
                    let c = csum::crc32c(s, &v);
                    if c == stored {
                        println!("MATCH: ino={} gen={:08x} seed={} span=4096_csumzero", ino, gen, name);
                    }
                }
            }
            println!("hunt done");
        }
        // reseal: re-seal the pristine root dir block with the FIXED model.
        // On an unmodified block the result must be BIT-IDENTICAL to what
        // mke2fs stored (proof the formula matches the oracle).
        "reseal" => {
            let mut b = rd_block(&img, f.root_dir_block);
            let before = b.clone();
            let seed = csum::dir_csum_seed(f.sb_seed, f.root_ino, 0);
            csum::dir_block_csum_set(seed, &mut b);
            let same = before[..] == b[..];
            println!("reseal bit-identical: {}", same);
            if !same {
                let n = BLOCK;
                println!("before tail {:02x?}", &before[n - 16..n]);
                println!("after  tail {:02x?}", &b[n - 16..n]);
            }
        }
        // dirfix: split the last dirent's slack with the REFERENCE geometry
        // (chain ends at block-12) and the fixed seal; e2fsck must report
        // no checksum complaints (unknown-inode semantic noise is fine).
        "dirfix" => {
            let mut b = rd_block(&img, f.root_dir_block);
            let bs = BLOCK;
            let usable = bs - 12;
            // walk the chain within the dirent area
            let mut off = 0usize;
            let mut last = 0usize;
            while off + 8 <= usable {
                let rec = le16(&b, off + 4) as usize;
                if rec < 8 {
                    break;
                }
                last = off;
                off += rec;
            }
            println!("dirfix: chain end={} last={} rec={}", off, last, le16(&b, last + 4));
            let rec = le16(&b, last + 4) as usize;
            let name_len = b[last + 6] as usize;
            let used = (8 + name_len + 3) & !3;
            assert!(rec > used, "no slack in last entry");
            let req = 12usize;
            b[last + 4..last + 6].copy_from_slice(&(used as u16).to_le_bytes());
            let no = last + used;
            let nrec = rec - used;
            let wrec = if nrec - req >= 8 { req } else { nrec };
            assert!(no + nrec <= usable, "slot crosses usable end");
            b[no..no + 4].copy_from_slice(&11u32.to_le_bytes());
            b[no + 4..no + 6].copy_from_slice(&(wrec as u16).to_le_bytes());
            b[no + 6] = 4;
            b[no + 7] = 1;
            b[no + 8..no + 12].copy_from_slice(b"nwx1");
            if nrec - wrec >= 8 {
                let to = no + wrec;
                b[to..to + 4].copy_from_slice(&0u32.to_le_bytes());
                b[to + 4..to + 6].copy_from_slice(&((nrec - wrec) as u16).to_le_bytes());
            }
            let seed = csum::dir_csum_seed(f.sb_seed, f.root_ino, 0);
            csum::dir_block_csum_set(seed, &mut b);
            wr_block(&mut img, f.root_dir_block, &b);
        }
        _ => {
            eprintln!("unknown class {}", class);
            exit(2);
        }
    }
    fs::write(&args[1], &img).expect("write img");
    println!("class {} applied", class);
}
