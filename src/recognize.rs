//! The set-of-positions recognizer (SCOPE.md 6.2, 6.3).
//!
//! For an element and a start position, computes the set of *all* end positions at which the
//! element can finish. Unordered alternation and ambiguity are then correct by construction:
//! nothing ever commits to the first branch that matched.
//!
//! A recognizer is bound to one input for its lifetime, because its memo table is keyed by
//! `(rule, position)` and is meaningless against a different input (D2). Memoization is a pure
//! optimization: results with the memo disabled must be identical.
//!
//! Lands in M2.1 and M2.2.
