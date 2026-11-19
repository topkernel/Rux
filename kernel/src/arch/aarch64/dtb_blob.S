.section .rodata
.balign 8
.globl aarch64_virt_dtb
aarch64_virt_dtb:
.incbin "/home/william/workspace/Rux/kernel/src/arch/aarch64/virt.dtb"
.globl aarch64_virt_dtb_end
aarch64_virt_dtb_end:
