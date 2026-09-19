//! Advisory warnings. A pure function of a checked grammar; never fails, never consulted by the
//! recognizer or generator (SCOPE.md 6.6, D5).
//!
//! `lint` reports what can be known without entry points — unreferenced, unproductive, shadowing
//! a core rule — and `lint_from` adds unreachability, which needs the caller to name start
//! rules, since a grammar may legitimately expose several.
//!
//! Lands in M1.8.
