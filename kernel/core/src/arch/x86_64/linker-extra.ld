/* MIT License
 *
 * Copyright (c) 2026 Fei Wang
 *
 * Supplement to arch/x86_64/linker.ld, appended via a second -T (the main
 * script lives in the pinned arch tree). Places the GOT (sysroot
 * compiler-builtins always reference memcpy/__udivti3 through GOTPCREL)
 * and discards debug sections the main script does not handle.
 */

SECTIONS
{
    . = ALIGN(8);
    .got : { *(.got .got.*) }

    /DISCARD/ :
    {
        *(.debug*)
        *(.gnu*)
    }
}
