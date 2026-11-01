# OpenHarmony Phase 1 Bring-up Runbook

> This is the machine-transferable checklist for running the built OH
> x86_64_virt images on Rux, on the machine that has the oh-robot tree.
> Everything on the kernel side is already merged to `main`; this document
> only covers fetching, booting, and reading results.

## 0. Prerequisites on the OH machine

```bash
git clone --depth 1 https://github.com/topkernel/Rux.git   # or your mirror
cd Rux
rustup target add riscv64gc-unknown-none-elf x86_64-unknown-none

# Build the x86 kernel (bzImage)
echo 'platform_target=x86_64' > build/.config
cargo build --target x86_64-unknown-none --no-default-features --features x86_64
python3 build/mkbzimage.py target/x86_64-unknown-none/debug/rux \
    target/x86_64-unknown-none/debug/bzImage
echo 'platform_target=riscv64' > build/.config   # restore default
```

QEMU ≥ 6.2 with `qemu-system-x86_64`.

## 1. Images

From the oh-robot tree, take the built package set (reference path:
`out/x86_64_virt/packages/phone/images/`):

| file | becomes |
|---|---|
| `ramdisk.img` | the initrd (gzip cpio — our unpacker handles it) |
| `updater.img` | vda |
| `system.img` | vdb → `/usr` (required mount) |
| `vendor.img` | vdc → `/vendor` (required mount) |
| `sys_prod.img` | vdd |
| `chip_prod.img` | vde |
| `userdata.img` | vdf → `/data` |

Copy them into one directory, e.g. `~/oh-images/`.

## 2. Boot

```bash
cd Rux
./test/run-oh.sh ~/oh-images console
```

The script mirrors the reference qemu scripts (q35, TCG, six virtio disks,
virtio-net/kb/tablet) and passes the ohos.* cmdline tokens verbatim.
`gui` mode adds the virtio-gpu at the reference 720x1280.

## 3. The Phase 1 gate (what to look for on serial)

In order — each line is a kernel-side milestone:

1. Kernel banner → subsystem table (both arches boot this far).
2. `initrd:` unpack summary — ramdisk extracted, `/init` found.
3. `/init` (= init_early) runs from the initramfs — watch for OH init's
   first lines (it prints through /dev/kmsg; console shows them after it
   mknods /dev/kmsg).
4. ueventd creates `/dev/block/vdb`, `/dev/block/vdc` from our uevents
   (SO_PASSCRED credentials + DEVTYPE=disk are already in).
5. `vdb` mounts → `/usr`, `vdc` → `/vendor`, switch_root, second-stage
   `/bin/init` starts: `ServiceStart` lines for ueventd/samgr.
   Services will crash immediately (no binder in this gate? note: binder
   IS present at samgr-lifecycle level — crashes here are bug reports,
   not expected).
6. Gate pass = second-stage init reaches ServiceStart and stays up 60s.

## 4. If it stops early — what to collect

- Full serial log (console= is ttyS0; everything lands there).
- Where it stopped vs the list above.
- Kernel-side tripwires already print on anomaly (WAKE-WILD-PTR,
  R56-STEAL, OOM-FORENSIC) — include any of those lines.
- `DUMP!` typed into the console triggers a task dump on any wedge.

## 5. Known-good reference numbers

- Ubuntu amd64 on the same kernel: boots to the graphical desktop
  (`test/run-x86-ub.sh`), LTP r3 = 850/1869.
- initrd path: `test/initrd/` probes boot on riscv64 (11/11 markers).
- binder: samgr-lifecycle probe passes 24+ rounds (death notifications,
  fd passing, three contexts, SECCTX).

## 6. Commit results back

Serial log + where it stopped → open an issue or paste into the agent
session; every kernel-side gap found this way becomes a fix task.
