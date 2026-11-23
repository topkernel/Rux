# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch — hot 46 lines (stay) / cold 337 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/mod.rs | `guard_on` | 83 | 5 |  |
| arch/mod.rs | `zone_open` | 87 | 5 |  |
| arch/mod.rs | `zone_close` | 91 | 2 |  |
| arch/mod.rs | `tag_kernel_pages` | 92 | 4 |  |
| arch/mod.rs | `untag_kernel_pages` | 95 | 4 |  |
| arch/mod.rs | `guard_arm` | 98 | 4 |  |
| arch/mod.rs | `switch_rearm` | 101 | 3 |  |
| arch/uaccess_safe.rs | `copy_out` | 34 | 15 | yes |
| arch/uaccess_safe.rs | `copy_in` | 48 | 14 |  |
| arch/uaccess_safe.rs | `copy_in_vec_exact` | 61 | 24 |  |
| arch/uaccess_safe.rs | `read_user` | 84 | 11 |  |
| arch/uaccess_safe.rs | `write_user` | 94 | 11 |  |
| arch/uaccess_safe.rs | `clear_user` | 104 | 11 |  |
| arch/uaccess_safe.rs | `strnlen_user` | 114 | 11 |  |
| arch/uaccess_safe.rs | `futex_cmpxchg_u32` | 124 | 17 |  |
| arch/uaccess_safe.rs | `copy_in_t` | 140 | 22 | yes |
| arch/uaccess_safe.rs | `copy_in_t_slice` | 161 | 16 |  |
| arch/uaccess_safe.rs | `copy_out_t` | 176 | 33 |  |
| arch/uaccess_safe.rs | `new` | 208 | 10 |  |
| arch/uaccess_safe.rs | `addr` | 217 | 9 |  |
| arch/uaccess_safe.rs | `is_valid` | 225 | 7 |  |
| arch/uaccess_safe.rs | `read` | 231 | 9 |  |
| arch/uaccess_safe.rs | `write` | 239 | 9 |  |
| arch/uaccess_safe.rs | `add` | 247 | 23 |  |
| arch/uaccess_safe.rs | `new` | 269 | 12 |  |
| arch/uaccess_safe.rs | `from_parts` | 280 | 7 |  |
| arch/uaccess_safe.rs | `addr` | 286 | 7 |  |
| arch/uaccess_safe.rs | `len` | 292 | 6 |  |
| arch/uaccess_safe.rs | `is_empty` | 297 | 8 |  |
| arch/uaccess_safe.rs | `is_valid` | 304 | 9 |  |
| arch/uaccess_safe.rs | `copy_in` | 312 | 12 |  |
| arch/uaccess_safe.rs | `copy_out` | 323 | 9 | yes |
| arch/uaccess_safe.rs | `clear` | 331 | 10 |  |
| arch/uaccess_safe.rs | `read_at` | 340 | 13 |  |
| arch/uaccess_safe.rs | `write_at` | 352 | 11 |  |
