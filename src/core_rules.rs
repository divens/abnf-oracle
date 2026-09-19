//! RFC 5234 Appendix B core rules, as a grammar constant.
//!
//! Always implicit, never configurable in v1 (SCOPE.md 4, 12 item 8). Parsed from ABNF source
//! rather than hand-built, so the core environment is exercised by the same code path as
//! everything else.
//!
//! Resolution is hygienic: a reference inside a core body resolves against core rules only, so
//! a user's `DIGIT = "x"` changes what *their* `DIGIT` means without silently redefining
//! `HEXDIG`, `LWSP` and everything downstream (SCOPE.md 4.1, D33).
//!
//! Lands in M1.3.
