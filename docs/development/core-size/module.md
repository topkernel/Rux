# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# module — hot 0 lines (stay) / cold 423 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| module/mod.rs | `rd16` | 79 | 6 |  |
| module/mod.rs | `rd32` | 84 | 6 |  |
| module/mod.rs | `rd64` | 89 | 14 |  |
| module/mod.rs | `parse_ehdr` | 102 | 37 |  |
| module/mod.rs | `shdr` | 138 | 20 |  |
| module/mod.rs | `module_name_from_blob` | 157 | 75 |  |
| module/mod.rs | `sysfs_publish` | 231 | 26 |  |
| module/mod.rs | `sysfs_unpublish` | 256 | 18 |  |
| module/mod.rs | `load_module_blob` | 273 | 34 |  |
| module/mod.rs | `delete_module_by_name` | 306 | 26 |  |
| module/mod.rs | `generate_proc_modules` | 331 | 18 |  |
| module/mod.rs | `check_cap_sys_module` | 348 | 6 |  |
| module/mod.rs | `sys_init_module_impl` | 353 | 35 |  |
| module/mod.rs | `sys_finit_module_impl` | 387 | 49 |  |
| module/mod.rs | `sys_delete_module_impl` | 435 | 53 |  |
