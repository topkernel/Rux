# SPDX-License-Identifier: GPL-2.0-or-later
define test-feature
$(shell $(MAKE) OBJDIR="$(OBJDIR)" CFLAGS="$(CFLAGS)" CPPFLAGS="$(CPPFLAGS)" LDFLAGS="$(LDFLAGS)" \
-f feature/Makefile feature-$1 clean >/dev/null 2>/dev/null; echo $$?)
endef
