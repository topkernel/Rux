// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * Latency histograms
 *
 * Copyright 2023 Red Hat Inc.
 */

#include <errno.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include "histogram.h"

int hist_init(struct histogram *h, unsigned long width, unsigned long num)
{
	memset(h, 0, sizeof(*h));
	h->width = width;
	h->num = num;

	h->buckets = calloc(num, sizeof(unsigned long));
	if (!h->buckets)
		return -ENOMEM;

	return 0;
}

int hist_init_oflow(struct histogram *h, unsigned long num)
{
	h->oflow_bufsize = num;
	h->oflows = calloc(num, sizeof(unsigned long));
	if (!h->oflows)
		return -ENOMEM;

	return 0;
}

void hist_destroy(struct histogram *h)
{
	free(h->oflows);
	h->oflows = NULL;
	free(h->buckets);
	h->buckets = NULL;
}

int hist_sample(struct histogram *h, uint64_t sample)
{
	unsigned long bucket = sample / h->width;
	unsigned long extra;
	unsigned long event = h->events++;
	int ret;

	if (bucket < h->num) {
		h->buckets[bucket]++;
		return 0;
	}

	ret = HIST_OVERFLOW;
	extra = bucket - h->num;
	if (h->oflow_magnitude + extra > h->oflow_magnitude)
		h->oflow_magnitude += extra;
	else
		ret |= HIST_OVERFLOW_MAG;

	if (h->oflows) {
		if (h->oflow_count < h->oflow_bufsize)
			h->oflows[h->oflow_count] = event;
		else
			ret |= HIST_OVERFLOW_LOG;
	}

	h->oflow_count++;
	return ret;
}

int hset_init(struct histoset *hs, unsigned long num_histos,
	      unsigned long bucket_width, unsigned long num_buckets,
	      unsigned long overflow)
{
	unsigned long i;

	if (num_histos == 0)
		return -EINVAL;

	hs->num_histos = num_histos;
	hs->num_buckets = num_buckets;
	hs->histos = calloc(num_histos, sizeof(struct histogram));
	if (!hs->histos)
		return -ENOMEM;

	for (i = 0; i < num_histos; i++) {
		if (hist_init(&hs->histos[i], bucket_width, num_buckets))
			goto fail;
		if (overflow && hist_init_oflow(&hs->histos[i], overflow))
			goto fail;
	}

	return 0;

fail:
	hset_destroy(hs);
	return -ENOMEM;
}

void hset_destroy(struct histoset *hs)
{
	unsigned long i;

	if (hs->histos) {
		for (i = 0; i < hs->num_histos; i++)
			hist_destroy(&hs->histos[i]);
	}

	free(hs->histos);
	hs->histos = NULL;
}

void hset_print_bucket(struct histoset *hs, FILE *f, const char *pre,
		       unsigned long bucket, unsigned long flags)
{
	unsigned long long sum = 0;
	unsigned long i;

	if (bucket >= hs->num_buckets)
		return;

	for (i = 0; i < hs->num_histos; i++)
		sum += hs->histos[i].buckets[bucket];

	if (sum == 0)
		return;
	if (pre)
		fprintf(f, "%s", pre);

	for (i = 0; i < hs->num_histos; i++) {
		unsigned long val = hs->histos[i].buckets[bucket];

		if (i != 0)
			fprintf(f, "\t");
		fprintf(f, "%06lu", val);
	}

	if (flags & HSET_PRINT_SUM)
		fprintf(f, "\t%06llu", sum);

	fprintf(f, "\n");
}

void hist_print_json(struct histogram *h, FILE *f)
{
	unsigned long i;
	bool comma = false;

	for (i = 0; i < h->num; i++) {
		unsigned long val = h->buckets[i];

		if (val != 0) {
			if (comma)
				fprintf(f, ",");
			fprintf(f, "\n        \"%lu\": %lu", i, val);
			comma = true;
		}
	}

	fprintf(f, "\n");
}

void hist_print_oflows(struct histogram *h, FILE *f)
{
	unsigned long i;

	for (i = 0; i < h->oflow_count; i++) {
		if (i >= h->oflow_bufsize)
			break;
		if (i != 0)
			fprintf(f, " ");
		fprintf(f, "%05lu", h->oflows[i]);
	}

	if (i >= h->oflow_bufsize)
		fprintf(f, " # %05lu others", h->oflow_count - h->oflow_bufsize);
}
