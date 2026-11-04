# SPDX-License-Identifier: GPL-2.0-or-later
all: feature-libcpupower

feature-libcpupower: $(OBJDIR)/test-libcpupower.o

$(OBJDIR)/test-libcpupower.o: feature/test-libcpupower.c
	@$(CC) $(CFLAGS) $(CPPFLAGS) $(LDFLAGS) $< -lcpupower -o $@

.PHONY: clean

clean:
	rm -f $(OBJDIR)/test-*.o
