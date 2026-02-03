# Build user programs for Rux OS

AS = aarch64-linux-gnu-as
OBJCOPY = aarch64-linux-gnu-objcopy
CC = aarch64-linux-gnu-gcc

.PHONY: all clean

all: hello.bin

hello.bin: hello.elf
	$(OBJCOPY) -O binary $< $@
	@echo "Built $@"

hello.elf: hello.S
	$(AS) $< -o $@

clean:
	rm -f *.elf *.bin
