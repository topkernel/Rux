import gdb

gdb.execute("set pagination off")
gdb.execute("set confirm off")
gdb.execute("target remote :1234")
print("=== where ===")
gdb.execute("x/3i $pc")

RSP = 0xFFFF88807C5BB440
RBP = 0xFFFF88807C5BA2C0

print("=== fault-time rsp region (return addr of the call-to-0 at *RSP) ===")
gdb.execute("x/24gx %#x" % (RSP - 64))

print("=== rbp chain from fault-time rbp ===")
cur = RBP
for _ in range(12):
    try:
        frm = int(gdb.parse_and_eval("*(unsigned long*)%#x" % cur))
        ret = int(gdb.parse_and_eval("*(unsigned long*)(%#x + 8)" % cur))
        print("  rbp=%#x -> ret=%#x" % (cur, ret))
        if frm == 0 or (frm & 0xFFFF000000000000) == 0:
            break
        cur = frm
    except gdb.error as e:
        print("  walk failed at %#x: %s" % (cur, e))
        break

print("=== scan stack upward for pt_regs (marker rbx=0x00007ff000006430) ===")
# The stub pushed regs just above rsp; scan a window above RSP.
found = None
for off in range(0, 0x400, 8):
    a = RSP + off
    try:
        v = int(gdb.parse_and_eval("*(unsigned long*)%#x" % a))
    except gdb.error:
        continue
    if v == 0x00007FF000006430:
        found = a
        print("  rbx marker at %#x" % a)
        break
if found:
    print("=== pt_regs dump around marker (marker-0x50 .. marker+0x100) ===")
    gdb.execute("x/48gx %#x" % (found - 0x50))

gdb.execute("detach")
