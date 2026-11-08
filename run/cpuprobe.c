/* cpuprobe — dump the CPUID leaves llvmpipe's JIT keys on.
 *
 * LLVM's X86Subtarget picks its MCJIT code model from CPUID: family,
 * model, and the feature bits (SSE4.2, AVX, AVX2). "64-bit code
 * requested on a subtarget that doesn't support it" means the feature
 * discovery produced a 32-bit-only subtarget — wrong family/model or
 * missing long-mode bit (leaf 0x80000001 EDX[29]).
 */
#include <cpuid.h>
#ifndef __cpuid_count
#define __cpuid_count(l, s, a, b, c, d) __cpuid(l, a, b, c, d)
#endif
#include <stdio.h>

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    unsigned a, b, c, d;

    if (__get_cpuid(0, &a, &b, &c, &d)) {
        printf("CPUP max_leaf=%x vendor=%.4s%.4s%.4s\n", a,
               (char *)&b, (char *)&d, (char *)&c);
    }
    if (__get_cpuid(1, &a, &b, &c, &d)) {
        unsigned family = ((a >> 8) & 0xf) + ((a >> 20) & 0xff);
        unsigned model = ((a >> 4) & 0xf) | (((a >> 16) & 0xf) << 4);
        printf("CPUP leaf1 family=%u model=%u stepping=%u\n",
               family, model, a & 0xf);
        printf("CPUP leaf1 ecx=%08x edx=%08x\n", c, d);
        printf("CPUP   sse3=%d ssse3=%d sse41=%d sse42=%d avx=%d osxsave=%d\n",
               !!(c & 1), !!(c & (1 << 9)), !!(c & (1 << 19)),
               !!(c & (1 << 20)), !!(c & (1 << 28)), !!(c & (1 << 27)));
        printf("CPUP   sse=%d sse2=%d htt=%d\n",
               !!(d & (1 << 25)), !!(d & (1 << 26)), !!(d & (1 << 28)));
    }
    if (__get_cpuid(7, &a, &b, &c, &d) == 0) {
        printf("CPUP leaf7 ebx=%08x (avx2=%d bmi1=%d bmi2=%d)\n",
               b, !!(b & (1 << 5)), !!(b & (1 << 3)), !!(b & (1 << 8)));
    }
    {
        unsigned em;
        __cpuid(0x80000000, em, b, c, d);
        printf("CPUP 80000000 max_ext=%x\n", em);
        __cpuid(0x80000001, a, b, c, d);
        printf("CPUP 80000001 raw edx=%08x (lm=%d) ecx=%08x\n",
               d, !!(d & (1 << 29)), c);
    }
    {
        /* XGETBV (needs CR4.OSXSAVE; the ECX=1 XGETBV instruction): the
         * value llvmpipe's JIT reads to decide YMM/ZMM support. */
        unsigned lo = 0, hi = 0;
        asm volatile("xgetbv" : "=a"(lo), "=d"(hi) : "c"(0));
        printf("CPUP xcr0=%08x%08x (xmm=%d ymm=%d opmask=%d)\n",
               hi, lo, !!(lo & 2), !!(lo & 4), !!(lo & 32));
        __cpuid_count(0xd, 0, a, b, c, d);
        printf("CPUP leafD0 eax=%08x (xsave=%d avxstate=%d)\n",
               a, !!(a & 1), !!(a & 4));
        __cpuid_count(0xd, 1, a, b, c, d);
        printf("CPUP leafD1 eax=%08x (xsaveopt=%d)\n", a, !!(a & 1));
        /* leaf 7 subleaf 0 full */
        __cpuid_count(7, 0, a, b, c, d);
        printf("CPUP leaf70 eax=%08x ebx=%08x ecx=%08x edx=%08x\n", a, b, c, d);
    }
    printf("CPUP-DONE\n");
    return 0;
}
