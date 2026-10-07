# Pinned-march soft-fp overlay (BUG-S005 / S004 toolchain hygiene)

Origin: gcc releases/gcc-15 `libgcc/soft-fp/*` + `libgcc/config/riscv/sfp-machine.h`
+ `include/longlong.h` (fetched from gcc.gnu.org gitweb, blob_plain). GPL-3.0+
with GCC runtime library exception, same as libgcc itself.

Why this exists: the host cross toolchain's prebuilt libgcc.a
(/usr/lib/gcc-cross/riscv64-linux-gnu/15) is compiled with
`-march=..._zbb..._zcb...`, so every static link that pulls a __*tf3/__floatsitf
soft-float member from `-lgcc` silently ships Zcb encodings (c.zext.w and
friends). The project's pinned QEMU CPU model (`-cpu rv64,zbb=true,zba=true,
zbs=true`) has no Zcb, and executing one of those halfwords raises
illegal_instruction -> the kernel correctly kills the process with SIGILL.
That killed /bin/sh (mrsh) inside `while ... i=$((i+1))` loops after two
iterations — misfiled as BUG-S005 ("core-dumping children corrupt the parent")
because the LTP repro loop ran abort01 children when it died.

Fix: compile the quad-precision conversion/arithmetic routines from gcc's own
sources with the same `-march=rv64gc_zicsr` pin as every other userspace
object, archive them as `libsoftfp-pin.a`, and link that archive AHEAD of
`-lgcc` so the pinned definitions preempt the zcb-bearing libgcc members.

`build-softfp.sh` refuses to emit an archive whose .riscv.attributes mention
zbb/zba/zbs/zcb, and the mrsh/toybox build scripts scan the FINAL linked
binary for `zcb1p0` and fail the build if it comes back — a zcb-attributed
member means some path still reached the unpinned libgcc.

Vendored file list: the TF routines statically linked userspace actually
resolves (__addtf3 __subtf3 __multf3 __divtf3 __eqtf2/__netf2 __getf2/__gttf2
__letf2/__lttf2 __extenddftf2 __extendsftf2 __trunctfdf2 __trunctfsf2
__fixtfsi __fixunstfsi __floatsitf __floatunsitf) plus their headers
(soft-fp.h quad.h double.h single.h op-*.h op-common.h) plus riscv
sfp-machine.h and longlong.h.
