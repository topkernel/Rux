set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/run/ww-hunt-frozen-rux.elf
target remote 127.0.0.1:12371
watch *(unsigned long long*)0xffffffff80a7a000
commands
silent
printf "WW-HIT GUARD_CANARY_LO rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7a7f8
commands
silent
printf "WW-HIT GUARD_CANARY_MID rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7b000
commands
silent
printf "WW-HIT BASE_LOCKED rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7b008
commands
silent
printf "WW-HIT BASE_LOCKED_INV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7b010
commands
silent
printf "WW-HIT DIV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7b018
commands
silent
printf "WW-HIT DIV_INV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7b020
commands
silent
printf "WW-HIT BOOT_OFFSET rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7b028
commands
silent
printf "WW-HIT BOOT_OFFSET_INV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80fc8888
commands
silent
printf "WW-HIT HPET_BASE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8fa8
commands
silent
printf "WW-HIT TSC_MULT rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8d80
commands
silent
printf "WW-HIT CONSOLE_UART rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a79088
commands
silent
printf "WW-HIT PHYS_RAM_BASE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80e95f20
commands
silent
printf "WW-HIT CMDLINE_PTR rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a7c5c0
commands
silent
printf "WW-HIT GDT_PER_CPU rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8db0
commands
silent
printf "WW-HIT VIRTIO_MMIO_OFF rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8f80
commands
silent
printf "WW-HIT TSC_ACTIVE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8f88
commands
silent
printf "WW-HIT JIFFIES_BASE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8f90
commands
silent
printf "WW-HIT TSC_CONT_OFF rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8fa0
commands
silent
printf "WW-HIT TSC_BASE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
printf "WW-ARMED 19 watchpoints, GUARD=0xffffffff80a7a000\n"
continue
