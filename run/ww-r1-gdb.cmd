set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/target/x86_64-unknown-none/debug/rux
target remote 127.0.0.1:12371
watch *(unsigned long long*)0xffffffff807bf608
commands
silent
printf "WW-HIT BASE_LOCKED_INV old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807bf610
commands
silent
printf "WW-HIT BOOT_OFFSET_INV old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff807bf618
commands
silent
printf "WW-HIT DIV_INV old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80d02d90
commands
silent
printf "WW-HIT BASE_LOCKED old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80d02d98
commands
silent
printf "WW-HIT BOOT_OFFSET old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80d02da0
commands
silent
printf "WW-HIT DIV old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
watch *(unsigned long long*)0xffffffff80d02da8
commands
silent
printf "WW-HIT BASE old->new, rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
info registers
x/32gx $rsp
continue
end
printf "WW-ARMED 7 watchpoints\n"
continue
