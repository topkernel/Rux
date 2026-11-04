// SPDX-License-Identifier: GPL-2.0-or-later
#include <stdint.h>
#include <stdio.h>

struct histogram {
	unsigned long *buckets;
	unsigned long width;		// interval covered by one bucket
	unsigned long num;		// number of buckets
	unsigned long events;		// number of events logged

	unsigned long *oflows;		// events when overflow happened
	unsigned long oflow_bufsize;	// number of overflows that can be logged
	unsigned long oflow_count;	// number of events that overflowed
	uint64_t oflow_magnitude;	// sum of how many buckets overflowed by
};

struct histoset {
	struct histogram *histos;	// Group of related histograms (e.g. per cpu)
	struct histogram *sum;		// Accumulates events from all histos
	unsigned long num_histos;	// Not including sum
	unsigned long num_buckets;
};

#define HIST_OVERFLOW		1
#define HIST_OVERFLOW_MAG	2
#define HIST_OVERFLOW_LOG	4

int hist_init(struct histogram *h, unsigned long width, unsigned long num);
int hist_init_oflow(struct histogram *h, unsigned long num);
void hist_destroy(struct histogram *h);
int hist_sample(struct histogram *h, uint64_t sample);

#define HSET_PRINT_SUM		1
#define HSET_PRINT_JSON		2

int hset_init(struct histoset *hs, unsigned long histos, unsigned long bucket_width,
	      unsigned long num_buckets, unsigned long overflow);
void hset_destroy(struct histoset *hs);
void hset_print_bucket(struct histoset *hs, FILE *f, const char *pre,
		       unsigned long bucket, unsigned long flags);
void hist_print_json(struct histogram *h, FILE *f);
void hist_print_oflows(struct histogram *h, FILE *f);
