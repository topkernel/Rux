# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# safety — hot 1123 lines (stay) / cold 0 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| safety/bytes.rs | `bytes_of` | 39 | 11 | yes |
| safety/bytes.rs | `bytes_of_mut` | 49 | 10 | yes |
| safety/bytes.rs | `from_bytes` | 58 | 14 | yes |
| safety/bytes.rs | `read_at` | 71 | 12 | yes |
| safety/bytes.rs | `write_at` | 82 | 18 | yes |
| safety/bytes.rs | `bytes_of_slice` | 99 | 11 | yes |
| safety/bytes.rs | `bytes_of_slice_mut` | 109 | 26 | yes |
| safety/bytes.rs | `pinned_frame_bytes` | 134 | 32 | yes |
| safety/bytes.rs | `pinned_frame_run_bytes` | 165 | 15 | yes |
| safety/bytes.rs | `pinned_frame_bytes_mut` | 179 | 21 | yes |
| safety/bytes.rs | `store_volatile` | 199 | 6 | yes |
| safety/call.rs | `safety_cast` | 98 | 14 | yes |
| safety/dma.rs | `new` | 60 | 20 | yes |
| safety/dma.rs | `phys` | 79 | 10 | yes |
| safety/dma.rs | `addr` | 88 | 8 | yes |
| safety/dma.rs | `phys_at` | 95 | 12 | yes |
| safety/dma.rs | `carve_array` | 106 | 22 | yes |
| safety/dma.rs | `carve_array_aligned` | 127 | 23 | yes |
| safety/dma.rs | `carve_aligned` | 149 | 9 | yes |
| safety/dma.rs | `carve` | 157 | 11 | yes |
| safety/dma.rs | `frontier_offset` | 167 | 6 | yes |
| safety/dma.rs | `len` | 172 | 9 | yes |
| safety/dma.rs | `read_volatile_at` | 180 | 21 | yes |
| safety/dma.rs | `read_volatile_at_inner` | 200 | 9 | yes |
| safety/dma.rs | `write_volatile_at` | 208 | 19 | yes |
| safety/dma.rs | `write_volatile_at_inner` | 226 | 11 | yes |
| safety/dma.rs | `set_pkey_range` | 236 | 21 | yes |
| safety/dma.rs | `clear_pkey_range` | 256 | 10 | yes |
| safety/dma.rs | `pkey_target` | 265 | 8 | yes |
| safety/dma.rs | `drop` | 272 | 23 | yes |
| safety/dma.rs | `as_ptr` | 294 | 6 | yes |
| safety/dma.rs | `phys` | 299 | 12 | yes |
| safety/dma.rs | `read_volatile` | 310 | 11 | yes |
| safety/dma.rs | `write_volatile` | 320 | 14 | yes |
| safety/dma.rs | `deref` | 333 | 9 | yes |
| safety/dma.rs | `deref_mut` | 341 | 7 | yes |
| safety/frame.rs | `new` | 40 | 8 | yes |
| safety/frame.rs | `base` | 47 | 5 | yes |
| safety/frame.rs | `top` | 51 | 8 | yes |
| safety/frame.rs | `read_u64` | 58 | 22 | yes |
| safety/frame.rs | `read_frame` | 79 | 13 | yes |
| safety/mmio.rs | `new` | 39 | 7 | yes |
| safety/mmio.rs | `base` | 45 | 6 | yes |
| safety/mmio.rs | `len` | 50 | 5 | yes |
| safety/mmio.rs | `check` | 54 | 27 | yes |
| safety/mmio.rs | `read_u8` | 80 | 11 | yes |
| safety/mmio.rs | `read_u16` | 90 | 9 | yes |
| safety/mmio.rs | `read_u32` | 98 | 12 | yes |
| safety/mmio.rs | `read_u64` | 109 | 18 | yes |
| safety/mmio.rs | `read_packed_u32` | 126 | 12 | yes |
| safety/mmio.rs | `read_packed_u64` | 137 | 9 | yes |
| safety/mmio.rs | `write_u8` | 145 | 10 | yes |
| safety/mmio.rs | `write_u16` | 154 | 9 | yes |
| safety/mmio.rs | `write_u32` | 162 | 9 | yes |
| safety/mmio.rs | `write_u64` | 170 | 12 | yes |
| safety/mmio.rs | `ptr_at` | 181 | 6 | yes |
| safety/ptr.rs | `new` | 57 | 22 | yes |
| safety/ptr.rs | `as_ptr` | 78 | 7 | yes |
| safety/ptr.rs | `phys` | 84 | 10 | yes |
| safety/ptr.rs | `read_volatile` | 93 | 12 | yes |
| safety/ptr.rs | `write_volatile` | 104 | 14 | yes |
| safety/ptr.rs | `into_raw` | 117 | 15 | yes |
| safety/ptr.rs | `from_raw` | 131 | 9 | yes |
| safety/ptr.rs | `deref` | 139 | 9 | yes |
| safety/ptr.rs | `deref_mut` | 147 | 9 | yes |
| safety/ptr.rs | `drop` | 155 | 50 | yes |
| safety/ptr.rs | `from_static` | 204 | 17 | yes |
| safety/ptr.rs | `from_parts` | 220 | 6 | yes |
| safety/ptr.rs | `get` | 225 | 10 | yes |
| safety/ptr.rs | `map` | 234 | 10 | yes |
| safety/ptr.rs | `deref` | 243 | 7 | yes |
| safety/ptr.rs | `clone` | 249 | 7 | yes |
| safety/ring.rs | `new` | 59 | 22 | yes |
| safety/ring.rs | `put` | 80 | 20 | yes |
| safety/ring.rs | `get` | 99 | 31 | yes |
| safety/ring.rs | `depth_hint` | 129 | 8 | yes |
| safety/slot.rs | `new` | 53 | 10 | yes |
| safety/slot.rs | `publish` | 62 | 10 | yes |
| safety/slot.rs | `publish_with_addr` | 71 | 18 | yes |
| safety/slot.rs | `get` | 88 | 17 | yes |
| safety/slot.rs | `get_mut_locked` | 104 | 12 | yes |
| safety/slot.rs | `is_set` | 115 | 7 | yes |
| safety/slot.rs | `default` | 121 | 11 | yes |
| safety/slot.rs | `publish_once_then_read` | 131 | 11 | yes |
| safety/slot.rs | `struct_value_with_interior_lock` | 141 | 13 | yes |
