import gdb

gdb.execute("set pagination off")
gdb.execute("set confirm off")
gdb.execute("target remote :1234")
gdb.execute("hbreak *0xffffffff804cb3d0")  # rust_begin_unwind
gdb.execute("continue")

gdb.execute("bt 30")

# Find the do_page_fault frame and pull its `regs` argument (pt_regs at fault).
found = False
for i in range(0, 30):
    try:
        gdb.execute("frame %d" % i)
    except gdb.error:
        break
    name = gdb.selected_frame().name() or ""
    if "do_page_fault" in name or "handle_page_fault" in name:
        print("=== FRAME %d: %s ===" % (i, name))
        try:
            gdb.execute("info args")
        except gdb.error as e:
            print("info args failed: %s" % e)
        try:
            regs = gdb.parse_and_eval("regs")
            print("regs ptr: %s" % regs)
            for f in ("rip", "rsp", "rbp", "cs", "ss", "rax", "rdi", "rsi", "rdx", "rbx", "r12"):
                try:
                    v = regs[f].cast(gdb.lookup_type("unsigned long"))
                    print("  pt_regs.%-4s = %#x" % (f, int(v)))
                except gdb.error:
                    pass
        except gdb.error as e:
            print("regs eval failed: %s" % e)
        found = True
        break
if not found:
    print("do_page_fault frame not found by name; trying raw walk")

# Regardless: dump the words at the printed fault rsp (call-to-0 return addr).
for addr in (0xFFFF88807C5BB440, 0xFFFF88807C5BBA50):
    try:
        print("=== stack words at %#x ===" % addr)
        gdb.execute("x/16gx %#x" % addr)
    except gdb.error as e:
        print("dump at %#x failed: %s" % (addr, e))

# Walk the rbp chain from the fault-time rbp.
for rbp in (0xFFFF88807C5BA2C0, 0xFFFF88807C5BA8C0):
    try:
        print("=== rbp chain from %#x ===" % rbp)
        cur = rbp
        for _ in range(10):
            frm = gdb.parse_and_eval("*(unsigned long*)%#x" % cur)
            ret = gdb.parse_and_eval("*(unsigned long*)(%#x + 8)" % cur)
            print("  rbp=%#x ret=%#x" % (int(cur), int(ret)))
            cur = int(frm)
            if cur == 0 or cur == 0xFFFFFFFFFFFFFFFF:
                break
    except gdb.error as e:
        print("rbp walk from %#x failed: %s" % (rbp, e))

gdb.execute("detach")
