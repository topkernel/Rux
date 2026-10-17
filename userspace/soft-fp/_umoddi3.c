/* Pin-clean replacement for Ubuntu libgcc _umoddi3.o (zbb encodings
 * SIGILL on the pinned QEMU CPU). rv64gc has hardware 64-bit divide,
 * so the operator compiles straight to div/divu/rem/remu.
 * File basename == archive member name; built by build-softfp.sh. */
unsigned long long __umoddi3(unsigned long long a, unsigned long long b) { return a % b; }
