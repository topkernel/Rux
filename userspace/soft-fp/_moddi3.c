/* Pin-clean replacement for Ubuntu libgcc _moddi3.o (zbb encodings
 * SIGILL on the pinned QEMU CPU). rv64gc has hardware 64-bit divide,
 * so the operator compiles straight to div/divu/rem/remu.
 * File basename == archive member name; built by build-softfp.sh. */
long long __moddi3(long long a, long long b) { return a % b; }
