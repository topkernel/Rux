# Rux Kernel Testing

> This document has been split into two focused reports:

- **[Kernel Unit Test Report](unit-test-report.md)** — 60 test files, 901 PASS + 94 SKIP at last report, test framework, best practices
- **[Linux LTP Compatibility Test Report](linux-ltp-test-report.md)** — 1,838 compiled test binaries, ABI compatibility verification
- **[Formal Verification Test Report](formal-verification-report.md)** — 4-layer verification: 1,116 verify-crate test functions, 157 Kani proofs, 4 SPIN models, Miri CI

Additional documentation:

- [Test Encapsulation & Visibility](test-visibility.md) — `pub(crate)` visibility tradeoffs and future improvements

## virtio-net slirp E2E (R34)

`/test/slirp_test` (source `test/slirp_test.c`, built into the rootfs by
`test/mkrootfs.sh` alongside `nettest`) exercises the **real** virtio-net
MMIO device against QEMU user networking — unlike `nettest`, whose traffic
takes the 127.0.0.1 loopback short-circuit. Coverage: ARP request/reply,
TX queue-1 chain completion (no `wait_for_completion` hang), RX queue-0
buffer DMA + `phys_to_virt`, UDP round-trip (DNS query to slirp's virtual
resolver 10.0.2.3:53) and TCP connect (10.0.2.2:80 — refused by the host,
which validates inbound-RST handling). Run it in the guest:

```
/test/slirp_test        # prints SLIRP-UDP-PASS / SLIRP PASS, exit 0 on pass
```

### QEMU invocation

The kernel only enumerates **virtio-mmio** NICs, so the NIC must be
`virtio-net-device` (not the machine's default e1000, which is PCI and
invisible to the probe). The virt machine's virtio-mmio transports default
to **legacy v1**, which the driver rejects — pass `-global
virtio-mmio.force-legacy=false` for modern v2 devices. The device lands on
MMIO slot 7 (0x10008000, IRQ 8; QEMU attaches `-device virtio-*-device` in
reverse slot order). Note: the distro `/usr/bin/qemu-system-riscv64` has
slirp; a custom build without `--enable-slirp` fails with "network backend
'user' is not compiled into this binary".

```bash
/usr/bin/qemu-system-riscv64 \
    -M virt -accel tcg,thread=single -cpu rv64 -m 2G -smp 4 -nographic \
    -serial mon:stdio \
    -global virtio-mmio.force-legacy=false \
    -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -netdev user,id=n1 -device virtio-net-device,netdev=n1 \
    -kernel target/riscv64gc-unknown-none-elf/debug/rux \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0"
```

For packet-level debugging add `-object
filter-dump,id=f1,netdev=n1,file=/tmp/dump.pcap` and inspect with
wireshark/tcpdump. Expected session: ARP req/reply, UDP query+reply, one
SYN and one RST (host has no listener on :80). Last verified: R34 —
SLIRP PASS, loopback `/test/nettest` NETTEST PASS in the same boot.
