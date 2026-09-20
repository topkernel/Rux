# Rux Kernel Testing

> This document has been split into two focused reports:

- **[Kernel Unit Test Report](unit-test-report.md)** — 60 test files, 901 PASS + 94 SKIP at last report, test framework, best practices
- **[Linux LTP Compatibility Test Report](linux-ltp-test-report.md)** — 1,838 compiled test binaries, ABI compatibility verification
- **[Formal Verification Test Report](formal-verification-report.md)** — 4-layer verification: 1,116 verify-crate test functions, 157 Kani proofs, 4 SPIN models, Miri CI

Additional documentation:

- [Test Encapsulation & Visibility](test-visibility.md) — `pub(crate)` visibility tradeoffs and future improvements
