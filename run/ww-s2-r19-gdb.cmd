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
printf "WW-ARMED 10 watchpoints, GUARD=0xffffffff80a7a000\n"
continue
