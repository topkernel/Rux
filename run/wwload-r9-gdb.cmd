set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/run/ww-hunt-frozen-rux.elf
target remote 127.0.0.1:12381
watch *(unsigned long long*)0xffffffff80fc88b0
commands
silent
printf "WW-HIT HEAL_LAST rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80a79050
commands
silent
printf "WW-HIT KERNEL_MAP rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8f78
commands
silent
printf "WW-HIT VMEMMAP_FLAG rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8d60
commands
silent
printf "WW-HIT ISIG_JIFFY rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8d68
commands
silent
printf "WW-HIT ISIG_SIGNO rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8d50
commands
silent
printf "WW-HIT TX_NOTICE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8d48
commands
silent
printf "WW-HIT DUMP_MAGIC rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80be8d70
commands
silent
printf "WW-HIT LEDGER_MAGIC rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
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
