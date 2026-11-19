# Collection Types Migration Record

## Historical Background

**Last Updated**: 2025-02-09
**Status**: Migrated to standard `alloc` crate

---

## Early Issues (Resolved)

In early Rust versions, due to symbol visibility issues with `__rust_no_alloc_shim_is_unstable_v2`, we implemented custom collection types (SimpleArc, SimpleVec, SimpleBox, SimpleString) to bypass the `alloc` crate.

**The problem at the time**:
- Rust compiler used unstable features
- `alloc` crate directly called hidden mangled symbols
- Could not link in statically linked `no_std` binaries

**Attempted solutions** (all failed):
1. `--export-dynamic-symbol` linker option
2. Linker script PROVIDE to create aliases
3. Manually implementing `__rust_alloc` functions
4. Assembly code to create jump wrappers
5. Overriding `__rust_no_alloc_shim_is_unstable_v2` symbol

---

## Final Solution: Use Standard alloc crate

### Verification Results

In **Rust 1.95.0-nightly (2026-02-04)**, the `__rust_no_alloc_shim_is_unstable_v2` issue has been resolved!

**Test Results**:
```
test: Testing standard alloc crate types...
test: 1. Testing alloc::vec::Vec...
test:    SUCCESS - Vec works correctly
test: 2. Testing alloc::boxed::Box...
test:    SUCCESS - Box works correctly
test: 3. Testing alloc::sync::Arc...
test:    SUCCESS - Arc works correctly
test: 4. Testing alloc::string::String...
test:    SUCCESS - String works correctly
test: All standard alloc crate types work correctly!
test: This means the __rust_no_alloc_shim_is_unstable_v2 issue is resolved.
```

---

## Migration Content

### Removed Custom Types

| Type | Replaced With | File |
|------|---------------|------|
| `SimpleArc<T>` | `alloc::sync::Arc<T>` | Deleted collection.rs |
| `SimpleVec<T>` | `alloc::vec::Vec<T>` | Deleted collection.rs |
| `SimpleBox<T>` | `alloc::boxed::Box<T>` | Deleted collection.rs |
| `SimpleString` | `alloc::string::String` | Deleted collection.rs |

### Modified Files

**Core Files**:
- `kernel/src/collection.rs` - **Deleted**
- `kernel/src/main.rs` - Removed `mod collection;`
- `kernel/src/fs/vfs.rs` - SimpleArc to Arc
- `kernel/src/fs/file.rs` - SimpleArc to Arc
- `kernel/src/fs/rootfs.rs` - SimpleArc to Arc
- `kernel/src/fs/dentry.rs` - SimpleArc to Arc
- `kernel/src/fs/inode.rs` - SimpleArc to Arc
- `kernel/src/fs/pipe.rs` - SimpleArc to Arc
- `kernel/src/fs/mount.rs` - SimpleArc to Arc
- `kernel/src/fs/superblock.rs` - SimpleArc to Arc
- `kernel/src/sched/sched.rs` - SimpleArc to Arc

**Test Files**:
- `kernel/src/tests/arc_alloc.rs` - **Deleted**
- `kernel/src/tests/dcache.rs` - SimpleArc to Arc
- `kernel/src/tests/fdtable.rs` - SimpleArc to Arc
- `kernel/src/tests/icache.rs` - SimpleArc to Arc
- `kernel/src/tests/standard_alloc.rs` - New standard alloc tests

---

## Code Change Statistics

- **Deleted Files**: 2 (collection.rs, tests/arc_alloc.rs)
- **Modified Files**: 15
- **Deleted Lines**: ~400 lines of custom collection implementations
- **Added Lines**: ~50 lines of standard alloc tests

---

## Key Change Examples

### SimpleArc to Arc Migration

**Before** (SimpleArc):
```rust
use crate::collection::SimpleArc;

// Create Arc, returns Option
let arc = match SimpleArc::new(value) {
    Some(a) => a,
    None => return Err(OutOfMemory),
};

// Access data
let data = arc.as_ref();
```

**Now** (Standard Arc):
```rust
use alloc::sync::Arc;

// Create Arc, panics on failure
let arc = Arc::new(value);

// Access data (automatic Deref)
let data = &*arc;
// or
let data = &arc;
```

### Arc Method Changes

| SimpleArc Method | Standard Arc Equivalent | Notes |
|------------------|-------------------------|-------|
| `SimpleArc::new(v)` | `Arc::new(v)` | Arc doesn't return Option |
| `arc.as_ref()` | `&*arc` or `&arc` | Arc auto Deref |
| `arc.as_ptr()` | `Arc::as_ptr(&arc)` | Need to pass reference explicitly |
| `SimpleArc::clone(v)` | `Arc::clone(&v)` | Standard interface |

---

## Advantages

1. **Standard Compatible** - Uses Rust standard library types, fully compatible
2. **Code Simplification** - No need to maintain custom collection implementations
3. **Performance Optimized** - Standard library is well optimized
4. **Community Support** - Standard library has better documentation and community support
5. **Future Compatible** - Automatically benefits from Rust version updates

---

## Known Limitations

### Interior Mutability

Standard `Arc<T>` only provides immutable references `&T`. If you need to modify `T`'s content, you must use interior mutability patterns:

```rust
use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::sync::Arc;

struct Data {
    value: AtomicUsize,
}

let data = Arc::new(Data { value: AtomicUsize::new(0) });

// Modify value through AtomicUsize
data.value.store(42, Ordering::SeqCst);
```

**Note**: File operations like `close()` require `&mut self`, which needs special handling in Arc environments (using unsafe conversion).

---

## Related Resources

- [Rust Tracking Issue #123015](https://github.com/rust-lang/rust/issues/123015) - __rust_no_alloc_shim_is_unstable
- [PR #86844](https://github.com/rust-lang/rust/pull/86844) - Support #[global_allocator] without allocator shim
- [Phil Opp's Allocator Design](https://os.phil-opp.com/allocator-designs/)

---

## Conclusion

Rux OS now fully uses the standard Rust `alloc` crate and no longer needs custom collection types. This is thanks to improvements in the Rust compiler that resolved early symbol visibility issues.

**Recommended Practices**:
- Use `alloc::sync::Arc` for shared references
- Use `alloc::boxed::Box` for heap allocation
- Use `alloc::vec::Vec` for dynamic arrays
- Use `alloc::string::String` for strings
- No longer use any `Simple*` custom types
