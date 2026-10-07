# Ubuntu GUI Image — From-Scratch Build Guide

How to produce `work/ubuntu-gui.img` (the Ubuntu 22.04 riscv64 rootfs with
the udesk framebuffer session) on **any** dev machine, starting from a clean
checkout. Nothing here depends on hand-carried artifacts from a previous
machine.

```bash
make ubuntu-image    # fetches the base rootfs if absent, builds the image
make ubuntu-run      # boots it in an SDL window (login: root / rux)
# CI-style verification (23 pixel/serial checks per boot):
python3 test/ubuntu-gui/verify.py --runs 2 --smp 4
```

Verified on a fresh machine 2026-10-07: `verify.py --runs 1` PASS (23/23).

## What happens under the hood

`test/ubuntu-gui/build-img.sh`:

1. **Base rootfs source** (in priority order):
   - `$SRC` env override, else
   - `/home/william/rux-gate/ubuntu/rootfs` (legacy shared rootfs), else
   - **auto-fetch**: `scripts/fetch-ubuntu-rootfs.sh` downloads the official
     `ubuntu-base-22.04.3-base-riscv64.tar.gz` (~26 MB) from TUNA (fallback:
     cdimage.ubuntu.com) and unpacks it to `work/ubuntu-base-rootfs/<ver>`
     with a `current` symlink. Idempotent; override mirrors with
     `UBUNTU_BASE_MIRRORS`, version with `UBUNTU_BASE_VERSION`.
2. **Session binaries** (`udesk` desktop, `udesk-init` PID 1, `shutdown`,
   `swap-probe`) are compiled **against our musl toolchain**
   (`toolchain/riscv64-rux-linux-musl`, built by `make sdk`), statically,
   with `-march=rv64gc_zicsr`, and installed into the rootfs;
   `/sbin/init` → `udesk-init` (ubuntu-base ships no init — it is a
   container rootfs — so the guard for `init.dist` is best-effort).
3. **Image**: ext4, `-b 4096 -O ^metadata_csum -I 256` (Rux ext4
   requirements), 400 MB filesystem in a 700 MB raw image — the tail 300 MB
   is deliberately left as non-fs space for the kernel swap carve
   (`Kernel.toml swap_size_mb=256`); do NOT resize2fs to fill.

## The two ISA traps (why "it just SIGILLs" happens on new machines)

Ubuntu 25.10+ raised the riscv64 baseline to RVA23 **including the vector
extension**. Two consequences bit us; both are now handled:

1. **Host cross-gcc static links host glibc** — that glibc contains
   vectorized `memcpy`/`memset` selected at runtime. Even a `-static`
   binary built on such a host dies with SIGILL (tval `0xcd…057` =
   `vsetivli`) on QEMU's plain `rv64` CPU. **Fix**: build session binaries
   against our musl (built with `-march=rv64gc_zicsr` in
   `toolchain/build-musl.sh`); link with `-lc -lgcc` (`__addtf3` etc. live
   in libgcc).
2. **Zbb residue**: even with march pinned, a few `zext.b` may survive from
   libgcc — harmless **as long as QEMU runs with the stateless bitmanip
   extensions enabled** (see the run command below; V stays forbidden: the
   kernel does not save vector state).

## Canonical QEMU invocation (headless/CI form)

```bash
qemu-system-riscv64 -M virt -accel tcg,thread=single \
  -cpu rv64,zbb=true,zba=true,zbs=true -m 2G -smp 4 \
  -nographic -snapshot -serial mon:stdio \
  -drive file=work/ubuntu-gui.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -device virtio-gpu-pci \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux \
  -append "root=/dev/vda rw init=/sbin/init console=ttyS0"
```

Success markers on serial: `udesk-init: boot ok` → `[udesk] fb 1280x800` →
`[udesk] login screen up`. For the interactive desktop use `make ubuntu-run`
(SDL window; keyboard works both in the window and on serial).

## Prerequisites (fresh machine)

```bash
# toolchain + deps (sudo where needed)
rustup (nightly; targets riscv64gc-unknown-none-elf, riscv64gc-unknown-linux-musl)
apt install gcc-riscv64-linux-gnu binutils-riscv64-linux-gnu qemu-system-riscv
apt install e2fsprogs                             # mkfs.ext4 -d
make sdk                                           # musl (march-pinned)
make user toybox mrsh && make rootfs               # musl rootfs for CI shell
```

Machine-transferable state: none. The base rootfs downloads on demand, the
session binaries compile from `test/ubuntu-gui/*.c`, and the image is
assembled locally.
