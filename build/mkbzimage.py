#!/usr/bin/env python3
"""Build a Linux-protocol bzImage from the linked x86_64 kernel ELF.

bzImage = setup part (header data block, N sectors) + vmlinux.bin
(the ELF flattened to binary: boot stub @0x100000, kernel @0x200000).

QEMU's linuxboot option ROM parses the header, loads the kernel part at
0x100000, switches to protected mode itself and jumps to code32_start.
"""
import struct
import subprocess
import sys
import os

def main():
    elf = sys.argv[1]
    out = sys.argv[2]
    tmp_bin = out + ".vmlinux.bin"

    # Flatten the ELF to binary (LMAs become file offsets from 0x100000)
    subprocess.run([os.environ.get("OBJCOPY", "objcopy"), "-O", "binary", elf, tmp_bin], check=True)
    kernel = open(tmp_bin, "rb").read()

    # ---- setup part: assemble the trampoline fresh ----
    here = os.path.dirname(__file__)
    setup_src = os.path.join(here, "..", "kernel", "src", "arch", "x86_64", "setup.S")
    subprocess.run(["as", "--32", "-o", out + ".setup.o", setup_src], check=True)
    subprocess.run([os.environ.get("OBJCOPY", "objcopy"), "-O", "binary",
                    "--only-section=.setup", out + ".setup.o", out + ".setup.bin"], check=True)
    setup = bytearray(open(out + ".setup.bin", "rb").read())
    assert len(setup) <= 2048, "setup trampoline too big"
    os.unlink(out + ".setup.o"); os.unlink(out + ".setup.bin")
    setup.extend(b"\x00" * (2048 - len(setup)))
    def put(off, data):
        setup[off:off+len(data)] = data
    # syssize = kernel size in 16-byte paragraphs
    put(0x1f4, struct.pack("<I", (len(kernel) + 15) // 16))
    put(0x1f1, bytes([3]))                             # setup_sects = 3 (4 sectors)

    with open(out, "wb") as f:
        f.write(setup)
        f.write(kernel)
    os.unlink(tmp_bin)
    print(f"bzImage: {out} ({len(setup)+len(kernel)} bytes, kernel {len(kernel)})")

if __name__ == "__main__":
    main()
