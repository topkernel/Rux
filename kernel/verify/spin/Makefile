# SPIN/Promela Model Checking for Rux Kernel
#
# Prerequisites: sudo apt-get install spin
#
# Usage:
#   make          - run all models, generate report
#   make check    - same as default
#   make clean    - remove generated files

MODELS  := $(sort $(wildcard *.pml))
REPORT  := spin-report.txt

.PHONY: all check clean

all: check

check: $(REPORT)

$(REPORT): $(MODELS)
	@echo "=== Rux Kernel SPIN Model Checking Report ===" > $@
	@echo "Date: $$(date)" >> $@
	@echo "Models: $(words $(MODELS)) files" >> $@
	@echo "" >> $@
	@PASS=0; FAIL=0; ERR=0; \
	for f in $(MODELS); do \
		echo ""; \
		echo "--- Checking $$f ---"; \
		echo "--- $$f ---" >> $@; \
		if ! spin -a $$f >> $@ 2>&1; then \
			echo "COMPILE ERROR: $$f" | tee -a $@; \
			ERR=$$((ERR+1)); \
			rm -f pan pan.* *.trail *.tmp; \
			continue; \
		fi; \
		if ! gcc -o pan pan.c -O2 >> $@ 2>&1; then \
			echo "COMPILE ERROR: $$f — pan.c build failed" | tee -a $@; \
			ERR=$$((ERR+1)); \
			rm -f pan pan.* *.trail *.tmp; \
			continue; \
		fi; \
		./pan -a > pan-out.txt 2>&1; \
		RETCODE=$$?; \
		cat pan-out.txt >> $@; \
		echo "" >> $@; \
		if grep -q "errors: 0" pan-out.txt; then \
			echo "PASS: $$f — 0 errors"; \
			echo "PASS: $$f" >> $@; \
			PASS=$$((PASS+1)); \
		else \
			echo "FAIL: $$f — check $(REPORT)"; \
			echo "FAIL: $$f" >> $@; \
			FAIL=$$((FAIL+1)); \
		fi; \
		rm -f pan pan.* *.trail *.tmp pan-out.txt; \
	done; \
	echo "" >> $@; \
	echo "=== Summary ===" >> $@; \
	echo "PASS: $$PASS  FAIL: $$FAIL  ERROR: $$ERR" >> $@; \
	echo ""; \
	echo "Summary: PASS=$$PASS  FAIL=$$FAIL  ERROR=$$ERR"; \
	echo "Report: $(REPORT)"

clean:
	rm -f pan pan.* *.trail *.tmp pan-out.txt $(REPORT)
