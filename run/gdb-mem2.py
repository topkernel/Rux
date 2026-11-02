import gdb

gdb.execute("set pagination off")
gdb.execute("set confirm off")
gdb.execute("target remote :1234")

RSP = 0xFFFF88807C5BB440

print("=== pt_regs below fault rsp (stub pushes down) ===")
gdb.execute("x/40gx %#x" % (RSP - 0x140))

print("=== string context around rbp area (fault rbp region) ===")
gdb.execute("x/32gx %#x" % (0xFFFF88807C5BA2C0 - 0x40))
print("=== as strings: rbp-0x40 .. rbp+0x200 ===")
gdb.execute("x/s %#x" % (0xFFFF88807C5BA2C0 - 0x40))
gdb.execute("find /b %#x, %#x, 0x2f, 0x6b, 0x65, 0x72, 0x6e" % (0xFFFF88807C5B8000, 0xFFFF88807C5BC000))

print("=== full 16K stack region overview: first nonzero qword each 256B ===")
base = 0xFFFF88807C5B8000
for off in range(0, 0x4000, 0x100):
    nz = []
    for k in range(0, 0x100, 8):
        try:
            v = int(gdb.parse_and_eval("*(unsigned long*)%#x" % (base + off + k)))
        except gdb.error:
            continue
        if v != 0:
            nz.append("%#x:%#x" % (off + k, v))
    if nz:
        print("  +%#05x: %s" % (off, " ".join(nz[:6])))

gdb.execute("detach")
