/* Pin-clean replacement for Ubuntu libgcc's _ctzdi2.o member (zcb
 * encodings SIGILL on the pinned QEMU CPU). Binary search — no
 * __builtin_ctz (on rv64gc it would recurse into libgcc's member). */
int __ctzdi2(unsigned long x)
{
    if (!x)
        return 64;
    int n = 0;
    if (!(x & 0xffffffffUL)) { n += 32; x >>= 32; }
    if (!(x & 0xffffUL))     { n += 16; x >>= 16; }
    if (!(x & 0xffUL))       { n += 8;  x >>= 8; }
    if (!(x & 0xfUL))        { n += 4;  x >>= 4; }
    if (!(x & 0x3UL))        { n += 2;  x >>= 2; }
    if (!(x & 0x1UL))        { n += 1; }
    return n;
}
