# Test Encapsulation & Visibility

## Problem

Rux unit tests reside in a dedicated `kernel/src/tests/` directory, structured as a submodule within the crate. This means test code lives in the **same crate** as the code under test, and Rust visibility rules apply:

- `pub(crate)` — visible anywhere within the crate, including test modules
- `pub` — fully public, visible to external crates
- No modifier — visible only within the current module and its children, **inaccessible from test modules**

Because tests need to call internal functions and construct internal types directly, some APIs that should remain private are forced to be promoted to `pub(crate)` or `pub`.

## Current Workarounds

### 1. `pub(crate)` — The Most Common Compromise

For functions that need testing but should not be exposed to external crates, `pub(crate)` is used:

```rust
// kernel/src/fs/ext4/allocator.rs
/// NOTE: Visibility is `pub(crate)` for unit testing (see tests/ext4_allocator.rs).
pub(crate) fn find_free_bit(bitmap: &[u8], start: u64, max_bits: u64) -> Option<u64> {
```

`pub(crate)` is the best current option: it limits visibility to within the crate and does not pollute the public API.

### 2. Indirect Testing via Syscall Entry Points

For modules like filesystem, process management, and signals, tests verify internal logic indirectly through the syscall layer:

```rust
// Instead of calling the internal do_wait_nonblock() directly,
// test through the sys_wait4() syscall entry point
let ret = sys_wait4([0, 0, 1, 0, 0, 0]);
test_assert!((ret as i32) == -errno::ECHILD, "...");
```

This approach naturally preserves encapsulation, but can only cover syscall-level paths — it cannot directly test internal helper functions.

### 3. Public Struct Layout Verification

Using `core::mem::size_of` and `#[repr(C)]` to verify struct layouts without accessing internal fields:

```rust
test_assert_eq!(core::mem::size_of::<VmaFlags>(), 4, "VmaFlags size");
```

## Known Limitations

| Scenario | Issue |
|----------|-------|
| Internal helper functions | Must be promoted to `pub(crate)` to be callable from tests |
| Internal state verification | Tests cannot read private fields (e.g., signal mask, page allocator internals) |
| `access_ok` rejection | Kernel test code runs in kernel space; pointers passed to syscalls are rejected with -EFAULT by `access_ok` |
| Inconsistent error code format | Some syscalls return `e as u32 as u64`, others return `(-errno) as u64`; tests must adapt per-syscall |

## Future Improvements

### 1. Integration Tests (Separate Crate)

Rust natively supports integration tests in a `tests/` directory, where each file compiles as a separate crate. However, Rux is a `no_std` kernel without a standard runtime, so the integration test framework is unavailable. If a custom test harness is implemented in the future, this would enable:

- Test code as a separate crate with access only to `pub` APIs
- Forcing tests through public interfaces without relying on internal implementation details

### 2. Test-only Feature Gate

Using Cargo features to conditionally expose APIs during testing:

```rust
#[cfg(test)]
pub(crate) fn internal_helper(&self) -> bool { ... }

#[cfg(not(test))]
fn internal_helper(&self) -> bool { ... }
```

Drawback: every function that needs testing requires duplicated signatures, leading to high maintenance cost.

### 3. Mock / Stub / Dependency Injection

For hardware-dependent modules (drivers, interrupts, timers), abstract dependencies through traits and inject mock implementations during testing:

```rust
// Define trait
trait BlockDevice {
    fn read_block(&self, block_no: u64, buf: &mut [u8]) -> Result<(), Error>;
}

// Production code uses real implementation
// Test code injects mock
struct MockBlockDevice;
impl BlockDevice for MockBlockDevice {
    fn read_block(&self, _: u64, buf: &mut [u8]) -> Result<(), Error> {
        buf.fill(0); // Return zeroed data
        Ok(())
    }
}
```

This approach fully decouples tests from hardware, but Rux's driver layer currently lacks trait abstraction, making this a large refactor.

### 4. Inline Test Modules (`#[cfg(test)] mod tests`)

The idiomatic Rust approach places tests inside the module under test:

```rust
// kernel/src/fs/ext4/allocator.rs
pub fn allocate_block() -> u64 { ... }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_free_bit() {
        // Can directly access private functions
        assert_eq!(find_free_bit(&[0x00], 0, 8), Some(0));
    }
}
```

Advantage: tests can access all private items within the module without promoting visibility. Disadvantage: Rux runs on QEMU and `cargo test` is unavailable, so these inline tests would need to be integrated into the custom test harness. This is a viable direction for future optimization.

### 5. Userspace Test Programs

Place userspace test programs in rootfs that exercise kernel behavior through syscall interfaces:

```c
// userspace/tests/signal_test.c
#include <signal.h>
#include <assert.h>
int main() {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    int ret = sigprocmask(SIG_BLOCK, &mask, NULL);
    assert(ret == 0);
    // ...
}
```

Advantage: fully tests through public syscall interfaces with zero encapsulation breakage. Disadvantage: can only test the syscall layer, not internal kernel logic. Best suited for end-to-end acceptance testing.

## Summary

| Approach | Encapsulation | Coverage | Effort |
|----------|--------------|----------|--------|
| `pub(crate)` (current) | Medium | All internal | Low |
| Syscall entry tests (current) | High | Syscall layer | Low |
| `#[cfg(test)]` inline tests | High | All internal | Medium |
| Userspace test programs | High | Syscall layer | Medium |
| Trait + Mock | High | Injectable modules | High |
| Integration tests (separate crate) | Highest | pub API only | High |

The current combination of `pub(crate)` and syscall entry tests is a pragmatic choice that covers most of the kernel codebase. Going forward, inline `#[cfg(test)]` modules and userspace test programs can be gradually introduced to reduce dependency on `pub(crate)` visibility promotion.
