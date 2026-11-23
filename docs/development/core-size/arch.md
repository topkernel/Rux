# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# arch — hot 215 lines (stay) / cold 133 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| arch/mod.rs | `guard_on` | 83 | 4 | yes |
| arch/mod.rs | `zone_open` | 87 | 4 | yes |
| arch/mod.rs | `zone_close` | 91 | 1 | yes |
| arch/mod.rs | `tag_kernel_pages` | 92 | 3 | yes |
| arch/mod.rs | `untag_kernel_pages` | 95 | 3 | yes |
| arch/mod.rs | `guard_arm` | 98 | 3 |  |
| arch/mod.rs | `switch_rearm` | 101 | 2 | yes |
| arch/uaccess_safe.rs | `copy_out` | 34 | 14 | yes |
| arch/uaccess_safe.rs | `copy_in` | 48 | 13 |  |
| arch/uaccess_safe.rs | `copy_in_vec_exact` | 61 | 23 |  |
| arch/uaccess_safe.rs | `read_user` | 84 | 10 |  |
| arch/uaccess_safe.rs | `write_user` | 94 | 10 |  |
| arch/uaccess_safe.rs | `clear_user` | 104 | 10 | yes |
| arch/uaccess_safe.rs | `strnlen_user` | 114 | 10 | yes |
| arch/uaccess_safe.rs | `futex_cmpxchg_u32` | 124 | 16 |  |
| arch/uaccess_safe.rs | `copy_in_t` | 140 | 21 | yes |
| arch/uaccess_safe.rs | `copy_in_t_slice` | 161 | 15 |  |
| arch/uaccess_safe.rs | `copy_out_t` | 176 | 32 |  |
| arch/uaccess_safe.rs | `new` | 208 | 9 | yes |
| arch/uaccess_safe.rs | `addr` | 217 | 8 | yes |
| arch/uaccess_safe.rs | `is_valid` | 225 | 6 | yes |
| arch/uaccess_safe.rs | `read` | 231 | 8 | yes |
| arch/uaccess_safe.rs | `write` | 239 | 8 | yes |
| arch/uaccess_safe.rs | `add` | 247 | 22 | yes |
| arch/uaccess_safe.rs | `new` | 269 | 11 | yes |
| arch/uaccess_safe.rs | `from_parts` | 280 | 6 | yes |
| arch/uaccess_safe.rs | `addr` | 286 | 6 | yes |
| arch/uaccess_safe.rs | `len` | 292 | 5 | yes |
| arch/uaccess_safe.rs | `is_empty` | 297 | 7 | yes |
| arch/uaccess_safe.rs | `is_valid` | 304 | 8 | yes |
| arch/uaccess_safe.rs | `copy_in` | 312 | 11 |  |
| arch/uaccess_safe.rs | `copy_out` | 323 | 8 | yes |
| arch/uaccess_safe.rs | `clear` | 331 | 9 | yes |
| arch/uaccess_safe.rs | `read_at` | 340 | 12 | yes |
| arch/uaccess_safe.rs | `write_at` | 352 | 10 | yes |
