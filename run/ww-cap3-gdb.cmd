set pagination off
set confirm off
set height 0
set width 0
set print thread-events off
file /home/william/rux-agents/wildwrite-hunt/run/ww-hunt-frozen-rux.elf
target remote 127.0.0.1:12403
watch *(unsigned long long*)0xffffffff80a7a000
commands
silent
printf "WW-CAP3-HIT GUARD_CANARY_LO rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7a7f8
commands
silent
printf "WW-CAP3-HIT GUARD_CANARY_MID rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7b000
commands
silent
printf "WW-CAP3-HIT BASE_LOCKED rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7b008
commands
silent
printf "WW-CAP3-HIT BASE_LOCKED_INV rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7b010
commands
silent
printf "WW-CAP3-HIT DIV rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7b018
commands
silent
printf "WW-CAP3-HIT DIV_INV rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7b020
commands
silent
printf "WW-CAP3-HIT BOOT_OFFSET rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7b028
commands
silent
printf "WW-CAP3-HIT BOOT_OFFSET_INV rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80fc8888
commands
silent
printf "WW-CAP3-HIT HPET_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8fa8
commands
silent
printf "WW-CAP3-HIT TSC_MULT rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8d80
commands
silent
printf "WW-CAP3-HIT CONSOLE_UART rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a79088
commands
silent
printf "WW-CAP3-HIT PHYS_RAM_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80e95f20
commands
silent
printf "WW-CAP3-HIT CMDLINE_PTR rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80a7c5c0
commands
silent
printf "WW-CAP3-HIT GDT_PER_CPU rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8db0
commands
silent
printf "WW-CAP3-HIT VIRTIO_MMIO_OFF rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f80
commands
silent
printf "WW-CAP3-HIT TSC_ACTIVE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f88
commands
silent
printf "WW-CAP3-HIT JIFFIES_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f90
commands
silent
printf "WW-CAP3-HIT TSC_CONT_OFF rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8fa0
commands
silent
printf "WW-CAP3-HIT TSC_BASE rip=%p\n", $rip
info symbol $rip
continue
end
watch *(unsigned long long*)0xffffffff80be8f98
commands
silent
printf "WW-CAP3-HOT JIFFIES rip=%p\n", $rip
info symbol $rip
continue
end
printf "WW-CAP3-ARMED 20 watchpoints\n"
continue
