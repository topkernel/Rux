set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/target/x86_64-unknown-none/debug/rux
target remote 127.0.0.1:12374
watch *(unsigned long long*)0xffffffff807c2000
commands
silent
printf "WW-HIT GUARD_CANARY_LO rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c27f8
commands
silent
printf "WW-HIT GUARD_CANARY_MID rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c3000
commands
silent
printf "WW-HIT BASE_LOCKED rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c3008
commands
silent
printf "WW-HIT BASE_LOCKED_INV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c3010
commands
silent
printf "WW-HIT DIV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c3018
commands
silent
printf "WW-HIT DIV_INV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c3020
commands
silent
printf "WW-HIT BOOT_OFFSET rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807c3028
commands
silent
printf "WW-HIT BOOT_OFFSET_INV rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15
x/24gx $rsp
continue
end
printf "WW-ARMED 8 watchpoints, GUARD=0xffffffff807c2000\n"
continue
