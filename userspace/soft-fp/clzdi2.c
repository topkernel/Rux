/* Pinned __clzdi2/__clzsi2 for the soft-fp overlay (BUG-S005).
 *
 * gcc's soft-fp expands count_leading_zeros to __builtin_clz, which on
 * -march=rv64gc_zicsr (no Zbb) becomes a call to libgcc's __clzdi2 — a
 * member built with zbb+zcb attributes. Defining it here with a plain
 * binary search keeps the whole link pin-clean. Compiled with -fno-builtin
 * so gcc cannot fold this back into a __builtin_clz call (recursion).
 */

int __clzdi2(unsigned long x)
{
    int n = 0;
    if (!x)
        return 64;
    if (!(x & 0xffffffff00000000UL)) { n += 32; x <<= 32; }
    if (!(x & 0xffff000000000000UL)) { n += 16; x <<= 16; }
    if (!(x & 0xff00000000000000UL))  { n += 8;  x <<= 8; }
    if (!(x & 0xf000000000000000UL))  { n += 4;  x <<= 4; }
    if (!(x & 0xc000000000000000UL))  { n += 2;  x <<= 2; }
    if (!(x & 0x8000000000000000UL))  { n += 1; }
    return n;
}

int __clzsi2(unsigned int x)
{
    return __clzdi2((unsigned long)x) - 32;
}
