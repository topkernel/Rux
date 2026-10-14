/* Pin-clean replacement for Ubuntu libgcc _udivmoddi4.o (zbb
 * encodings SIGILL on the pinned QEMU CPU); see _divdi3.c. */
typedef unsigned long long u64;
u64 __udivmoddi4(u64 n, u64 d, u64 *rem)
{
    if (rem)
        *rem = n % d;
    return n / d;
}
