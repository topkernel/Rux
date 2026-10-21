set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/run/ww-hunt-frozen-rux.elf
target remote 127.0.0.1:12399
watch *(unsigned long long*)0xffffffff80fc8888
commands
silent
printf "WW-CAP-HIT HPET_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d80
commands
silent
printf "WW-CAP-HIT CONSOLE_UART rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8fa8
commands
silent
printf "WW-CAP-HIT TSC_MULT rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a79088
commands
silent
printf "WW-CAP-HIT PHYS_RAM_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80e95f20
commands
silent
printf "WW-CAP-HIT CMDLINE_PTR rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7c5c0
commands
silent
printf "WW-CAP-HIT GDT_PER_CPU rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8db0
commands
silent
printf "WW-CAP-HIT VIRTIO_MMIO_OFF rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f88
commands
silent
printf "WW-CAP-HIT JIFFIES_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f98
commands
silent
printf "WW-CAP-HIT JIFFIES rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80e81428
commands
silent
printf "WW-CAP-HIT LAST_TICK rip=%p\n", $rip
info symbol $rip
continue
end
printf "WW-CAP-ARMED 10 watchpoints\n"
continue
