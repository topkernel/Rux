/* Pin-clean replacement for Ubuntu libgcc _divmoddi4.o (zbb
 * encodings SIGILL on the pinned QEMU CPU); see _divdi3.c. */
typedef long long i64;
typedef int word_type;
word_type __divmoddi4(i64 n, i64 d, i64 *rem)
{
    if (rem)
        *rem = n % d;
    return (word_type)(n / d);
}
