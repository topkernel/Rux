set pagination off
set confirm off
target remote :12399
info threads
thread apply all printf "CPU-CR0 probe\n"
thread apply all info registers cr0
detach
quit
