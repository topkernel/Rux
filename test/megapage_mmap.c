/*
 * E8 Bug-2 reproducer: fixed-address 4K mmap over a 2MB device megapage.
 *
 * gnome-shell (gjs/mozjs) reserved its JS heap cage at 0x0c000000 — the
 * exact VA where every address space carries the kernel's cloned PLIC
 * 2MB megapage. The kernel refused each 4K map ("collides with 2MB
 * megapage — refused") 64 times in a row and the first cage touch died
 * with pagefault: Permission denied at 0x0c01f0f8 -> SIGSEGV (sig=11).
 *
 * With the megapage-demotion fix this must behave like Linux: the
 * fixed-address mapping REPLACES the underlying translation for the
 * mapped pages only; neighbours keep their device translations; unmap
 * restores them; a re-map works again.
 *
 * Success = prints PASS lines and exits 0.
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -o megapage_mmap megapage_mmap.c
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <sys/mman.h>

#define CAGE_BASE   0x0c000000UL   /* PLIC window (megapaged) */
#define TOUCH_PAGES 64             /* the 64 consecutive pages the forensic saw refused */
#define PAGE_SIZE   4096UL

static int touch_all(unsigned char *p, unsigned seed)
{
	for (int i = 0; i < TOUCH_PAGES; i++) {
		unsigned char *pg = p + (unsigned long)i * PAGE_SIZE;
		pg[0] = (unsigned char)(seed + i);
		pg[PAGE_SIZE - 1] = (unsigned char)(seed + i + 1);
		pg[0x1f0f8 & (PAGE_SIZE - 1)] = 0x5a; /* the forensic fault offset */
		if (pg[0] != (unsigned char)(seed + i))
			return -1;
	}
	return 0;
}

int main(void)
{
	const size_t len = TOUCH_PAGES * PAGE_SIZE;

	/* 1. The gjs cage pattern: SHARED|ANONYMOUS|FIXED at the megapage. */
	unsigned char *p = mmap((void *)CAGE_BASE, len,
				PROT_READ | PROT_WRITE,
				MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
	if (p == MAP_FAILED) {
		printf("megapage_mmap: FAIL step1 mmap: %s\n", strerror(errno));
		return 1;
	}
	if (p != (void *)CAGE_BASE) {
		printf("megapage_mmap: FAIL step1 placement got %p\n", p);
		return 1;
	}
	if (touch_all(p, 1) != 0) {
		printf("megapage_mmap: FAIL step1 touch/readback\n");
		return 1;
	}
	printf("megapage_mmap: PASS fixed mmap+touch over megapage\n");

	/* 2. Unmap: must restore the device translation underneath (the
	 * kernel touches PLIC registers on this satp) — and not crash. */
	if (munmap(p, len) != 0) {
		printf("megapage_mmap: FAIL step2 munmap: %s\n", strerror(errno));
		return 1;
	}
	printf("megapage_mmap: PASS munmap (device window restored)\n");

	/* 3. Re-map the same range — demote/restore must be repeatable. */
	p = mmap((void *)(CAGE_BASE + PAGE_SIZE * 8), len / 2,
		 PROT_READ | PROT_WRITE,
		 MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
	if (p == MAP_FAILED) {
		printf("megapage_mmap: FAIL step3 remap: %s\n", strerror(errno));
		return 1;
	}
	for (int i = 0; i < (int)(len / 2 / PAGE_SIZE); i++) {
		unsigned char *pg = p + (unsigned long)i * PAGE_SIZE;
		pg[7] = (unsigned char)i;
		if (pg[7] != (unsigned char)i) {
			printf("megapage_mmap: FAIL step3 readback at page %d\n", i);
			return 1;
		}
	}
	printf("megapage_mmap: PASS remap after munmap\n");

	/* 4. A large reservation spanning several megapages (the 192MB cage
	 * reservation shape), touched sparsely. */
	unsigned char *big = mmap((void *)(CAGE_BASE + 0x400000UL), 0x0c000000UL,
				  PROT_NONE,
				  MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED | MAP_NORESERVE,
				  -1, 0);
	if (big == MAP_FAILED) {
		printf("megapage_mmap: FAIL step4 reserve: %s\n", strerror(errno));
		return 1;
	}
	if (mprotect(big, PAGE_SIZE * 4, PROT_READ | PROT_WRITE) != 0) {
		printf("megapage_mmap: FAIL step4 mprotect: %s\n", strerror(errno));
		return 1;
	}
	for (int i = 0; i < 4; i++)
		big[(unsigned long)i * PAGE_SIZE + 13] = (unsigned char)(i + 9);
	if (big[13] != 9) {
		printf("megapage_mmap: FAIL step4 readback\n");
		return 1;
	}
	munmap(big, 0x0c000000UL);
	printf("megapage_mmap: PASS multi-megapage reservation\n");

	printf("megapage_mmap: DONE\n");
	return 0;
}
