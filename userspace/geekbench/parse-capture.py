#!/usr/bin/env python3
"""parse-capture.py — extract the Geekbench result JSON from the capture
server log and print the score table (overall + per-workload, grouped by
category, single-core and multi-core).

Usage: parse-capture.py <captures.log> [captures.log ...]
"""
import json
import re
import sys

CATEGORY = {
    "AES-XTS": "AES-XTS",
    "Text Compression": "Integer", "Image Compression": "Integer",
    "Navigation": "Integer", "HTML5": "Integer", "SQLite": "Integer",
    "PDF Rendering": "Integer", "Text Rendering": "Integer",
    "Clang": "Integer", "Camera": "Integer",
    "N-Body Physics": "Floating Point", "Rigid Body Physics": "Floating Point",
    "Gaussian Blur": "Floating Point", "Face Detection": "Floating Point",
    "Horizon Detection": "Floating Point", "Image Inpainting": "Floating Point",
    "HDR": "Floating Point", "Ray Tracing": "Floating Point",
    "Structure from Motion": "Floating Point", "Speech Recognition": "Floating Point",
    "Machine Learning": "Floating Point",
}


def payloads(path):
    data = open(path, "rb").read()
    for chunk in data.split(b"==== ")[1:]:
        body = chunk.split(b"\n", 1)[1]
        m = re.search(rb'\{.*\}', body, re.S)
        if not m:
            continue
        raw = m.group(0)
        try:
            yield json.loads(raw)
            continue
        except json.JSONDecodeError:
            pass
        depth = 0
        for i, b in enumerate(raw):
            if b == 0x7b:
                depth += 1
            elif b == 0x7d:
                depth -= 1
                if depth == 0:
                    try:
                        yield json.loads(raw[: i + 1])
                    except json.JSONDecodeError:
                        pass


def main():
    for path in sys.argv[1:]:
        for doc in payloads(path):
            if doc.get("document_type") != 0:
                continue
            sysname = doc.get("system", {})
            print("== run @%s  (runtime %.1f s)" % (
                doc.get("date", "?"), doc.get("runtime", 0)))
            print("  Single-Core Score  %s" % doc.get("score"))
            print("  Multi-Core Score   %s" % doc.get("multicore_score"))
            for s in doc.get("sections", []):
                print("  [%s] section score %s" % (s.get("name"), s.get("score")))
                for w in s.get("workloads", []):
                    cat = CATEGORY.get(w.get("name"), "Other")
                    print("    %-20s %-14s %6s" % (w.get("name"), cat, w.get("score")))


if __name__ == "__main__":
    main()
