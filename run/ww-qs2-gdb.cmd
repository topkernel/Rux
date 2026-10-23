set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/run/ww-hunt-frozen-rux.elf
target remote 127.0.0.1:12377
watch *(unsigned long long*)0xffffffff80fc88b0
commands
silent
printf "WW-HIT HEAL_LAST rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a79050
commands
silent
printf "WW-HIT KERNEL_MAP rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f78
commands
silent
printf "WW-HIT VMEMMAP_FLAG rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d60
commands
silent
printf "WW-HIT ISIG_JIFFY rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d68
commands
silent
printf "WW-HIT ISIG_SIGNO rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d50
commands
silent
printf "WW-HIT TX_NOTICE rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d48
commands
silent
printf "WW-HIT DUMP_MAGIC rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d70
commands
silent
printf "WW-HIT LEDGER_MAGIC rip=%p rsp=%p rbp=%p\n", $rip, $rsp, $rbp
info symbol $rip
continue
end
printf "WW-ARMED 8 storm-quiet candidates\n"
continue
