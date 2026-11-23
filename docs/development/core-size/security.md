# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# security — hot 104 lines (stay) / cold 246 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| security/cap_lsm.rs | `name` | 19 | 4 | yes |
| security/cap_lsm.rs | `order` | 23 | 4 | yes |
| security/cap_lsm.rs | `call` | 27 | 46 |  |
| security/capability.rs | `new` | 120 | 7 | yes |
| security/capability.rs | `has` | 127 | 9 |  |
| security/capability.rs | `set` | 136 | 8 | yes |
| security/capability.rs | `clear` | 144 | 8 | yes |
| security/capability.rs | `intersect` | 152 | 6 |  |
| security/capability.rs | `union` | 158 | 6 |  |
| security/capability.rs | `xor` | 164 | 6 |  |
| security/capability.rs | `complement` | 170 | 6 |  |
| security/capability.rs | `is_subset_of` | 176 | 6 |  |
| security/capability.rs | `is_empty` | 182 | 6 | yes |
| security/capability.rs | `bits` | 188 | 6 | yes |
| security/capability.rs | `lo` | 194 | 6 |  |
| security/capability.rs | `hi` | 200 | 6 |  |
| security/capability.rs | `from_halves` | 206 | 4 |  |
| security/lsm.rs | `new` | 61 | 16 | yes |
| security/lsm.rs | `default` | 77 | 8 | yes |
| security/lsm.rs | `name` | 85 | 4 | yes |
| security/lsm.rs | `order` | 89 | 8 | yes |
| security/lsm.rs | `call` | 97 | 18 |  |
| security/lsm.rs | `register_lsm` | 115 | 28 |  |
| security/lsm.rs | `chain_is_capability_only` | 143 | 10 |  |
| security/lsm.rs | `security_hook_call` | 153 | 17 |  |
| security/lsm.rs | `security_capable` | 170 | 10 |  |
| security/lsm.rs | `security_signal_send` | 180 | 13 |  |
| security/lsm.rs | `lsm_count` | 193 | 3 |  |
| security/mod.rs | `capable` | 38 | 15 | yes |
| security/mod.rs | `has_capability` | 53 | 10 | yes |
| security/mod.rs | `can_send_signal` | 63 | 28 |  |
| security/mod.rs | `security_init` | 91 | 18 |  |
