# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# safety — hot 833 lines (stay) / cold 205 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| safety/bytes.rs | `bytes_of` | 39 | 10 |  |
| safety/bytes.rs | `bytes_of_mut` | 49 | 9 |  |
| safety/bytes.rs | `from_bytes` | 58 | 13 | yes |
| safety/bytes.rs | `read_at` | 71 | 11 | yes |
| safety/bytes.rs | `write_at` | 82 | 17 | yes |
| safety/bytes.rs | `bytes_of_slice` | 99 | 10 |  |
| safety/bytes.rs | `bytes_of_slice_mut` | 109 | 25 |  |
| safety/bytes.rs | `pinned_frame_bytes` | 134 | 31 | yes |
| safety/bytes.rs | `pinned_frame_run_bytes` | 165 | 14 | yes |
| safety/bytes.rs | `pinned_frame_bytes_mut` | 179 | 20 | yes |
| safety/bytes.rs | `store_volatile` | 199 | 5 |  |
| safety/call.rs | `safety_cast` | 98 | 13 |  |
| safety/dma.rs | `new` | 60 | 19 | yes |
| safety/dma.rs | `phys` | 79 | 9 | yes |
| safety/dma.rs | `addr` | 88 | 7 | yes |
| safety/dma.rs | `phys_at` | 95 | 11 | yes |
| safety/dma.rs | `carve_array` | 106 | 21 |  |
| safety/dma.rs | `carve_array_aligned` | 127 | 22 |  |
| safety/dma.rs | `carve_aligned` | 149 | 8 |  |
| safety/dma.rs | `carve` | 157 | 10 |  |
| safety/dma.rs | `frontier_offset` | 167 | 5 | yes |
| safety/dma.rs | `len` | 172 | 8 | yes |
| safety/dma.rs | `read_volatile_at` | 180 | 20 |  |
| safety/dma.rs | `read_volatile_at_inner` | 200 | 8 |  |
| safety/dma.rs | `write_volatile_at` | 208 | 18 |  |
| safety/dma.rs | `write_volatile_at_inner` | 226 | 10 |  |
| safety/dma.rs | `set_pkey_range` | 236 | 20 | yes |
| safety/dma.rs | `clear_pkey_range` | 256 | 9 | yes |
| safety/dma.rs | `pkey_target` | 265 | 7 | yes |
| safety/dma.rs | `drop` | 272 | 22 | yes |
| safety/dma.rs | `as_ptr` | 294 | 5 | yes |
| safety/dma.rs | `phys` | 299 | 11 | yes |
| safety/dma.rs | `read_volatile` | 310 | 10 | yes |
| safety/dma.rs | `write_volatile` | 320 | 13 | yes |
| safety/dma.rs | `deref` | 333 | 8 | yes |
| safety/dma.rs | `deref_mut` | 341 | 6 | yes |
| safety/frame.rs | `new` | 40 | 7 | yes |
| safety/frame.rs | `base` | 47 | 4 | yes |
| safety/frame.rs | `top` | 51 | 7 | yes |
| safety/frame.rs | `read_u64` | 58 | 21 | yes |
| safety/frame.rs | `read_frame` | 79 | 12 | yes |
| safety/mmio.rs | `new` | 39 | 6 | yes |
| safety/mmio.rs | `base` | 45 | 5 | yes |
| safety/mmio.rs | `len` | 50 | 4 | yes |
| safety/mmio.rs | `check` | 54 | 26 | yes |
| safety/mmio.rs | `read_u8` | 80 | 10 | yes |
| safety/mmio.rs | `read_u16` | 90 | 8 | yes |
| safety/mmio.rs | `read_u32` | 98 | 11 | yes |
| safety/mmio.rs | `read_u64` | 109 | 17 | yes |
| safety/mmio.rs | `read_packed_u32` | 126 | 11 | yes |
| safety/mmio.rs | `read_packed_u64` | 137 | 8 | yes |
| safety/mmio.rs | `write_u8` | 145 | 9 | yes |
| safety/mmio.rs | `write_u16` | 154 | 8 | yes |
| safety/mmio.rs | `write_u32` | 162 | 8 | yes |
| safety/mmio.rs | `write_u64` | 170 | 11 | yes |
| safety/mmio.rs | `ptr_at` | 181 | 5 |  |
| safety/ptr.rs | `new` | 57 | 21 | yes |
| safety/ptr.rs | `as_ptr` | 78 | 6 | yes |
| safety/ptr.rs | `phys` | 84 | 9 | yes |
| safety/ptr.rs | `read_volatile` | 93 | 11 | yes |
| safety/ptr.rs | `write_volatile` | 104 | 13 | yes |
| safety/ptr.rs | `into_raw` | 117 | 14 | yes |
| safety/ptr.rs | `from_raw` | 131 | 8 | yes |
| safety/ptr.rs | `deref` | 139 | 8 | yes |
| safety/ptr.rs | `deref_mut` | 147 | 8 | yes |
| safety/ptr.rs | `drop` | 155 | 49 | yes |
| safety/ptr.rs | `from_static` | 204 | 16 | yes |
| safety/ptr.rs | `from_parts` | 220 | 5 | yes |
| safety/ptr.rs | `get` | 225 | 9 | yes |
| safety/ptr.rs | `map` | 234 | 9 | yes |
| safety/ptr.rs | `deref` | 243 | 6 | yes |
| safety/ptr.rs | `clone` | 249 | 6 | yes |
| safety/ring.rs | `new` | 59 | 21 | yes |
| safety/ring.rs | `put` | 80 | 19 | yes |
| safety/ring.rs | `get` | 99 | 30 | yes |
| safety/ring.rs | `depth_hint` | 129 | 7 | yes |
| safety/slot.rs | `new` | 53 | 9 | yes |
| safety/slot.rs | `publish` | 62 | 9 | yes |
| safety/slot.rs | `publish_with_addr` | 71 | 17 | yes |
| safety/slot.rs | `get` | 88 | 16 | yes |
| safety/slot.rs | `get_mut_locked` | 104 | 11 |  |
| safety/slot.rs | `is_set` | 115 | 6 | yes |
| safety/slot.rs | `default` | 121 | 10 | yes |
| safety/slot.rs | `publish_once_then_read` | 131 | 10 | yes |
| safety/slot.rs | `struct_value_with_interior_lock` | 141 | 12 | yes |
