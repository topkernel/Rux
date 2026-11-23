# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# module — hot 0 lines (stay) / cold 408 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| module/mod.rs | `rd16` | 79 | 5 |  |
| module/mod.rs | `rd32` | 84 | 5 |  |
| module/mod.rs | `rd64` | 89 | 13 |  |
| module/mod.rs | `parse_ehdr` | 102 | 36 |  |
| module/mod.rs | `shdr` | 138 | 19 |  |
| module/mod.rs | `module_name_from_blob` | 157 | 74 |  |
| module/mod.rs | `sysfs_publish` | 231 | 25 |  |
| module/mod.rs | `sysfs_unpublish` | 256 | 17 |  |
| module/mod.rs | `load_module_blob` | 273 | 33 |  |
| module/mod.rs | `delete_module_by_name` | 306 | 25 |  |
| module/mod.rs | `generate_proc_modules` | 331 | 17 |  |
| module/mod.rs | `check_cap_sys_module` | 348 | 5 |  |
| module/mod.rs | `sys_init_module_impl` | 353 | 34 |  |
| module/mod.rs | `sys_finit_module_impl` | 387 | 48 |  |
| module/mod.rs | `sys_delete_module_impl` | 435 | 52 |  |
